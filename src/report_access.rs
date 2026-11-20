use crate::App;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, FromRequestParts, RawPathParams, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use minijinja::context;
use raidledger::reports::LegacyGroups;
use std::collections::{BTreeSet, HashMap};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const COOKIE: &str = "raidledger-private";
type RequestCounts = HashMap<(IpAddr, &'static str), (Instant, u32)>;

pub struct Access {
    ids: BTreeSet<String>,
    login: Option<Login>,
    rate: Mutex<RequestCounts>,
    reports_per_minute: u32,
    downloads_per_minute: u32,
}

struct Login {
    password_hash: String,
    cookie_token: String,
}

impl Access {
    pub fn load(root: &Path, groups: &LegacyGroups) -> io::Result<Self> {
        let list = match std::fs::read_to_string(root.join("__private.txt")) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        let ids = list
            .lines()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .flat_map(|id| groups.related_ids(id))
            .map(str::to_owned)
            .collect();
        let secret = |name| {
            std::env::var(name).ok().filter(|value| {
                value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        };
        let login = match (
            secret("RAIDLEDGER_PRIVATE_PASSWORD_SHA256"),
            secret("RAIDLEDGER_PRIVATE_SESSION_TOKEN"),
        ) {
            (Some(password_hash), Some(cookie_token))
                if !password_hash.eq_ignore_ascii_case(&cookie_token) =>
            {
                Some(Login {
                    password_hash: password_hash.to_lowercase(),
                    cookie_token,
                })
            }
            _ => None,
        };
        Ok(Self {
            ids,
            login,
            rate: Mutex::default(),
            reports_per_minute: read_rate("RAIDLEDGER_REPORTS_PER_MINUTE")?,
            downloads_per_minute: read_rate("RAIDLEDGER_DOWNLOADS_PER_MINUTE")?,
        })
    }

    pub fn public(&self, id: &str) -> bool {
        !self.ids.contains(id)
    }

    fn authenticated(&self, headers: &HeaderMap) -> bool {
        self.login.as_ref().is_some_and(|login| {
            headers
                .get_all(header::COOKIE)
                .iter()
                .filter_map(|value| value.to_str().ok())
                .flat_map(|value| value.split(';'))
                .filter_map(|cookie| cookie.trim().split_once('='))
                .any(|(key, value)| {
                    key == COOKIE && secret_eq(value.as_bytes(), login.cookie_token.as_bytes())
                })
        })
    }

    fn allow(&self, ip: IpAddr, scope: &'static str, limit: u32) -> bool {
        if limit == 0 {
            return true;
        }
        let now = Instant::now();
        let mut peers = self
            .rate
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if peers.len() >= 4096 {
            peers.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(60));
            if peers.len() >= 4096 && !peers.contains_key(&(ip, scope)) {
                return false;
            }
        }
        let (start, count) = peers.entry((ip, scope)).or_insert((now, 0));
        if now.duration_since(*start) >= Duration::from_secs(60) {
            (*start, *count) = (now, 0);
        }
        if *count >= limit {
            return false;
        }
        *count += 1;
        true
    }
}

fn read_rate(name: &str) -> io::Result<u32> {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|_| io::Error::other(format!("Invalid rate limit in {name}"))),
        Err(_) => Ok(0),
    }
}

fn secret_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0, |difference, (a, b)| difference | (a ^ b))
            == 0
}

fn limited() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, "60")],
        "Try again in a minute",
    )
        .into_response()
}

pub async fn login(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    value: Bytes,
) -> Response {
    let Some(login) = &app.access.login else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !app.access.allow(peer.ip(), "login", 5) {
        return limited();
    }
    if !secret_eq(&value, login.password_hash.as_bytes()) {
        return (StatusCode::UNAUTHORIZED, "Incorrect password").into_response();
    }
    (
        StatusCode::OK,
        [(
            header::SET_COOKIE,
            format!(
                "{COOKIE}={}; Path=/reports; HttpOnly; Secure; SameSite=Strict",
                login.cookie_token
            ),
        )],
    )
        .into_response()
}

pub async fn report(State(app): State<App>, request: Request, next: Next) -> Response {
    let (mut parts, body) = request.into_parts();
    let path = parts.uri.path().to_owned();
    if let Ok(params) = RawPathParams::from_request_parts(&mut parts, &()).await
        && let Some((_, id)) = params.iter().find(|(key, _)| *key == "id")
        && path.starts_with("/reports/")
    {
        let (scope, limit) = if path.ends_with("/download") {
            ("download", app.access.downloads_per_minute)
        } else {
            ("report", app.access.reports_per_minute)
        };
        if let Some(ConnectInfo(peer)) = parts.extensions.get::<ConnectInfo<SocketAddr>>()
            && !app.access.allow(peer.ip(), scope, limit)
        {
            return limited();
        }
        if !app.access.public(id) && !app.access.authenticated(&parts.headers) {
            if parts.method != Method::GET || scope == "download" {
                return StatusCode::FORBIDDEN.into_response();
            }
            return match app.render(
                "protected.html",
                context! {
                    request => context! { path }, PASSWORD_CONFIGURED => app.access.login.is_some()
                },
            ) {
                Ok(html) => (
                    StatusCode::UNAUTHORIZED,
                    [(header::CACHE_CONTROL, "no-store")],
                    html,
                )
                    .into_response(),
                Err(error) => error.into_response(),
            };
        }
        if !app.access.public(id) {
            let mut response = next.run(Request::from_parts(parts, body)).await;
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
            return response;
        }
    }
    next.run(Request::from_parts(parts, body)).await
}
