mod auto_import;
mod character_view;
mod player_view;
mod ranking_view;
mod report_access;
mod report_view;
mod section_view;
mod upload;

use axum::body::Body;
use axum::extract::{FromRequest, OriginalUri, Path as UrlPath, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::{Form, Json, Router, routing::get};
use minijinja::{Environment, context, path_loader};
use raidledger::combat::CATALOG;
use raidledger::metrics::aggregate;
use raidledger::reports::{Encounter, LegacyGroups, Report, import_capture, list_ids};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::io::{ReaderStream, SyncIoBridge};
use tower_http::services::{ServeDir, ServeFile};

#[derive(Clone)]
struct App {
    root: Arc<PathBuf>,
    templates: Arc<Environment<'static>>,
    uploads: Arc<upload::Uploads>,
    rankings: Arc<std::sync::Mutex<raidledger::rankings::Index>>,
    realms: Arc<character_view::Realms>,
    access: Arc<report_access::Access>,
    legacy_groups: Arc<LegacyGroups>,
    worker_status: auto_import::Status,
    viewer: Option<String>,
}

#[derive(Debug)]
struct Error(StatusCode, &'static str);

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::NotFound {
            return Self(StatusCode::NOT_FOUND, "Report not found");
        }
        eprintln!("{error}");
        Self(StatusCode::INTERNAL_SERVER_ERROR, "Could not read report")
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, self.1).into_response()
    }
}

impl App {
    fn render(&self, template: &str, data: impl Serialize) -> Result<Html<String>, Error> {
        self.templates
            .get_template(template)
            .and_then(|template| template.render(data))
            .map(Html)
            .map_err(|error| {
                eprintln!("{error}");
                Error(StatusCode::INTERNAL_SERVER_ERROR, "Could not render page")
            })
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| Error(StatusCode::INTERNAL_SERVER_ERROR, "Report worker failed"))?
}

async fn json_or_form<T: serde::de::DeserializeOwned>(request: Request) -> Result<T, Error> {
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, 2 * 1024 * 1024)
        .await
        .map_err(|_| Error(StatusCode::PAYLOAD_TOO_LARGE, "Report query is too large"))?;
    if let Ok(value) = serde_json::from_slice(&bytes) {
        return Ok(value);
    }
    let request = Request::from_parts(parts, Body::from(bytes));
    let Form(fields) =
        Form::<std::collections::BTreeMap<String, String>>::from_request(request, &())
            .await
            .map_err(|_| Error(StatusCode::BAD_REQUEST, "Invalid report query"))?;
    serde_json::from_value(serde_json::json!(fields))
        .map_err(|_| Error(StatusCode::BAD_REQUEST, "Invalid report query"))
}

#[derive(Serialize)]
struct LibraryRow {
    id: String,
    author: String,
    server: String,
    date: String,
    time: String,
    datetime: String,
    year: u32,
    day: u32,
    players: usize,
    raid_label: String,
    fights: String,
    progress: String,
    progress_note: &'static str,
    #[serde(skip)]
    matched: bool,
}

fn library_row(
    report: Report,
    snapshots: Option<&auto_import::SavedProgress>,
    query: &LibraryQuery,
) -> LibraryRow {
    let fields = report.id.splitn(4, "--").collect::<Vec<_>>();
    let year = report.id[..2].parse::<u32>().unwrap_or(0) + 2000;
    let date = format!("{year}{}", fields[0].get(2..).unwrap_or(""));
    let day = date
        .get(8..10)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let time = fields[1].get(..5).unwrap_or(fields[1]).replace('-', ":");
    let fights = report
        .metadata
        .encounters
        .iter()
        .map(|encounter| encounter.boss.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let kills = report
        .metadata
        .encounters
        .iter()
        .filter(|encounter| encounter.outcome == "kill")
        .map(|encounter| encounter.boss.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let raids = CATALOG
        .raids
        .iter()
        .enumerate()
        .filter(|(_, raid)| fights.iter().any(|boss| raid.bosses.contains(*boss)))
        .collect::<Vec<_>>();
    let raid_label = if raids.is_empty() {
        "Рейдовый отчёт".to_owned()
    } else {
        raids
            .iter()
            .map(|(_, raid)| raid.name.as_str())
            .collect::<Vec<_>>()
            .join(" / ")
    };
    let (progress, progress_note) = if let [(index, raid)] = raids.as_slice() {
        if let Some((completed, total)) =
            snapshots.and_then(|snapshots| snapshots.report(&report, *index))
        {
            (
                format!("{completed}/{total}"),
                "Прогресс сохранения, включая боссов до входа в рейд",
            )
        } else {
            (
                format!("≥{}/{}", kills.len(), raid.total),
                "Количество убийств, подтверждённых журналом",
            )
        }
    } else {
        ("—".to_owned(), "Рейд не определён")
    };
    let text_matches = |filter: &Option<String>, value: &str| {
        filter
            .as_deref()
            .is_none_or(|filter| filter.is_empty() || value.eq_ignore_ascii_case(filter))
    };
    let matched = text_matches(&query.author, fields[2])
        && text_matches(&query.server, fields[3])
        && (query.player.as_deref().is_none_or(str::is_empty)
            || report
                .metadata
                .players
                .values()
                .any(|name| text_matches(&query.player, name)))
        && (query.fight.as_deref().is_none_or(str::is_empty)
            || fights.iter().any(|boss| text_matches(&query.fight, boss)));
    LibraryRow {
        matched,
        author: fields[2].into(),
        server: fields[3].into(),
        date: date.clone(),
        time: time.clone(),
        datetime: format!("{date}T{time}"),
        year,
        day,
        id: report.id.clone(),
        players: report.metadata.players.len(),
        raid_label,
        fights: fights.into_iter().collect::<Vec<_>>().join(" · "),
        progress,
        progress_note,
    }
}

#[derive(Default, Deserialize)]
struct LibraryQuery {
    server: Option<String>,
    year: Option<u32>,
    month: Option<u32>,
    author: Option<String>,
    player: Option<String>,
    fight: Option<String>,
}

async fn library(
    State(app): State<App>,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<LibraryQuery>,
) -> Result<Html<String>, Error> {
    let path = uri.path().to_owned();
    blocking(move || {
        let mut rows = Vec::new();
        let snapshots = auto_import::SavedProgress::load(&app.root).ok();
        let mut realms = std::collections::BTreeSet::new();
        for id in list_ids(&app.root)?
            .into_iter()
            .filter(|id| !app.legacy_groups.is_fragment(id))
            .filter(|id| app.access.public(id))
            .take(100)
        {
            if let Some((_, realm)) = id.rsplit_once("--") {
                realms.insert(realm.to_owned());
            }
            match Report::open_group(&app.root, &id, &app.legacy_groups) {
                Ok(report) => rows.push(library_row(report, snapshots.as_ref(), &query)),
                Err(error) => eprintln!("Cannot open {id}: {error}"),
            }
        }
        if path != "/" {
            return render_calendar(&app, &path, &query, rows, realms);
        }
        app.render(
            "home.html",
            context! { request => context! { path }, LOCAL_REPORTS => rows },
        )
    })
    .await
}

async fn library_filter(
    State(app): State<App>,
    body: axum::body::Bytes,
) -> Result<Json<Vec<String>>, Error> {
    let mut filter =
        serde_json::from_slice::<std::collections::BTreeMap<String, serde_json::Value>>(&body)
            .map_err(|_| Error(StatusCode::BAD_REQUEST, "Invalid report filter"))?;
    filter.retain(|key, value| {
        matches!(
            key.as_str(),
            "year" | "month" | "day" | "time" | "author" | "server" | "player" | "fight"
        ) && (value.as_str().is_some_and(|text| !text.is_empty())
            || value.as_u64().is_some_and(|number| number > 0))
    });
    if filter.is_empty() {
        return Err(Error(
            StatusCode::IM_A_TEAPOT,
            "At least one report filter is required",
        ));
    }
    blocking(move || {
        let mut matches = Vec::new();
        for id in list_ids(&app.root)?
            .into_iter()
            .filter(|id| !app.legacy_groups.is_fragment(id))
            .filter(|id| app.access.public(id))
        {
            let report = Report::open_group(&app.root, &id, &app.legacy_groups)?;
            let fields = id.splitn(4, "--").collect::<Vec<_>>();
            let date = fields[0].split('-').collect::<Vec<_>>();
            let yes = filter.iter().all(|(key, value)| {
                let text = value.as_str().unwrap_or_default();
                let number = value.as_u64().or_else(|| text.parse().ok());
                let component = |index| {
                    date.get(index)
                        .and_then(|value: &&str| value.parse::<u64>().ok())
                };
                match key.as_str() {
                    "year" => component(0).is_some_and(|year| {
                        number.is_some_and(|number| number == year || number == year + 2000)
                    }),
                    "month" => component(1) == number,
                    "day" => component(2) == number,
                    "time" => fields[1].replace('-', ":").contains(text),
                    "author" => fields[2].contains(text),
                    "server" => fields[3].contains(text),
                    "player" => report.metadata.players.values().any(|name| name == text),
                    "fight" => report
                        .metadata
                        .encounters
                        .iter()
                        .any(|fight| fight.boss == text),
                    _ => false,
                }
            });
            if yes {
                matches.push(id);
            }
        }
        Ok(Json(matches))
    })
    .await
}

fn render_calendar(
    app: &App,
    path: &str,
    query: &LibraryQuery,
    reports: Vec<LibraryRow>,
    realms: std::collections::BTreeSet<String>,
) -> Result<Html<String>, Error> {
    use raidledger::combat::days_in_month;
    let (default_year, default_month) = reports
        .first()
        .map(|report| {
            (
                report.year,
                report
                    .date
                    .get(5..7)
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or(1)
                    .saturating_sub(1)
                    .min(11),
            )
        })
        .unwrap_or((2026, 0));
    let year = query
        .year
        .filter(|year| (2000..=2100).contains(year))
        .unwrap_or(default_year);
    let month = query
        .month
        .filter(|month| *month < 12)
        .unwrap_or(default_month);
    const MONTH_NAMES: [&str; 12] = [
        "Январь",
        "Февраль",
        "Март",
        "Апрель",
        "Май",
        "Июнь",
        "Июль",
        "Август",
        "Сентябрь",
        "Октябрь",
        "Ноябрь",
        "Декабрь",
    ];
    let calendar_month = |year, month| {
        let elapsed = (2000..year)
            .flat_map(|year| (1..=12).map(move |month| days_in_month(year, month)))
            .sum::<u32>()
            + (1..month + 1)
                .map(|month| days_in_month(year, month))
                .sum::<u32>();
        let start = (elapsed + 5) % 7;
        let days = days_in_month(year, month + 1);
        let previous = (month + 11) % 12;
        let previous_days = days_in_month(if month == 0 { year - 1 } else { year }, previous + 1);
        let size = (start + days).div_ceil(7) * 7;
        (0..size)
            .map(|position| {
                let (month, day) = if position < start {
                    (previous, previous_days + position + 1 - start)
                } else if position >= start + days {
                    ((month + 1) % 12, position + 1 - start - days)
                } else {
                    (month, position + 1 - start)
                };
                (format!("{:02}-{day:02}", month + 1), day)
            })
            .collect::<Vec<_>>()
            .chunks(7)
            .map(<[_]>::to_vec)
            .collect::<Vec<_>>()
    };
    if path == "/logs_list" {
        let mut weeks = calendar_month(year, month);
        let last_month = (month + 11) % 12;
        let last_year = if month == 0 { year - 1 } else { year };
        let previous = calendar_month(last_year, last_month);
        if let Some(week) = previous
            .iter()
            .rev()
            .find(|week| Some(*week) != weeks.first())
        {
            weeks.insert(0, week.clone());
        }
        let allowed = [
            format!("{year}-{:02}-", month + 1),
            format!("{last_year}-{:02}-", last_month + 1),
        ];
        let mut days = std::collections::BTreeMap::<_, Vec<_>>::new();
        for row in reports
            .iter()
            .rev()
            .filter(|row| row.matched && allowed.iter().any(|prefix| row.date.starts_with(prefix)))
        {
            days.entry(&row.date[5..])
                .or_default()
                .push((&row.id, format!("{} · {}", row.time, row.raid_label)));
        }
        let fights = CATALOG
            .bosses
            .values()
            .collect::<std::collections::BTreeSet<_>>();
        return app.render(
            "logs_list.html",
            context! {
                request => context! { path }, MONTH => days, CALEND => weeks,
                LOCAL_REPORTS => reports, CURRENT_YEAR => year, CURRENT_MONTH => month,
                CURRENT_SERVER => query.server.as_deref().unwrap_or_default(),
                MONTHS => MONTH_NAMES.into_iter().enumerate().collect::<Vec<_>>(),
                YEARS => (2018..=year.max(default_year) + 1).collect::<Vec<_>>(),
                SERVERS => realms, ALL_FIGHT_NAMES => fights,
            },
        );
    }
    let selected = format!("{year}-{:02}-", month + 1);
    let reports = reports
        .into_iter()
        .filter(|report| report.matched && report.date.starts_with(&selected))
        .collect::<Vec<_>>();
    let days = (2000..year)
        .flat_map(|year| (1..=12).map(move |month| days_in_month(year, month)))
        .sum::<u32>()
        + (1..month + 1)
            .map(|month| days_in_month(year, month))
            .sum::<u32>();
    let start = (days + 5) % 7;
    let days = days_in_month(year, month + 1);
    let size = (start + days).div_ceil(7) * 7;
    let days = (0..size)
        .map(|position| {
            (start..start + days)
                .contains(&position)
                .then(|| position + 1 - start)
        })
        .collect::<Vec<_>>();
    let server = query.server.as_deref().unwrap_or_default();
    let link = |year, month| {
        format!(
            "{path}?year={year}&month={month}&server={}",
            path_segment(server)
        )
    };
    app.render("rust_calendar.html", context! {
        request => context! { path }, path, year, month, month_name => MONTH_NAMES[month as usize], reports, realms, server,
        weeks => days.chunks(7).collect::<Vec<_>>(),
        previous => link(if month == 0 { year-1 } else { year }, (month+11)%12),
        next => link(if month == 11 { year+1 } else { year }, (month+1)%12),
    })
}

#[derive(Default, Deserialize)]
struct ReportQuery {
    boss: Option<String>,
    mode: Option<String>,
    #[serde(default, deserialize_with = "query_attempt")]
    attempt: Option<String>,
    target: Option<String>,
    #[serde(default, deserialize_with = "query_number")]
    s: Option<u64>,
    #[serde(default, deserialize_with = "query_number")]
    f: Option<u64>,
    #[serde(default, deserialize_with = "query_number")]
    sc: Option<u64>,
    #[serde(default, deserialize_with = "query_number")]
    fc: Option<u64>,
}

fn query_attempt<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Attempt {
        Text(String),
        Number(i64),
    }
    Ok(
        Option::<Attempt>::deserialize(deserializer)?.map(|value| match value {
            Attempt::Text(value) => value,
            Attempt::Number(value) => value.to_string(),
        }),
    )
}

fn query_number<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Value {
        Number(u64),
        Text(String),
    }
    Ok(match Option::<Value>::deserialize(deserializer)? {
        Some(Value::Number(number)) => Some(number),
        Some(Value::Text(text)) => text.parse().ok(),
        None => None,
    })
}

fn select(report: &mut Report, query: &ReportQuery) -> io::Result<Vec<Encounter>> {
    select_with_duration(report, query).map(|(selected, _)| selected)
}

fn select_with_duration(
    report: &mut Report,
    query: &ReportQuery,
) -> io::Result<(Vec<Encounter>, f64)> {
    let windows = base_selection(report, query)?;
    let duration = windows.iter().map(|window| window.duration_seconds).sum();
    if query.sc.is_none() && query.fc.is_none() {
        return Ok((windows, duration));
    }
    let start = query.sc.unwrap_or(0).saturating_mul(1000);
    let end = query
        .fc
        .map_or(u64::MAX, |seconds| seconds.saturating_mul(1000));
    let mut clipped = Vec::new();
    let mut offset = 0u64;
    for window in windows {
        let after = offset.saturating_add(window.duration_millis());
        if start < after
            && offset < end
            && let Some(window) = report.clip_encounter(
                window,
                start.saturating_sub(offset)..end.saturating_sub(offset),
            )?
        {
            clipped.push(window);
        }
        offset = after;
    }
    Ok((clipped, duration))
}

fn base_selection(report: &mut Report, query: &ReportQuery) -> io::Result<Vec<Encounter>> {
    if query
        .boss
        .as_deref()
        .is_none_or(|value| value.is_empty() || value == "custom")
    {
        if let (Some(start), Some(end)) = (query.s, query.f) {
            return Ok(report.time_slice(start..end)?.into_iter().collect());
        }
        if !report.has_lockout() {
            return Ok(report.whole_log()?.into_iter().collect());
        }
    }
    let attempt = query.attempt.as_deref().unwrap_or_default();
    let number = attempt.parse::<usize>().ok();
    let mut selected = report
        .metadata
        .encounters
        .iter()
        .filter(|encounter| {
            query
                .boss
                .as_deref()
                .is_none_or(|boss| boss == "all" || boss == encounter.boss_slug)
                && query
                    .mode
                    .as_ref()
                    .is_none_or(|mode| mode.is_empty() || *mode == encounter.difficulty)
                && number.is_none_or(|attempt| attempt == encounter.attempt)
                && (attempt != "kill" || encounter.outcome == "kill")
        })
        .cloned()
        .collect::<Vec<_>>();
    if attempt == "kill" {
        selected = selected.pop().into_iter().collect();
    }
    Ok(selected)
}

async fn report_page(
    State(app): State<App>,
    UrlPath(id): UrlPath<String>,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<ReportQuery>,
) -> Result<Response, Error> {
    blocking(move || {
        let mut report = Report::open_group(&app.root, &id, &app.legacy_groups)?;
        if query.attempt.as_deref() == Some("kill")
            && let [kill] = base_selection(&mut report, &query)?.as_slice()
        {
            let mut path = format!("/reports/{}/{}", path_segment(&id), kill.query());
            if let Some(target) = &query.target {
                path.push_str(&format!("&target={}", path_segment(target)));
            }
            for (key, value) in [("sc", query.sc), ("fc", query.fc)] {
                if let Some(value) = value {
                    path.push_str(&format!("&{key}={value}"));
                }
            }
            return Ok((StatusCode::FOUND, [(header::LOCATION, path)]).into_response());
        }
        let (selected, graph_duration) = select_with_duration(&mut report, &query)?;
        let summary = aggregate(&mut report, &selected)?;
        let specific = query.boss.as_deref().is_some_and(|boss| boss != "all");
        let table = report_view::overview(&mut report, &selected, specific, summary)?;
        let common = report_view::context(&report, &id, &selected, &query, &uri);
        app.render(
            "report_main.html",
            context! { ..common, ..table, ..context! { GRAPH_DURATION => graph_duration } },
        )
        .map(IntoResponse::into_response)
    })
    .await
}

async fn spell_page(
    State(app): State<App>,
    UrlPath((id, view, player)): UrlPath<(String, String, String)>,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<ReportQuery>,
) -> Result<Html<String>, Error> {
    if !matches!(
        view.as_str(),
        "player" | "heal" | "taken" | "healed" | "casts"
    ) {
        return Err(Error(StatusCode::NOT_FOUND, "Unknown report view"));
    }
    blocking(move || {
        let mut report = Report::open_group(&app.root, &id, &app.legacy_groups)?;
        let (selected, graph_duration) = select_with_duration(&mut report, &query)?;
        let common = report_view::context(&report, &id, &selected, &query, &uri);
        if view == "casts" {
            return app.render(
                "timeline.html",
                context! { SOURCE_NAME => &player, ..common },
            );
        }
        let detail = player_view::breakdown(
            &mut report,
            &selected,
            &player,
            &view,
            query.target.as_deref(),
        )?;
        let server = report.id.splitn(4, "--").nth(3).unwrap_or_default();
        let name = detail.get_attr("SOURCE_NAME").unwrap_or_default();
        let name = name.as_str().unwrap_or_default();
        let armory = app.realms.armory_url(server, name);
        let profile = format!("/character?name={}&server={}", path_segment(name), path_segment(server));
        app.render(
            "dmg_done2.html",
            context! { ..common, ..detail, ..context! { GRAPH_DURATION => graph_duration, ARMORY_URL => armory, CHARACTER_URL => profile } },
        )
    })
    .await
}

#[derive(Deserialize)]
struct TimelineQuery {
    boss: String,
    #[serde(deserialize_with = "query_attempt")]
    attempt: Option<String>,
    name: String,
}

async fn casts(
    State(app): State<App>,
    UrlPath(id): UrlPath<String>,
    request: Request,
) -> Result<Json<raidledger::timeline::Timeline>, Error> {
    let query: TimelineQuery = json_or_form(request).await?;
    let attempt = query
        .attempt
        .and_then(|text| text.parse::<isize>().ok())
        .ok_or(Error(StatusCode::BAD_REQUEST, "Invalid encounter attempt"))?;
    blocking(move || {
        let mut report = Report::open_group(&app.root, &id, &app.legacy_groups)?;
        let mut fights = report
            .metadata
            .encounters
            .iter()
            .filter(|fight| fight.boss_slug == query.boss);
        let encounter = if attempt < 0 {
            fights.rev().nth(attempt.unsigned_abs() - 1)
        } else {
            fights.find(|fight| fight.attempt == attempt as usize)
        }
        .ok_or(Error(StatusCode::BAD_REQUEST, "Unknown encounter"))?
        .clone();
        Ok(Json(raidledger::timeline::build(
            &mut report,
            &encounter,
            &query.name,
        )?))
    })
    .await
}

async fn players_classes(
    State(app): State<App>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<std::collections::BTreeMap<String, String>>, Error> {
    blocking(move || {
        let report = Report::open_group(&app.root, &id, &app.legacy_groups)?;
        Ok(Json(
            report
                .metadata
                .players
                .into_iter()
                .map(|(guid, name)| {
                    let class = report
                        .metadata
                        .classes
                        .get(&guid)
                        .cloned()
                        .unwrap_or_else(|| "unknown".into());
                    (name, class)
                })
                .collect(),
        ))
    })
    .await
}

async fn report_section(
    State(app): State<App>,
    UrlPath((id, section)): UrlPath<(String, String)>,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<ReportQuery>,
) -> Result<Html<String>, Error> {
    render_section(app, id, section, None, uri, query).await
}

async fn spell_section(
    State(app): State<App>,
    UrlPath((id, spell)): UrlPath<(String, String)>,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<ReportQuery>,
) -> Result<Html<String>, Error> {
    render_section(app, id, "spell".into(), Some(spell), uri, query).await
}

async fn render_section(
    app: App,
    id: String,
    section: String,
    spell: Option<String>,
    uri: axum::http::Uri,
    query: ReportQuery,
) -> Result<Html<String>, Error> {
    blocking(move || {
        let mut report = Report::open_group(&app.root, &id, &app.legacy_groups)?;
        let selected = select(&mut report, &query)?;
        let common = report_view::context(&report, &id, &selected, &query, &uri);
        let boss = match section.as_str() {
            "valks" => Some("the-lich-king"),
            "lady_spirits" => Some("lady-deathwhisper"),
            "ucm" => Some("sindragosa"),
            "toc_valks" => Some("twin-valkyr"),
            _ => None,
        };
        if let Some(boss) = boss {
            let fights = if query.boss.is_none() {
                report
                    .metadata
                    .encounters
                    .iter()
                    .filter(|fight| fight.boss_slug == boss)
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                selected
                    .into_iter()
                    .filter(|fight| fight.boss_slug == boss)
                    .collect::<Vec<_>>()
            };
            let (template, details) = section_view::special(&mut report, &fights, &section)?;
            return app.render(template, context! { ..common, ..details });
        }
        let (template, details) = match section.as_str() {
            "deaths" => ("deaths.html", section_view::deaths(&mut report, &selected)?),
            "damage" => (
                "damage_target.html",
                section_view::damage(&mut report, &selected)?,
            ),
            "all_auras" => (
                "all_auras.html",
                section_view::auras(&mut report, &selected)?,
            ),
            "consumables" => (
                "consumables.html",
                section_view::consumables(&mut report, &selected)?,
            ),
            "powers" => ("powers.html", section_view::powers(&mut report, &selected)?),
            "entities" => (
                "entities.html",
                section_view::entities(&mut report, &selected)?,
            ),
            "spell" => (
                "spells_page.html",
                section_view::spells(&mut report, &selected, spell.as_deref().unwrap_or_default())?,
            ),
            _ => return Err(Error(StatusCode::NOT_FOUND, "Unknown report section")),
        };
        app.render(template, context! { ..common, ..details })
    })
    .await
}

#[derive(Deserialize)]
struct SlicesQuery {
    boss: String,
}

async fn report_slices(
    State(app): State<App>,
    UrlPath(id): UrlPath<String>,
    Json(query): Json<SlicesQuery>,
) -> Result<Json<Vec<String>>, Error> {
    blocking(move || {
        let report = Report::open_group(&app.root, &id, &app.legacy_groups)?;
        let values = report
            .metadata
            .encounters
            .iter()
            .filter(|encounter| encounter.boss_slug == query.boss)
            .map(|encounter| {
                let seconds = encounter.duration_seconds;
                let state = if encounter.outcome == "kill" {
                    "Kill".into()
                } else {
                    format!("Wipe {}", encounter.attempt + 1)
                };
                format!(
                    "{:02}:{:06.3} | {state}",
                    seconds as u64 / 60,
                    seconds % 60.
                )
            })
            .collect();
        Ok(Json(values))
    })
    .await
}

#[derive(Deserialize)]
struct DpsQuery {
    #[serde(flatten)]
    selection: ReportQuery,
    player_name: Option<String>,
    sec: Option<serde_json::Value>,
}

async fn dps(
    State(app): State<App>,
    UrlPath(id): UrlPath<String>,
    request: Request,
) -> Result<Json<std::collections::BTreeMap<String, f64>>, Error> {
    let query: DpsQuery = json_or_form(request).await?;
    blocking(move || {
        let mut report = Report::open_group(&app.root, &id, &app.legacy_groups)?;
        let selected = select(&mut report, &query.selection)?;
        let interval = query
            .sec
            .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
            .unwrap_or(0);
        Ok(Json(raidledger::metrics::dps(
            &mut report,
            &selected,
            query.player_name.as_deref(),
            interval,
        )?))
    })
    .await
}

#[derive(Deserialize)]
struct CompareQuery {
    class: String,
    target: Option<String>,
}

async fn comparison(
    State(app): State<App>,
    UrlPath(id): UrlPath<String>,
    Query(query): Query<ReportQuery>,
    request: Request,
) -> Result<Json<serde_json::Value>, Error> {
    let data: CompareQuery = json_or_form(request).await?;
    blocking(move || {
        let mut report = Report::open_group(&app.root, &id, &app.legacy_groups)?;
        let selected = select(&mut report, &query)?;
        Ok(Json(raidledger::compare::compare(
            &mut report,
            &selected,
            &data.class,
            data.target.as_deref(),
        )?))
    })
    .await
}

async fn compare_page(
    State(app): State<App>,
    UrlPath(id): UrlPath<String>,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<ReportQuery>,
) -> Result<Html<String>, Error> {
    blocking(move || {
        let mut report = Report::open_group(&app.root, &id, &app.legacy_groups)?;
        let selected = select(&mut report, &query)?;
        app.render(
            "compare.html",
            report_view::context(&report, &id, &selected, &query, &uri),
        )
    })
    .await
}

#[derive(Deserialize)]
struct SpellSearch {
    filter: String,
}

async fn search_spells(
    State(app): State<App>,
    UrlPath(id): UrlPath<String>,
    request: Request,
) -> Result<Json<std::collections::BTreeMap<String, String>>, Error> {
    let data: SpellSearch = json_or_form(request).await?;
    blocking(move || {
        let mut report = Report::open_group(&app.root, &id, &app.legacy_groups)?;
        Ok(Json(raidledger::compare::spell_search(
            &mut report,
            &data.filter,
        )?))
    })
    .await
}

async fn about(
    State(app): State<App>,
    OriginalUri(uri): OriginalUri,
) -> Result<Html<String>, Error> {
    app.render(
        "about.html",
        context! { request => context! { path => uri.path() } },
    )
}

#[derive(Deserialize)]
struct Download {
    format: Option<String>,
}

async fn download(
    State(app): State<App>,
    UrlPath(id): UrlPath<String>,
    Query(query): Query<Download>,
) -> Result<Response, Error> {
    let format = query.format.as_deref().unwrap_or("txt");
    let filename = id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let root = Arc::clone(&app.root);
    let mut report =
        blocking(move || Ok(Report::open_group(&app.root, &id, &app.legacy_groups)?)).await?;
    if query.format.is_none()
        && let Some(path) = report.archive(&root)
    {
        let file = tokio::fs::File::open(path).await?;
        return Ok((
            [
                (header::CONTENT_TYPE, "application/x-7z-compressed".into()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{filename}.7z\""),
                ),
            ],
            Body::from_stream(ReaderStream::new(file)),
        )
            .into_response());
    }
    if format == "json" {
        let summary = blocking(move || {
            let windows = report.metadata.encounters.clone();
            Ok(aggregate(&mut report, &windows)?)
        })
        .await?;
        return Ok((
            [(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}.json\""),
            )],
            Json(summary),
        )
            .into_response());
    }
    if format != "txt" {
        return Err(Error(
            StatusCode::BAD_REQUEST,
            "Supported download formats: txt, json",
        ));
    }
    let (write, read) = tokio::io::duplex(128 * 1024);
    let pipe = SyncIoBridge::new(write);
    tokio::task::spawn_blocking(move || {
        if let Err(error) = report.copy_log(pipe)
            && error.kind() != io::ErrorKind::BrokenPipe
        {
            eprintln!("Report download failed: {error}");
        }
    });
    Ok((
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}.txt\""),
            ),
        ],
        Body::from_stream(ReaderStream::new(read)),
    )
        .into_response())
}

fn app_env(key: &str, old: &str) -> Option<std::ffi::OsString> {
    env::var_os(key).or_else(|| env::var_os(old))
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "app": "RaidLedger", "local_only": true, "auto_import_version": 1 }))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(app_env("RAIDLEDGER_ROOT", "UWU_ROOT").unwrap_or_else(|| ".".into()));
    match env::args().nth(1).as_deref() {
        Some("import") => {
            let [log, year, server] = env::args()
                .skip(2)
                .collect::<Vec<_>>()
                .try_into()
                .map_err(|_| "usage: raidledger import <combat-log.txt> <year> <server>")?;
            let ids = import_capture(
                &root,
                [Ok(BufReader::new(File::open(log)?))],
                year.parse()?,
                &server,
                None,
                None,
            )?;
            for id in ids {
                println!("{id}");
            }
        }
        Some("split-raids") => {
            let id = env::args()
                .nth(2)
                .ok_or("usage: raidledger split-raids <report-id>")?;
            for id in raidledger::reports::split_report(&root, &id)? {
                println!("{id}");
            }
        }
        Some("reindex-appearance") => {
            let mut ids = env::args().skip(2).peekable();
            if ids.peek().is_none() {
                return Err("usage: raidledger reindex-appearance <report-id>...".into());
            }
            for id in ids {
                let changed = raidledger::reports::reindex_appearance(&root, &id)?;
                println!("{id}: {}", if changed { "updated" } else { "unchanged" });
            }
        }
        Some("reindex-difficulty") => {
            let mut ids = env::args().skip(2).collect::<Vec<_>>();
            if ids.is_empty() {
                ids = list_ids(&root)?;
            }
            for id in ids {
                if raidledger::reports::reindex_difficulty(&root, &id)? {
                    println!("{id}: updated");
                }
            }
        }
        Some("merge") => {
            let ids = env::args().skip(2).collect::<Vec<_>>();
            println!("{}", raidledger::reports::merge_reports(&root, &ids)?);
        }
        Some("summary") => {
            let id = env::args()
                .nth(2)
                .ok_or("usage: raidledger summary <report-id>")?;
            let mut report = Report::open_group(&root, &id, &LegacyGroups::load(&root)?)?;
            let encounters = report.metadata.encounters.clone();
            serde_json::to_writer(io::stdout().lock(), &aggregate(&mut report, &encounters)?)?;
            println!();
        }
        Some(_) => return Err("unknown command".into()),
        None => serve(root).await?,
    }
    Ok(())
}

async fn serve(root: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("LogsDir"))?;
    let assets =
        PathBuf::from(app_env("RAIDLEDGER_ASSETS", "UWU_ASSETS").unwrap_or_else(|| ".".into()));
    let mut templates = Environment::new();
    templates.set_loader(path_loader(assets.join("templates")));
    templates.add_filter("urlencode", path_segment);
    report_view::configure_templates(&mut templates);
    let root = root.canonicalize()?;
    let address = app_env("RAIDLEDGER_BIND", "UWU_BIND")
        .map(|address| address.to_string_lossy().into_owned())
        .unwrap_or_else(|| "127.0.0.1:5000".into());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    let config = auto_import::Config::load(&root)?;
    let legacy_groups = LegacyGroups::load(&root)?;
    let realms = character_view::Realms::load(&root)?;
    let access = report_access::Access::load(&root, &legacy_groups)?;
    let viewer = config.as_ref().and_then(auto_import::Config::viewer);
    let worker_status = Arc::new(std::sync::Mutex::new(serde_json::json!({
        "state": if config.is_some() { "starting" } else { "disabled" }
    })));
    if let Some(config) = config {
        templates.add_global("AUTO_IMPORT", minijinja::Value::from(true));
        auto_import::start(root.clone(), config, Arc::clone(&worker_status));
    }
    let app = App {
        realms: Arc::new(realms),
        access: Arc::new(access),
        legacy_groups: Arc::new(legacy_groups),
        root: Arc::new(root),
        templates: Arc::new(templates),
        uploads: Arc::new(upload::Uploads::default()),
        rankings: Arc::default(),
        worker_status,
        viewer,
    };
    let router = Router::new()
        .route("/", get(library))
        .route("/about", get(about))
        .route("/support", get(about))
        .route(
            "/character",
            get(character_view::page).post(character_view::character_post),
        )
        .route(
            "/character/{server}/{name}/{spec}",
            get(character_view::character_get),
        )
        .route("/rank", axum::routing::post(character_view::rank))
        .route("/gear/{server}/{name}", get(character_view::gear))
        .route(
            "/gear-assets/{server}/{kind}/{file}",
            get(character_view::gear_asset),
        )
        .route("/missing", axum::routing::put(character_view::missing))
        .route("/armory/{server}/{name}", get(character_view::armory))
        .route("/ladder", get(character_view::ladder))
        .route("/pw_validate", axum::routing::post(report_access::login))
        .route("/top", get(ranking_view::page).post(ranking_view::top))
        .route(
            "/pve_stats",
            get(ranking_view::page).post(ranking_view::stats),
        )
        .route(
            "/top_stats",
            get(ranking_view::page).post(ranking_view::stats),
        )
        .route("/top_points", axum::routing::post(ranking_view::points))
        .route(
            "/top_speedrun",
            axum::routing::post(ranking_view::speedruns),
        )
        .route("/logs_list", get(library).post(library_filter))
        .route("/raid_calendar", get(library))
        .route("/local-health", get(health))
        .route("/local-status", get(auto_import::status))
        .route("/upload", get(upload::form).post(upload::receive))
        .route("/upload_progress", get(upload::progress))
        .route(
            "/reports",
            get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/logs_list")]) }),
        )
        .route(
            "/reports/",
            get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/logs_list")]) }),
        )
        .route("/reports/{id}/", get(report_page))
        .route(
            "/reports/{id}",
            get(
                |UrlPath(id): UrlPath<String>, OriginalUri(uri): OriginalUri| async move {
                    let query = uri
                        .query()
                        .map_or(String::new(), |query| format!("?{query}"));
                    Redirect::permanent(&format!("/reports/{}/{query}", path_segment(&id)))
                },
            ),
        )
        .route("/reports/{id}/{view}/{player}/", get(spell_page))
        .route("/reports/{id}/{section}/", get(report_section))
        .route("/reports/{id}/spell/{spell}/", get(spell_section))
        .route("/reports/{id}/download", get(download))
        .route("/reports/{id}/compare/", get(compare_page).post(comparison))
        .route(
            "/reports/{id}/spellsearch",
            axum::routing::post(search_spells),
        )
        .route(
            "/reports/{id}/report_slices/",
            axum::routing::post(report_slices),
        )
        .route("/reports/{id}/get_dps", axum::routing::post(dps))
        .route("/reports/{id}/casts/", axum::routing::post(casts))
        .route(
            "/reports/{id}/players_classes/",
            axum::routing::post(players_classes),
        )
        .nest_service("/static", ServeDir::new(assets.join(Path::new("static"))))
        .route_service(
            "/favicon.ico",
            ServeFile::new(assets.join("static/favicon.ico")),
        )
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            auto_import::viewer_redirect,
        ))
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            report_access::report,
        ))
        .with_state(app);
    eprintln!("Listening on {address}");
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

fn path_segment(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            escaped.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(&mut escaped, "%{byte:02X}");
        }
    }
    escaped
}
