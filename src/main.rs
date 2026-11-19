mod auto_import;
mod upload;

use axum::body::Body;
use axum::extract::{OriginalUri, Path as UrlPath, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::{Json, Router, routing::get};
use minijinja::{Environment, context, path_loader};
use raidledger::combat::CATALOG;
use raidledger::metrics::{aggregate, player_spells};
use raidledger::reports::{Encounter, LegacyGroups, Report, import_capture, list_ids};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::io::{ReaderStream, SyncIoBridge};
use tower_http::services::ServeDir;

#[derive(Clone)]
struct App {
    root: Arc<PathBuf>,
    templates: Arc<Environment<'static>>,
    uploads: Arc<upload::Uploads>,
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
}

fn library_row(report: Report, snapshots: Option<&auto_import::SavedProgress>) -> LibraryRow {
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
    LibraryRow {
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
            .take(100)
        {
            if let Some((_, realm)) = id.rsplit_once("--") {
                realms.insert(realm.to_owned());
            }
            if query
                .server
                .as_ref()
                .is_some_and(|server| !server.is_empty() && !id.ends_with(&format!("--{server}")))
            {
                continue;
            }
            match Report::open_group(&app.root, &id, &app.legacy_groups) {
                Ok(report) => rows.push(library_row(report, snapshots.as_ref())),
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
    let selected = format!("{year}-{:02}-", month + 1);
    let reports = reports
        .into_iter()
        .filter(|report| report.date.starts_with(&selected))
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
    attempt: Option<String>,
    #[serde(default, deserialize_with = "query_number")]
    s: Option<u64>,
    #[serde(default, deserialize_with = "query_number")]
    f: Option<u64>,
    #[serde(default, deserialize_with = "query_number")]
    sc: Option<u64>,
    #[serde(default, deserialize_with = "query_number")]
    fc: Option<u64>,
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
    if query
        .boss
        .as_deref()
        .is_none_or(|value| value.is_empty() || value == "custom")
        && let (Some(start), Some(end)) = (query.s, query.f)
    {
        return Ok(report.time_slice(start..end)?.into_iter().collect());
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
    if (query.sc.is_some() || query.fc.is_some()) && selected.len() == 1 {
        let window = &selected[0];
        let start = window.start_second.saturating_add(query.sc.unwrap_or(0));
        let end = query.fc.map_or(window.end_second, |offset| {
            window.start_second.saturating_add(offset)
        });
        let end = end.min(window.end_second);
        if let Some(mut selection) = report.time_slice(start..end)? {
            selection.boss.clone_from(&selected[0].boss);
            selection.boss_slug.clone_from(&selected[0].boss_slug);
            selection.difficulty.clone_from(&selected[0].difficulty);
            selection.outcome.clone_from(&selected[0].outcome);
            selection.attempt = selected[0].attempt;
            selected = vec![selection];
        }
    }
    Ok(selected)
}

async fn report_page(
    State(app): State<App>,
    UrlPath(id): UrlPath<String>,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<ReportQuery>,
) -> Result<Html<String>, Error> {
    let path = uri.path().to_owned();
    blocking(move || {
        let mut report = Report::open_group(&app.root, &id, &app.legacy_groups)?;
        let selected = select(&mut report, &query)?;
        let mechanics = [
            ("the-lich-king", "valks", "Валь’киры Короля-лича"),
            ("lady-deathwhisper", "lady_spirits", "Мстительные тени"),
            ("sindragosa", "ucm", "Нестабильность Синдрагосы"),
            ("twin-valkyr", "toc_valks", "Щиты близнецов-валь’кир"),
        ].into_iter().filter(|(boss, _, _)| selected.iter().any(|fight| fight.boss_slug == *boss))
            .map(|(_, route, title)| context! { route, title }).collect::<Vec<_>>();

        let summary = aggregate(&mut report, &selected)?;
        let mut players = summary.players.into_values().collect::<Vec<_>>();
        players.sort_by_key(|player| std::cmp::Reverse(player.rates.totals.damage));
        let encounter_links = report.metadata.encounters.iter().map(|encounter| context! {
            boss => &encounter.boss, outcome => &encounter.outcome, difficulty => &encounter.difficulty,
            duration => encounter.duration_seconds, query => encounter.query()
        }).collect::<Vec<_>>();
        app.render("rust_report.html", context! {
            request => context! { path }, REPORT_ID => &id, REPORT_NAME => &id,
            encounters => encounter_links, players, mechanics, totals => summary.totals, duration => summary.duration_seconds, query => uri.query().map(|query| format!("?{query}")).unwrap_or_default(),
        })
    }).await
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
    let path = uri.path().to_owned();
    blocking(move || {
        let mut report = Report::open_group(&app.root, &id, &app.legacy_groups)?;
        if view == "casts" {
            let encounters = report.metadata.encounters.iter().map(|encounter| context! {
                boss => &encounter.boss, outcome => &encounter.outcome, difficulty => &encounter.difficulty,
                duration => encounter.duration_seconds, query => encounter.query()
            }).collect::<Vec<_>>();
            return app.render("timeline.html", context! {
                request => context! { path }, REPORT_ID => &id, REPORT_NAME => &id,
                SOURCE_NAME => &player, QUERY => uri.query().map(|query| format!("?{query}")).unwrap_or_default(),
                RUST_TIMELINE => true, encounters,
            });
        }

        let windows = select(&mut report, &query)?.into_iter().map(|encounter| encounter.start..encounter.end).collect::<Vec<_>>();
        let received_only = view == "healed";
        let mut spells = player_spells(&mut report, &windows, &player, received_only)?.into_iter().collect::<Vec<_>>();
        spells.sort_by_key(|(_, spell)| std::cmp::Reverse(match view.as_str() {
            "heal" => spell.healing, "healed" => spell.received_healing, "taken" => spell.taken, _ => spell.damage,
        }));
        app.render("rust_player.html", context! { request => context! { path }, REPORT_ID => &id, REPORT_NAME => &id, player, spells,
            received_only, query => uri.query().map(|query| format!("?{query}")).unwrap_or_default() })
    }).await
}

#[derive(Deserialize)]
struct TimelineQuery {
    boss: String,
    attempt: usize,
    name: String,
}

async fn casts(
    State(app): State<App>,
    UrlPath(id): UrlPath<String>,
    Json(query): Json<TimelineQuery>,
) -> Result<Json<raidledger::timeline::Timeline>, Error> {
    blocking(move || {
        let mut report = Report::open_group(&app.root, &id, &app.legacy_groups)?;
        let encounter = report
            .metadata
            .encounters
            .iter()
            .find(|encounter| {
                encounter.boss_slug == query.boss && encounter.attempt == query.attempt
            })
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
    let path = uri.path().to_owned();
    blocking(move || {
        use raidledger::analysis::{self, View};
        let mut report = Report::open_group(&app.root, &id, &app.legacy_groups)?;
        let selected = select(&mut report, &query)?;
        let query = uri
            .query()
            .map(|query| format!("?{query}"))
            .unwrap_or_default();
        let mechanic = match section.as_str() {
            "valks" => Some(("the-lich-king", "Валь’киры Короля-лича")),
            "lady_spirits" => Some(("lady-deathwhisper", "Мстительные тени")),
            "ucm" => Some(("sindragosa", "Нестабильность Синдрагосы")),
            "toc_valks" => Some(("twin-valkyr", "Щиты близнецов-валь’кир")),
            _ => None,
        };
        if let Some((boss, heading)) = mechanic {
            let relevant = report.metadata.encounters.iter().filter(|fight| fight.boss_slug == boss)
                .map(|fight| context! { query => fight.query(), label => format!("Попытка {}", fight.attempt + 1) }).collect::<Vec<_>>();
            let tables = raidledger::mechanics::analyze(&mut report, &selected.into_iter()
                .filter(|fight| fight.boss_slug == boss).collect::<Vec<_>>(), &section)?;
            return app.render("raid_mechanics.html", context! {
                request => context! { path }, REPORT_ID => &id, REPORT_NAME => &id, query, heading, tables, relevant,
            });
        }
        let view =
            match section.as_str() {
                "deaths" => return app.render(
                    "rust_deaths.html",
                    context! {
                        request => context! { path }, REPORT_ID => &id, REPORT_NAME => &id, query,
                        deaths => analysis::deaths(&mut report, &selected)?
                    },
                ),
                "damage" => View::Targets,
                "all_auras" => View::Auras,
                "consumables" => View::Consumables,
                "powers" => View::Resources,
                "entities" => View::Units,
                "spell" => View::Spell(spell.as_deref().unwrap_or_default()),
                _ => return Err(Error(StatusCode::NOT_FOUND, "Unknown report section")),
            };
        let (heading, label, metric) = view.labels();
        app.render(
            "rust_section.html",
            context! {
                request => context! { path }, REPORT_ID => &id, REPORT_NAME => &id, query,
                heading, label, metric, rows => analysis::inspect(&mut report, &selected, view)?
            },
        )
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
    Json(query): Json<DpsQuery>,
) -> Result<Json<std::collections::BTreeMap<String, f64>>, Error> {
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
    Json(data): Json<CompareQuery>,
) -> Result<Json<serde_json::Value>, Error> {
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
) -> Result<Html<String>, Error> {
    blocking(move || {
        Report::open_group(&app.root, &id, &app.legacy_groups)?;
        app.render(
            "compare.html",
            context! {
                request => context! { path => uri.path() }, REPORT_ID => &id, REPORT_NAME => &id,
                query => uri.query().map(|query| format!("?{query}")).unwrap_or_default(),
            },
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
    body: axum::body::Bytes,
) -> Result<Json<std::collections::BTreeMap<String, String>>, Error> {
    let data = serde_json::from_slice::<SpellSearch>(&body)
        .map_err(|_| Error(StatusCode::BAD_REQUEST, "Invalid spell filter"))?;
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
    Json(serde_json::json!({ "app": "RaidLedger", "local_only": true }))
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
    templates.add_global("RUST_VIEW", minijinja::Value::from(true));
    let root = root.canonicalize()?;
    let address = app_env("RAIDLEDGER_BIND", "UWU_BIND")
        .map(|address| address.to_string_lossy().into_owned())
        .unwrap_or_else(|| "127.0.0.1:5000".into());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    let config = auto_import::Config::load(&root)?;
    let viewer = config.as_ref().and_then(auto_import::Config::viewer);
    let worker_status = Arc::new(std::sync::Mutex::new(serde_json::json!({
        "state": if config.is_some() { "starting" } else { "disabled" }
    })));
    if let Some(config) = config {
        templates.add_global("AUTO_IMPORT", minijinja::Value::from(true));
        auto_import::start(root.clone(), config, Arc::clone(&worker_status));
    }
    let app = App {
        legacy_groups: Arc::new(LegacyGroups::load(&root)?),
        root: Arc::new(root),
        templates: Arc::new(templates),
        uploads: Arc::new(upload::Uploads::default()),
        worker_status,
        viewer,
    };
    let router = Router::new()
        .route("/", get(library))
        .route("/about", get(about))
        .route("/support", get(about))
        .route("/logs_list", get(library).post(library_filter))
        .route("/raid_calendar", get(library))
        .route("/local-health", get(health))
        .route("/local-status", get(auto_import::status))
        .route("/upload", get(upload::form).post(upload::receive))
        .route("/upload_progress", get(upload::progress))
        .route("/reports/{id}/", get(report_page))
        .route(
            "/reports/{id}",
            get(|UrlPath(id): UrlPath<String>| async move {
                Redirect::temporary(&format!("/reports/{}/", path_segment(&id)))
            }),
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
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            auto_import::viewer_redirect,
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
