use crate::{App, Error, blocking};
use axum::Json;
use axum::extract::{OriginalUri, State};
use axum::http::StatusCode;
use axum::response::Html;
use minijinja::context;
use raidledger::rankings::{self, FightQuery, Index, PointsQuery, SpeedrunQuery, TopQuery};
use serde_json::{Value, to_value};

pub async fn page(
    State(app): State<App>,
    OriginalUri(uri): OriginalUri,
) -> Result<Html<String>, Error> {
    blocking(move || {
        let servers = rankings::servers(&app.root, &app.legacy_groups)?;
        let top = uri.path() == "/top";
        let default = if let Some(server) = servers.first() {
            let index = refresh(&app, server)?;
            index.default_fight(server).cloned()
        } else {
            None
        };
        let title = if top { "Rankings" } else { "PvE Statistics" };
        app.render(
            if top { "top.html" } else { "pve_stats.html" },
            context! {
                request => context! { path => uri.path() }, REPORT_NAME => title,
                SERVERS => servers, RAID_BOSSES => rankings::raid_options(),
                DEFAULT_FIGHT => default, SPECS_BASE => rankings::specs(),
            },
        )
    })
    .await
}

pub(crate) fn refresh<'a>(
    app: &'a App,
    server: &str,
) -> Result<std::sync::MutexGuard<'a, Index>, Error> {
    let mut index = app.rankings.lock().map_err(|_| {
        Error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Rankings cache unavailable",
        )
    })?;
    index.refresh_public(&app.root, &app.legacy_groups, server, |id| {
        app.access.public(id)
    })?;
    Ok(index)
}

fn result(value: impl serde::Serialize) -> Result<Json<Value>, Error> {
    to_value(value)
        .map(Json)
        .map_err(|error| Error::from(std::io::Error::other(error)))
}

pub(crate) fn validate(valid: bool) -> Result<(), Error> {
    if valid {
        Ok(())
    } else {
        Err(Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid ranking filters",
        ))
    }
}

pub async fn top(
    State(app): State<App>,
    Json(query): Json<TopQuery>,
) -> Result<Json<Value>, Error> {
    validate(query.valid())?;
    blocking(move || result(refresh(&app, &query.fight.server)?.top(&query))).await
}

pub async fn stats(
    State(app): State<App>,
    Json(query): Json<FightQuery>,
) -> Result<Json<Value>, Error> {
    validate(query.valid())?;
    blocking(move || result(refresh(&app, &query.server)?.stats(&query))).await
}

pub async fn points(
    State(app): State<App>,
    Json(query): Json<PointsQuery>,
) -> Result<Json<Value>, Error> {
    validate(query.valid())?;
    blocking(move || result(refresh(&app, &query.server)?.points(&query))).await
}

pub async fn speedruns(
    State(app): State<App>,
    Json(query): Json<SpeedrunQuery>,
) -> Result<Json<Value>, Error> {
    validate(query.valid())?;
    blocking(move || result(refresh(&app, &query.server)?.speedruns(&query))).await
}
