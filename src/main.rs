mod upload;

use axum::body::Body;
use axum::extract::{OriginalUri, Path as UrlPath, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::{Json, Router, routing::get};
use minijinja::{Environment, context, path_loader};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::io::{ReaderStream, SyncIoBridge};
use tower_http::services::ServeDir;
use uwu_logs::combat::CATALOG;
use uwu_logs::metrics::{aggregate, player_spells};
use uwu_logs::reports::{Encounter, LegacyGroups, Report, import_log, list_ids};

#[derive(Clone)]
struct App {
    root: Arc<PathBuf>,
    templates: Arc<Environment<'static>>,
    uploads: Arc<upload::Uploads>,
    legacy_groups: Arc<LegacyGroups>,
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
    players: usize,
    raid_label: String,
    fights: String,
    progress: String,
    progress_note: &'static str,
}

fn library_row(report: Report) -> LibraryRow {
    let fields = report.id.splitn(4, "--").collect::<Vec<_>>();
    let year = report.id[..2].parse::<u32>().unwrap_or(0) + 2000;
    let date = format!("{year}{}", fields[0].get(2..).unwrap_or(""));
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
        .filter(|raid| fights.iter().any(|boss| raid.bosses.contains(*boss)))
        .collect::<Vec<_>>();
    let raid_label = if raids.is_empty() {
        "Рейдовый отчёт".to_owned()
    } else {
        raids
            .iter()
            .map(|raid| raid.name.as_str())
            .collect::<Vec<_>>()
            .join(" / ")
    };
    let progress = if raids.len() == 1 {
        format!("≥{}/{}", kills.len(), raids[0].bosses.len())
    } else {
        "—".to_owned()
    };
    LibraryRow {
        author: fields[2].into(),
        server: fields[3].into(),
        date: date.clone(),
        time: time.clone(),
        datetime: format!("{date}T{time}"),
        year,
        id: report.id.clone(),
        players: report.metadata.players.len(),
        raid_label,
        fights: fights.into_iter().collect::<Vec<_>>().join(" · "),
        progress,
        progress_note: "Количество убийств, подтверждённых журналом",
    }
}

#[derive(Default, Deserialize)]
struct LibraryQuery {
    server: Option<String>,
}

async fn library(
    State(app): State<App>,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<LibraryQuery>,
) -> Result<Html<String>, Error> {
    let path = uri.path().to_owned();
    blocking(move || {
        let mut rows = Vec::new();
        for id in list_ids(&app.root)?
            .into_iter()
            .filter(|id| !app.legacy_groups.is_fragment(id))
            .take(100)
        {
            if query
                .server
                .as_ref()
                .is_some_and(|server| !id.ends_with(&format!("--{server}")))
            {
                continue;
            }
            match Report::open_group(&app.root, &id, &app.legacy_groups) {
                Ok(report) => rows.push(library_row(report)),
                Err(error) => eprintln!("Cannot open {id}: {error}"),
            }
        }
        app.render(
            "home.html",
            context! { request => context! { path }, LOCAL_REPORTS => rows },
        )
    })
    .await
}

#[derive(Default, Deserialize)]
struct ReportQuery {
    boss: Option<String>,
    mode: Option<String>,
    attempt: Option<usize>,
}

fn select(encounters: &[Encounter], query: &ReportQuery) -> Vec<Encounter> {
    encounters
        .iter()
        .filter(|encounter| {
            query
                .boss
                .as_deref()
                .is_none_or(|boss| boss == "all" || boss == encounter.boss_slug)
                && query
                    .mode
                    .as_ref()
                    .is_none_or(|mode| *mode == encounter.difficulty)
                && query
                    .attempt
                    .is_none_or(|attempt| attempt == encounter.attempt)
        })
        .cloned()
        .collect()
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
        let selected = select(&report.metadata.encounters, &query);
        let summary = aggregate(&mut report, &selected)?;
        let mut players = summary.players.into_values().collect::<Vec<_>>();
        players.sort_by_key(|player| std::cmp::Reverse(player.rates.totals.damage));
        let encounter_links = report.metadata.encounters.iter().map(|encounter| context! {
            boss => &encounter.boss, outcome => &encounter.outcome, difficulty => &encounter.difficulty,
            duration => encounter.duration_seconds, query => encounter.query()
        }).collect::<Vec<_>>();
        app.render("rust_report.html", context! {
            request => context! { path }, REPORT_ID => &id, REPORT_NAME => &id,
            encounters => encounter_links, players, duration => summary.duration_seconds, query => uri.query(),
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
        let windows = select(&report.metadata.encounters, &query).into_iter().map(|encounter| encounter.start..encounter.end).collect::<Vec<_>>();
        let mut spells = player_spells(&mut report, &windows, &player)?.into_iter().collect::<Vec<_>>();
        spells.sort_by_key(|(_, spell)| std::cmp::Reverse(match view.as_str() {
            "heal" => spell.healing, "taken" => spell.taken, "casts" => spell.casts as i64, _ => spell.damage,
        }));
        app.render("rust_player.html", context! { request => context! { path }, REPORT_ID => &id, REPORT_NAME => &id, player, spells })
    }).await
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
    let mut report =
        blocking(move || Ok(Report::open_group(&app.root, &id, &app.legacy_groups)?)).await?;
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

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "app": "UwU Logs Rust", "local_only": true }))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env::var_os("UWU_ROOT").unwrap_or_else(|| ".".into()));
    match env::args().nth(1).as_deref() {
        Some("import") => {
            let [log, year, server] = env::args()
                .skip(2)
                .collect::<Vec<_>>()
                .try_into()
                .map_err(|_| "usage: uwu-logs import <combat-log.txt> <year> <server>")?;
            let id = import_log(
                &root,
                BufReader::new(File::open(log)?),
                year.parse()?,
                &server,
            )?;
            println!("{id}");
        }
        Some("summary") => {
            let id = env::args()
                .nth(2)
                .ok_or("usage: uwu-logs summary <report-id>")?;
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
    let assets = PathBuf::from(env::var_os("UWU_ASSETS").unwrap_or_else(|| ".".into()));
    let mut templates = Environment::new();
    templates.set_loader(path_loader(assets.join("templates")));
    templates.add_filter("urlencode", path_segment);
    let app = App {
        legacy_groups: Arc::new(LegacyGroups::load(&root)?),
        root: Arc::new(root),
        templates: Arc::new(templates),
        uploads: Arc::new(upload::Uploads::default()),
    };
    let router = Router::new()
        .route("/", get(library))
        .route("/logs_list", get(library))
        .route("/raid_calendar", get(library))
        .route("/local-health", get(health))
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
        .route("/reports/{id}/download", get(download))
        .nest_service("/static", ServeDir::new(assets.join(Path::new("static"))))
        .with_state(app);
    let address = env::var("UWU_BIND").unwrap_or_else(|_| "127.0.0.1:5000".into());
    let listener = tokio::net::TcpListener::bind(&address).await?;
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
