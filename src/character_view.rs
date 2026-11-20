use crate::{App, Error, blocking, path_segment, ranking_view};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use minijinja::context;
use raidledger::rankings::{self, Rank, RankQuery};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Realms {
    #[serde(default)]
    servers: BTreeMap<String, Realm>,
    pub ladder_websocket: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Realm {
    rank_api: Option<String>,
    gear_api: Option<String>,
    #[serde(default)]
    gear_format: GearFormat,
    gear_assets: Option<String>,
    gear_missing: Option<String>,
    armory: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GearFormat {
    #[default]
    Snapshots,
    Azerothcore,
}

impl Realms {
    pub fn load(root: &Path) -> io::Result<Self> {
        let path = std::env::var_os("RAIDLEDGER_REALMS").map(std::path::PathBuf::from);
        let bytes = match std::fs::read(path.as_deref().unwrap_or(&root.join("realms.json"))) {
            Ok(bytes) => bytes,
            Err(error) if path.is_none() && error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error),
        };
        let config: Self = serde_json::from_slice(&bytes)?;
        for realm in config.servers.values() {
            if matches!(realm.gear_format, GearFormat::Azerothcore)
                && realm
                    .gear_api
                    .as_ref()
                    .is_some_and(|url| url.contains(['?', '#']))
            {
                return Err(io::Error::other(
                    "AzerothCore gear API must be a collection URL",
                ));
            }
            for url in [
                &realm.rank_api,
                &realm.gear_api,
                &realm.gear_assets,
                &realm.gear_missing,
                &realm.armory,
            ]
            .into_iter()
            .flatten()
            {
                if !valid_url(url, &["http://", "https://"]) {
                    return Err(io::Error::other(
                        "invalid HTTP URL in RaidLedger realm configuration",
                    ));
                }
            }
        }
        if config
            .ladder_websocket
            .as_ref()
            .is_some_and(|url| !valid_url(url, &["ws://", "wss://"]))
        {
            return Err(io::Error::other("invalid ladder WebSocket URL"));
        }
        Ok(config)
    }

    fn get(&self, server: &str) -> Option<&Realm> {
        self.servers.get(&server.to_lowercase()).or_else(|| {
            self.servers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(server))
                .map(|(_, value)| value)
        })
    }

    pub fn armory_url(&self, server: &str, player: &str) -> Option<String> {
        Some(expand(self.get(server)?.armory.as_deref()?, server, player))
    }

    fn armory_servers(&self) -> Vec<&str> {
        self.servers
            .iter()
            .filter(|(_, realm)| realm.armory.is_some())
            .map(|(server, _)| server.as_str())
            .collect()
    }
}

fn valid_url(url: &str, schemes: &[&str]) -> bool {
    schemes.iter().any(|prefix| {
        url.strip_prefix(prefix)
            .is_some_and(|host| !host.is_empty() && !host.starts_with('/'))
    }) && url.bytes().all(|byte| byte.is_ascii_graphic())
        && !url.contains('\\')
}

fn expand(url: &str, server: &str, name: &str) -> String {
    url.replace("{server}", &path_segment(server))
        .replace("{name}", &path_segment(name))
}

#[derive(Deserialize)]
pub struct CharacterQuery {
    name: String,
    server: String,
    #[serde(default, alias = "spec_i")]
    spec: Option<Value>,
}

pub async fn page(State(app): State<App>) -> Result<Html<String>, Error> {
    blocking(move || {
        let mut servers = rankings::servers(&app.root, &app.legacy_groups)?;
        servers.extend(app.realms.servers.keys().cloned());
        app.render("character.html", context! {
            request => context! { path => "/character" }, REPORT_NAME => "Character", SERVERS => servers,
            ARMORY_SERVERS => app.realms.armory_servers(),
        })
    }).await
}

pub async fn ladder(State(app): State<App>) -> Result<Html<String>, Error> {
    blocking(move || {
        let mut servers = rankings::servers(&app.root, &app.legacy_groups)?;
        servers.extend(app.realms.servers.keys().cloned());
        app.render("ladder.html", context! {
            request => context! { path => "/ladder" }, REPORT_NAME => "Ladder", SERVERS => servers,
            LADDER_WEBSOCKET => app.realms.ladder_websocket, ARMORY_SERVERS => app.realms.armory_servers(),
        })
    }).await
}

pub async fn character_post(
    State(app): State<App>,
    Json(query): Json<CharacterQuery>,
) -> Result<Json<Value>, Error> {
    character(app, query).await
}

pub async fn character_get(
    State(app): State<App>,
    UrlPath((server, name, spec)): UrlPath<(String, String, String)>,
) -> Result<Json<Value>, Error> {
    character(
        app,
        CharacterQuery {
            name,
            server,
            spec: Some(Value::String(spec)),
        },
    )
    .await
}

async fn character(app: App, query: CharacterQuery) -> Result<Json<Value>, Error> {
    let name = query.name.trim();
    if name.chars().count() < 2 || name.len() > 120 || name.chars().any(char::is_control) {
        return Err(Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid character name",
        ));
    }
    let name = name.to_owned();
    let spec = query.spec.and_then(|value| {
        value
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .or_else(|| value.as_str()?.parse().ok())
    });
    blocking(move || {
        ranking_view::refresh(&app, &query.server)?
            .character(&query.server, &name, spec)
            .map(Json)
            .ok_or(Error(
                StatusCode::NOT_FOUND,
                "Character has no indexed raid data",
            ))
    })
    .await
}

pub async fn rank(
    State(app): State<App>,
    Json(query): Json<RankQuery>,
) -> Result<Json<Value>, Error> {
    ranking_view::validate(query.fight.valid())?;
    blocking(move || {
        let realm = app.realms.get(&query.fight.server);
        let value = if let Some(url) = realm.and_then(|realm| realm.rank_api.as_deref()) {
            let url = expand(url, &query.fight.server, "");
            let payload = serde_json::to_vec(&query).map_err(io::Error::other)?;
            let (status, data) = provider("POST", &url, Some(&payload))?;
            if !status.is_success() {
                return Err(Error(
                    StatusCode::BAD_GATEWAY,
                    "Rank provider returned an error",
                ));
            }
            let ranks: BTreeMap<String, Rank> = serde_json::from_slice(&data)
                .map_err(|_| Error(StatusCode::BAD_GATEWAY, "Invalid rank response"))?;
            serde_json::to_value(ranks)
        } else {
            serde_json::to_value(
                ranking_view::refresh(&app, &query.fight.server)?.raid_rank(&query),
            )
        };
        Ok(Json(value.map_err(io::Error::other)?))
    })
    .await
}

pub async fn armory(
    State(app): State<App>,
    UrlPath((server, name)): UrlPath<(String, String)>,
) -> Result<Redirect, Error> {
    app.realms
        .armory_url(&server, &name)
        .map(|url| Redirect::temporary(&url))
        .ok_or(Error(
            StatusCode::NOT_FOUND,
            "No Armory configured for this realm",
        ))
}

pub async fn gear(
    State(app): State<App>,
    UrlPath((server, name)): UrlPath<(String, String)>,
) -> Result<Json<Value>, Error> {
    let realm = app
        .realms
        .get(&server)
        .filter(|realm| realm.gear_api.is_some())
        .ok_or(Error(
            StatusCode::NOT_FOUND,
            "No gear provider configured for this realm",
        ))?;
    let url = expand(realm.gear_api.as_deref().unwrap(), &server, &name);
    if matches!(realm.gear_format, GearFormat::Azerothcore) {
        return blocking(move || {
            let mut profile = azerothcore_gear(url.trim_end_matches('/'), &name)?;
            for item in &mut profile.equipment {
                item.icon = item
                    .icon
                    .take()
                    .filter(|icon| resource_id(icon))
                    .or_else(|| {
                        app.item_icons
                            .get(&item.display_id)
                            .filter(|icon| resource_id(icon))
                            .cloned()
                    });
            }
            Ok(Json(serde_json::json!({"live_equipment": profile})))
        })
        .await;
    }
    blocking(move || {
        let (status, data) = provider("GET", &url, None)?;
        require_asset(status)?;
        let value: Value = serde_json::from_slice(&data)
            .map_err(|_| Error(StatusCode::BAD_GATEWAY, "Invalid gear response"))?;
        if !value.is_object() {
            return Err(Error(StatusCode::BAD_GATEWAY, "Invalid gear response"));
        }
        Ok(Json(value))
    })
    .await
}

#[derive(Deserialize)]
struct CharacterLookup {
    data: Vec<CharacterIdentity>,
}

#[derive(Deserialize)]
struct CharacterIdentity {
    guid: u32,
    name: String,
}

#[derive(Deserialize, Serialize)]
struct LiveEquipment {
    name: String,
    level: u8,
    spec: Option<String>,
    equipment: Vec<LiveItem>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveItem {
    slot: u8,
    item_entry: u32,
    #[serde(default, skip_serializing)]
    display_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    icon: Option<String>,
    name: String,
    name_ru: Option<String>,
    item_level: u16,
    quality: u8,
    #[serde(default)]
    armor: u32,
    #[serde(default)]
    block: u32,
    #[serde(default)]
    delay_ms: u32,
    #[serde(default)]
    damage: Vec<LiveDamage>,
    #[serde(default)]
    stats: Vec<LiveStat>,
    #[serde(default)]
    resistances: BTreeMap<String, i32>,
    #[serde(default)]
    spells: Vec<LiveEffect>,
    #[serde(default)]
    enchantments: Vec<LiveEffect>,
    #[serde(default)]
    random_property_enchantments: Vec<LiveEffect>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveEffect {
    name: String,
    name_ru: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct LiveStat {
    #[serde(rename = "type")]
    kind: u8,
    value: i32,
}

#[derive(Deserialize, Serialize)]
struct LiveDamage {
    min: f64,
    max: f64,
    #[serde(rename = "type", default)]
    school: u8,
}

pub fn load_item_icons(assets: &Path) -> BTreeMap<u32, String> {
    let path = assets.join("static/item-icons/item-display-icons.json");
    let result = std::fs::File::open(&path).and_then(|file| {
        serde_json::from_reader(io::BufReader::new(file)).map_err(io::Error::other)
    });
    match result {
        Ok(icons) => icons,
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                eprintln!("Cannot load item icons from {}: {error}", path.display());
            }
            BTreeMap::new()
        }
    }
}

fn azerothcore_gear(collection: &str, name: &str) -> Result<LiveEquipment, Error> {
    if name.is_empty() || name.chars().count() > 24 || !name.chars().all(char::is_alphabetic) {
        return Err(Error(StatusCode::BAD_REQUEST, "Invalid character name"));
    }
    let query = format!("{collection}?names={}&limit=2", path_segment(name));
    let lookup: CharacterLookup = gear_json(&query)?;
    let identity = lookup
        .data
        .into_iter()
        .find(|identity| identity.name.to_lowercase() == name.to_lowercase())
        .ok_or(Error(
            StatusCode::NOT_FOUND,
            "Character not found on this realm",
        ))?;
    let mut profile: LiveEquipment = gear_json(&format!("{collection}/{}", identity.guid))?;
    if profile.name != identity.name {
        return Err(Error(
            StatusCode::BAD_GATEWAY,
            "Unexpected character profile",
        ));
    }
    profile
        .equipment
        .retain(|item| item.item_entry != 0 && item.slot < 19);
    profile.equipment.sort_by_key(|item| item.slot);
    Ok(profile)
}

fn gear_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, Error> {
    let (status, data) = provider("GET", url, None)?;
    require_asset(status)?;
    serde_json::from_slice(&data)
        .map_err(|_| Error(StatusCode::BAD_GATEWAY, "Invalid gear response"))
}

#[cfg(test)]
mod gear_tests {
    use super::LiveEquipment;
    use serde_json::{json, to_value};

    #[test]
    fn core_equipment_projection_excludes_private_character_fields() {
        let profile: LiveEquipment = serde_json::from_value(json!({
            "name": "Sbol", "level": 60, "spec": "Survival", "accountId": 1, "money": 24,
            "equipment": [{"slot": 0, "itemEntry": 16939, "name": "Helm",
                "itemLevel": 76, "quality": 4, "instanceGuid": 12345, "displayId": 34367,
                "stats": [{"type": 3, "value": 27}],
                "enchantments": [{"name": "Hit rating", "nameRu": "Рейтинг меткости", "id": 3754}]}]
        }))
        .unwrap();
        let public = to_value(profile).unwrap();
        assert_eq!(public["equipment"][0]["itemEntry"], 16939);
        assert_eq!(
            public["equipment"][0]["stats"],
            json!([{"type": 3, "value": 27}])
        );
        assert_eq!(
            public["equipment"][0]["enchantments"][0]["nameRu"],
            "Рейтинг меткости"
        );
        assert!(public["equipment"][0].get("displayId").is_none());
        assert!(public.get("accountId").is_none());
        assert!(public.get("money").is_none());
        assert!(public["equipment"][0].get("instanceGuid").is_none());
    }
}

pub async fn gear_asset(
    State(app): State<App>,
    UrlPath((server, kind, file)): UrlPath<(String, String, String)>,
) -> Result<Response, Error> {
    let (ext, mime) = match kind.as_str() {
        "item" | "enchant" => (".json", "application/json"),
        "icons" => (".jpg", "image/jpeg"),
        _ => return Err(Error(StatusCode::NOT_FOUND, "Unknown gear resource")),
    };
    if !file.strip_suffix(ext).is_some_and(resource_id) {
        return Err(Error(StatusCode::BAD_REQUEST, "Invalid gear resource"));
    }
    let url = app
        .realms
        .get(&server)
        .and_then(|realm| realm.gear_assets.as_deref())
        .ok_or(Error(
            StatusCode::NOT_FOUND,
            "No gear assets configured for this realm",
        ))?;
    let url = format!(
        "{}/{kind}/{file}",
        expand(url, &server, "").trim_end_matches('/')
    );
    blocking(move || {
        let (status, data) = provider("GET", &url, None)?;
        require_asset(status)?;
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, mime.parse().unwrap());
        headers.insert(
            header::CACHE_CONTROL,
            "public, max-age=900".parse().unwrap(),
        );
        headers.insert("x-content-type-options", "nosniff".parse().unwrap());
        Ok((headers, Bytes::from(data)).into_response())
    })
    .await
}

#[derive(Deserialize)]
pub struct Missing {
    server: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    id: String,
}

pub async fn missing(
    State(app): State<App>,
    Query(data): Query<Missing>,
) -> Result<StatusCode, Error> {
    if !matches!(data.kind.as_str(), "item" | "enchant" | "icon") || !resource_id(&data.id) {
        return Err(Error(StatusCode::BAD_REQUEST, "Invalid gear resource"));
    }
    let configured = || {
        let mut realms = app
            .realms
            .servers
            .iter()
            .filter(|(_, realm)| realm.gear_missing.is_some());
        let (name, _) = realms.next()?;
        realms.next().is_none().then_some(name.as_str())
    };
    let server = data
        .server
        .as_deref()
        .or_else(configured)
        .ok_or(Error(StatusCode::NOT_FOUND, "No gear cache for this realm"))?;
    let url = app
        .realms
        .get(server)
        .and_then(|realm| realm.gear_missing.as_deref())
        .ok_or(Error(StatusCode::NOT_FOUND, "No gear cache for this realm"))?;
    let url = expand(url, server, "");
    let separator = if url.contains('?') { '&' } else { '?' };
    let url = format!("{url}{separator}type={}&id={}", data.kind, data.id);
    blocking(move || provider("PUT", &url, None).map(|(status, _)| status)).await
}

fn resource_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}

fn require_asset(status: StatusCode) -> Result<(), Error> {
    if status.is_success() {
        Ok(())
    } else if status == StatusCode::NOT_FOUND {
        Err(Error(status, "Gear resource not found"))
    } else {
        Err(Error(
            StatusCode::BAD_GATEWAY,
            "Gear provider returned an error",
        ))
    }
}

fn provider(method: &str, url: &str, body: Option<&[u8]>) -> Result<(StatusCode, Vec<u8>), Error> {
    const MAX_RESPONSE: u64 = 4 * 1024 * 1024;
    let upstream_error = |_| Error(StatusCode::BAD_GATEWAY, "Configured provider unavailable");
    let mut curl = Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
    curl.args([
        "-q",
        "--silent",
        "--globoff",
        "--proto",
        "=http,https",
        "--connect-timeout",
        "3",
        "--max-time",
        "10",
        "--max-filesize",
        "4194304",
        "--compressed",
        "--write-out",
        "%{http_code}",
        "--request",
        method,
    ]);
    if body.is_some() {
        curl.args([
            "--header",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
        ]);
    }
    let mut child = curl
        .args(["--url", url])
        .stdin(if body.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(upstream_error)?;
    let written = body.map(|data| child.stdin.take().unwrap().write_all(data));
    let mut data = Vec::new();
    let read = child
        .stdout
        .take()
        .unwrap()
        .take(MAX_RESPONSE + 4)
        .read_to_end(&mut data);
    if read.is_err() || data.len() as u64 > MAX_RESPONSE + 3 {
        let _ = child.kill();
    }
    let status = child.wait().map_err(upstream_error)?;
    if read.is_err()
        || data.len() as u64 > MAX_RESPONSE + 3
        || written.is_some_and(|result| result.is_err())
        || !status.success()
    {
        return Err(Error(
            StatusCode::BAD_GATEWAY,
            "Configured provider unavailable",
        ));
    }
    let offset = data
        .len()
        .checked_sub(3)
        .ok_or(Error(StatusCode::BAD_GATEWAY, "Invalid provider response"))?;
    let status = StatusCode::from_bytes(&data[offset..])
        .map_err(|_| Error(StatusCode::BAD_GATEWAY, "Invalid provider response"))?;
    data.truncate(offset);
    Ok((status, data))
}
