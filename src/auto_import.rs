use crate::{App, upload};
use axum::Json;
use axum::extract::{Request, State};
use axum::http::{Method, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use raidledger::lockouts::Manifest;
use raidledger::reports::import_capture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Deserialize)]
#[serde(default)]
pub(crate) struct Config {
    source: PathBuf,
    raw_archive: PathBuf,
    timezone: String,
    server: String,
    poll_seconds: u64,
    settle_seconds: u64,
    realms: BTreeMap<String, String>,
    remote_export_url: String,
    reports_url: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            source: "../Logs/WoWCombatLog.txt".into(),
            raw_archive: "../Logs/RaidArchive".into(),
            timezone: "UTC".into(),
            server: "Unknown".into(),
            poll_seconds: 10,
            settle_seconds: 8,
            realms: BTreeMap::new(),
            remote_export_url: String::new(),
            reports_url: String::new(),
        }
    }
}

impl Config {
    pub(crate) fn load(root: &Path) -> io::Result<Option<Self>> {
        let config = match fs::read(root.join("auto-config.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound && cfg!(windows) => {
                Self::default()
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if [&config.remote_export_url, &config.reports_url]
            .into_iter()
            .any(|url| !url.is_empty() && parse_http(url).is_none())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "report server must be an HTTP or HTTPS URL",
            ));
        }
        Ok(Some(config))
    }

    pub(crate) fn viewer(&self) -> Option<String> {
        [&self.reports_url, &self.remote_export_url]
            .into_iter()
            .find(|url| !url.is_empty())
            .map(|url| url.trim_end_matches('/').to_owned())
    }
}

pub(crate) type Status = Arc<Mutex<Value>>;

pub(crate) async fn status(State(app): State<App>) -> Json<Value> {
    Json(app.worker_status.lock().unwrap().clone())
}

pub(crate) async fn viewer_redirect(
    State(app): State<App>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(base) = &app.viewer
        && matches!(*request.method(), Method::GET | Method::HEAD)
    {
        let path = request.uri().path();
        let target = match path {
            "/" => Some("/logs_list"),
            "/logs_list" => Some(path),
            _ => path.starts_with("/reports/").then_some(path),
        };
        let host = request
            .uri()
            .authority()
            .map(|host| host.as_str())
            .or_else(|| {
                request
                    .headers()
                    .get(header::HOST)
                    .and_then(|value| value.to_str().ok())
            });
        if let Some(target) = target
            && let Some(destination) = parse_http(base)
            && !host.is_some_and(|host| {
                destination
                    .authority()
                    .is_some_and(|authority| host.eq_ignore_ascii_case(authority.as_str()))
            })
        {
            let query = request
                .uri()
                .query()
                .map_or(String::new(), |query| format!("?{query}"));
            return Redirect::temporary(&format!("{base}{target}{query}")).into_response();
        }
    }
    next.run(request).await
}

fn parse_http(value: &str) -> Option<Uri> {
    let uri = value.parse::<Uri>().ok()?;
    matches!(uri.scheme_str(), Some("http" | "https")).then_some(())?;
    uri.authority()?;
    if uri.authority()?.as_str().contains('@') || uri.query().is_some() {
        return None;
    }
    Some(uri)
}

pub(crate) fn start(root: PathBuf, config: Config, status: Status) {
    std::thread::spawn(move || {
        let source = root.join(&config.source);
        let Some(log_dir) = source.parent() else {
            return;
        };
        let mut worker = Worker {
            queue: log_dir.join(".raidledger-captures"),
            archive: root.join(&config.raw_archive),
            game: log_dir.parent().unwrap_or(log_dir).to_owned(),
            root,
            source,
            config,
            status,
            observed: None,
            next_export: Instant::now(),
        };
        loop {
            if let Err(error) = worker.tick(game_is_open) {
                eprintln!("RaidLedger automatic import: {error}");
                worker.set(json!({"state":"error", "error":error.to_string()}));
            }
            std::thread::sleep(Duration::from_secs(
                worker.config.poll_seconds.clamp(1, 300),
            ));
        }
    });
}

struct Worker {
    root: PathBuf,
    game: PathBuf,
    source: PathBuf,
    queue: PathBuf,
    archive: PathBuf,
    config: Config,
    status: Status,
    observed: Option<((u64, SystemTime), Instant)>,
    next_export: Instant,
}

#[derive(Deserialize, Serialize)]
struct Capture {
    year: u32,
    server: String,
    timezone: String,
}

impl Worker {
    fn set(&self, state: Value) {
        *self.status.lock().unwrap() = state;
    }

    fn tick(&mut self, game_is_open: impl Fn() -> io::Result<bool>) -> io::Result<()> {
        if game_is_open()? {
            self.observed = None;
            self.set(json!({"state":"deferred_until_client_exit","game_running":true}));
            return Ok(());
        }
        if self.queue.is_dir() {
            for entry in sorted_entries(&self.queue)? {
                if entry.is_dir()
                    && entry.join("WoWCombatLog.txt").is_file()
                    && let Err(error) = self.finish_capture(&entry)
                {
                    eprintln!(
                        "RaidLedger capture {} retained for retry: {error}",
                        entry.display()
                    );
                    self.set(json!({"state":"error", "error":error.to_string()}));
                }
            }
        }
        if !self.config.remote_export_url.is_empty()
            && Instant::now() >= self.next_export
            && let Err(error) = self.export_pending()
        {
            self.next_export = Instant::now() + Duration::from_secs(60);
            eprintln!("RaidLedger export pending: {error}");
            self.set(json!({"state":"ready","export_state":"retry_pending","export_error":error.to_string()}));
        }
        let stat = match fs::symlink_metadata(&self.source) {
            Ok(stat) if stat.file_type().is_file() && stat.len() > 0 => stat,
            Ok(_) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                self.observed = None;
                if self.status.lock().unwrap()["state"] != "ready" {
                    self.set(json!({"state":"waiting_for_log","game_running":false}));
                }
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let signature = (stat.len(), stat.modified()?);
        let now = Instant::now();
        if self.observed.is_none_or(|(old, _)| old != signature) {
            self.observed = Some((signature, now));
        }
        if now.duration_since(self.observed.unwrap().1)
            < Duration::from_secs(self.config.settle_seconds)
        {
            self.set(json!({"state":"waiting_for_file_flush"}));
            return Ok(());
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(4);
        }
        let mut file = match options.open(&self.source) {
            Ok(file) => file,
            Err(e)
                if e.kind() == io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(32) =>
            {
                self.set(json!({"state":"rotation_waiting_for_file_unlock"}));
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let current = file.metadata()?;
        file.seek(SeekFrom::End(-1))?;
        let mut tail = [0];
        file.read_exact(&mut tail)?;
        if tail != *b"\n" || (current.len(), current.modified()?) != signature || game_is_open()? {
            self.set(json!({"state":"waiting_for_file_flush"}));
            return Ok(());
        }
        let id = format!("capture-{}", nonce()?);
        let pending = self.queue.join(&id);
        fs::create_dir_all(&self.queue)?;
        fs::create_dir(&pending)?;
        let mut capture_year = upload::year_at(signature.1);
        file.seek(SeekFrom::Start(0))?;
        if let Some(first) = first_stamp(BufReader::new(&file))?
            && let Ok(elapsed) = signature.1.duration_since(UNIX_EPOCH)
        {
            let first_millis = first.millis_of_year(capture_year);
            let mut end_millis = elapsed.as_secs() * 1000;
            for year in 1970..capture_year {
                end_millis = end_millis.saturating_sub(
                    (1..=12)
                        .map(|month| u64::from(raidledger::combat::days_in_month(year, month)))
                        .sum::<u64>()
                        * 86_400_000,
                );
            }
            if first_millis > end_millis.saturating_add(183 * 86_400_000) {
                capture_year -= 1;
            }
        }
        let config = Capture {
            year: capture_year,
            server: self.server_name(),
            timezone: self.config.timezone.clone(),
        };
        write_json(&pending.join("capture.json"), &config)?;
        write_json(
            &pending.join("uwu-lockouts.json"),
            &saved_manifest(&self.game, &self.config.realms)?,
        )?;
        fs::rename(&self.source, pending.join("WoWCombatLog.txt"))?;
        drop(file);
        self.observed = None;
        self.finish_capture(&pending)
    }

    fn server_name(&self) -> String {
        let Ok(settings) = fs::read_to_string(self.game.join("WTF/Config.wtf")) else {
            return self.config.server.clone();
        };
        let realm = settings.lines().find_map(|line| {
            let mut words = line.split_whitespace();
            (words.next()?.eq_ignore_ascii_case("set")
                && words.next()?.eq_ignore_ascii_case("realmlist"))
            .then(|| words.next())
            .flatten()
            .map(|name| name.trim_matches('"'))
        });
        realm
            .and_then(|realm| {
                self.config
                    .realms
                    .get(realm.split(':').next().unwrap_or(realm))
            })
            .cloned()
            .unwrap_or_else(|| self.config.server.clone())
    }

    fn finish_capture(&self, pending: &Path) -> io::Result<()> {
        let id = pending
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| raidledger::reports::valid_capture(value))
            .ok_or_else(|| io::Error::other("invalid pending capture directory"))?;
        let raw = pending.join("WoWCombatLog.txt");
        let manifest_file = pending.join("uwu-lockouts.json");
        let capture: Capture = read_json(&pending.join("capture.json"))?;
        let manifest = Manifest::parse(&fs::read(&manifest_file)?)?;
        self.set(json!({"state":"importing"}));
        let report = import_capture(
            &self.root,
            [Ok(BufReader::new(File::open(&raw)?))],
            capture.year,
            &capture.server,
            Some((&manifest, &capture.timezone)),
            Some(id),
        )?;
        fs::create_dir_all(&self.archive)?;
        let destination = self.archive.join(format!("WoWCombatLog-{id}.txt"));
        if !destination.is_file() {
            let temporary = destination.with_extension("copying");
            let mut file = File::create(&temporary)?;
            io::copy(&mut File::open(&raw)?, &mut file)?;
            file.sync_all()?;
            drop(file);
            fs::rename(temporary, &destination)?;
        }
        if !self.config.remote_export_url.is_empty() {
            let queue = self.root.join("uploads/raidledger-export").join(id);
            fs::create_dir_all(&queue)?;
            let zip = queue.join("capture.zip");
            if !zip.is_file() {
                let staging = queue.join("capture.partial");
                if staging.is_file() {
                    fs::remove_file(&staging)?;
                }
                let sevenzip = crate::upload::sevenzip();
                let result = command(sevenzip)
                    .current_dir(pending)
                    .args(["a", "-tzip", "-mx=1", "--"])
                    .arg(&staging)
                    .args(["WoWCombatLog.txt", "uwu-lockouts.json"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .output()?;
                if !result.status.success() {
                    return Err(io::Error::other(format!(
                        "7-Zip failed: {}",
                        String::from_utf8_lossy(&result.stderr).trim()
                    )));
                }
                File::open(&staging)?.sync_all()?;
                write_json(&queue.join("capture.json"), &capture)?;
                fs::rename(staging, &zip)?;
            }
        }
        fs::remove_file(&raw)?;
        fs::remove_file(manifest_file)?;
        fs::remove_file(pending.join("capture.json"))?;
        fs::remove_dir(pending)?;
        self.set(json!({"state":"ready","last_reports":[report],"last_rotated":destination}));
        Ok(())
    }

    fn export_pending(&self) -> io::Result<()> {
        let directory = self.root.join("uploads/raidledger-export");
        if !directory.is_dir() {
            return Ok(());
        }
        let base = self.config.remote_export_url.trim_end_matches('/');
        for entry in sorted_entries(&directory)? {
            let zip = entry.join("capture.zip");
            if !zip.is_file() || entry.join("receipt.json").is_file() {
                continue;
            }
            let capture: Capture = read_json(&entry.join("capture.json"))?;
            let id = entry.file_name().unwrap().to_string_lossy();
            self.set(json!({"state":"ready","export_state":"sending"}));
            let ids = send_archive(base, &zip, &capture, &id)?;
            write_json(&entry.join("receipt.json"), &ids)?;
            self.set(json!({"state":"ready","export_state":"complete","export_reports":ids}));
            return Ok(());
        }
        Ok(())
    }
}

fn first_stamp(mut file: impl io::BufRead) -> io::Result<Option<raidledger::combat::Stamp>> {
    let mut row = String::new();
    for _ in 0..20 {
        row.clear();
        if file.read_line(&mut row)? == 0 {
            break;
        }
        let stamp = row
            .trim_start_matches('\u{feff}')
            .split_once("  ")
            .or_else(|| row.split_once(','))
            .and_then(|(stamp, _)| raidledger::combat::Stamp::parse(stamp));
        if stamp.is_some() {
            return Ok(stamp);
        }
    }
    Ok(None)
}

fn sorted_entries(path: &Path) -> io::Result<Vec<PathBuf>> {
    let mut entries = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    entries.sort();
    Ok(entries)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    serde_json::from_reader(File::open(path)?).map_err(io::Error::other)
}

fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let temporary = path.with_extension("pending");
    let mut file = File::create(&temporary)?;
    serde_json::to_writer(&mut file, value).map_err(io::Error::other)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, path)
}

fn nonce() -> io::Result<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .map_err(io::Error::other)
}

fn command(name: impl Into<OsString>) -> Command {
    let command = Command::new(name.into());
    #[cfg(windows)]
    let command = {
        use std::os::windows::process::CommandExt;
        let mut command = command;
        command.creation_flags(0x08000000);
        command
    };
    command
}

#[cfg(windows)]
fn game_is_open() -> io::Result<bool> {
    let output = command("tasklist.exe")
        .args(["/FO", "CSV", "/NH"])
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut processes = stdout
        .lines()
        .filter_map(|line| {
            line.strip_prefix('"')?
                .split_once(r#"",""#)
                .map(|(name, _)| name)
        })
        .peekable();
    if !output.status.success() || processes.peek().is_none() {
        return Err(io::Error::other(
            "could not determine whether WoW is running",
        ));
    }
    Ok(processes.any(|name| {
        name.eq_ignore_ascii_case("wow.exe") || name.eq_ignore_ascii_case("wow-64.exe")
    }))
}

#[cfg(target_os = "linux")]
fn game_is_open() -> io::Result<bool> {
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        if !entry
            .file_name()
            .as_encoded_bytes()
            .iter()
            .all(u8::is_ascii_digit)
        {
            continue;
        }
        if let Ok(name) = fs::read_to_string(entry.path().join("comm"))
            && matches!(
                name.trim().to_ascii_lowercase().as_str(),
                "wow.exe" | "wow-64.exe" | "world of warcraft"
            )
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(all(not(windows), not(target_os = "linux")))]
fn game_is_open() -> io::Result<bool> {
    let output = command("ps").args(["-A", "-o", "comm="]).output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            "could not determine whether WoW is running",
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| Path::new(line.trim()).file_name()?.to_str())
        .any(|name| {
            matches!(
                name.to_ascii_lowercase().as_str(),
                "wow.exe" | "wow-64.exe" | "world of warcraft"
            )
        }))
}

fn hex(value: &str) -> Option<String> {
    if value.len() > 300 || !value.len().is_multiple_of(2) {
        return None;
    }
    let bytes = (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(value.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

fn saved_manifest(game: &Path, realms: &BTreeMap<String, String>) -> io::Result<Value> {
    let accounts = game.join("WTF/Account");
    let mut records = Vec::new();
    if accounts.is_dir() {
        for account in fs::read_dir(accounts)? {
            let file = account?.path().join("SavedVariables/RaidAutoLog.lua");
            if !file.is_file() {
                continue;
            }
            let mut input = String::new();
            File::open(file)?
                .take(2_000_001)
                .read_to_string(&mut input)?;
            if input.len() > 2_000_000 {
                continue;
            }
            records.extend(input.split('"').filter_map(|row| {
                let [time, realm, player, instance, id, high, difficulty, reset, session] =
                    row.strip_prefix("RAL2|")?.split('|').collect::<Vec<_>>().try_into().ok()?;
                let realm = hex(realm)?;
                let server = realms.get(realm.split(':').next().unwrap_or(&realm)).cloned().unwrap_or(realm);
                Some(json!({"time":time.parse::<u64>().ok()?, "server":server, "player":hex(player)?,
                    "instance":hex(instance)?, "id":hex(id)?, "high":hex(high)?, "difficulty":difficulty.parse::<u32>().ok()?,
                    "reset":reset.parse::<u64>().ok()?, "session":session.parse::<u64>().ok()?}))
            }));
        }
    }
    records.sort_by_key(|value| std::cmp::Reverse(value["time"].as_u64().unwrap_or(0)));
    records.truncate(2000);
    Ok(json!({"version":1,"records":records}))
}

fn send_archive(
    base: &str,
    archive: &Path,
    capture: &Capture,
    id: &str,
) -> io::Result<Vec<String>> {
    let route = format!("{base}/upload");
    let progress = format!("{base}/upload_progress");
    let status = curl(&progress, &[], None)?;
    if serde_json::from_slice::<Value>(&status)
        .ok()
        .is_some_and(|value| value["done"].as_i64() == Some(0))
    {
        return Err(io::Error::other("destination is processing another upload"));
    }
    let upload_id = nonce()?.to_string();
    let length = archive.metadata()?.len();
    if length == 0 || length > (256 * 1024 * 32_768u64) {
        return Err(io::Error::other("invalid export archive size"));
    }
    let mut file = BufReader::new(File::open(archive)?);
    let mut data = vec![0; 256 * 1024];
    let mut chunks = 0;
    let mut left = length;
    while left > 0 {
        let count = left.min(data.len() as u64) as usize;
        file.read_exact(&mut data[..count])?;
        curl(
            &route,
            &[
                &format!("X-Upload-ID: {upload_id}"),
                &format!("X-Chunk: {chunks}"),
                "Content-Type: application/octet-stream",
            ],
            Some(&data[..count]),
        )?;
        left -= count as u64;
        chunks += 1;
    }
    let meta = serde_json::to_vec(&json!({"filename":"capture.zip","server":capture.server,
        "timezone":capture.timezone,"chunks":chunks,"year":capture.year,"capture":id}))
    .map_err(io::Error::other)?;
    curl(&route, &["Content-Type: application/json"], Some(&meta))?;
    let cutoff = Instant::now() + Duration::from_secs(600);
    while Instant::now() < cutoff {
        let bytes = curl(&progress, &[], None)?;
        let status: Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        match status["done"].as_i64() {
            Some(0) => std::thread::sleep(Duration::from_secs(1)),
            Some(1) => {
                let reports = status["slices"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(id, value)| {
                        raidledger::reports::valid_id(id) && value["done"].as_i64() == Some(1)
                    })
                    .map(|(id, _)| id.to_owned())
                    .collect::<Vec<_>>();
                if reports.is_empty() {
                    return Err(io::Error::other("destination returned no reports"));
                }
                for report in &reports {
                    let url = format!("{base}/reports/{}/", crate::path_segment(report));
                    let page = curl(&url, &[], None)?;
                    if !String::from_utf8_lossy(&page).contains("id=\"report-main\"") {
                        return Err(io::Error::other("destination report cannot be opened"));
                    }
                }
                return Ok(reports);
            }
            _ => {
                return Err(io::Error::other(
                    status["status"]
                        .as_str()
                        .unwrap_or("destination import failed")
                        .to_owned(),
                ));
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "destination import is still pending",
    ))
}

fn curl(url: &str, headers: &[&str], data: Option<&[u8]>) -> io::Result<Vec<u8>> {
    let mut client = command(if cfg!(windows) { "curl.exe" } else { "curl" });
    client.args([
        "--disable",
        "--fail",
        "--silent",
        "--show-error",
        "--max-time",
        "45",
        "--proto",
        "=http,https",
        "--url",
        url,
    ]);
    for header in headers {
        client.args(["--header", header]);
    }
    client
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if data.is_some() {
        client.args(["--data-binary", "@-"]).stdin(Stdio::piped());
    }
    let mut child = client.spawn()?;
    if let Some(data) = data {
        child.stdin.take().unwrap().write_all(data)?;
    }
    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_game_imports_one_capture_and_retains_raw_bytes() {
        let fixture = std::env::temp_dir().join(format!("raidledger-worker-{}", nonce().unwrap()));
        let source = fixture.join("game/Logs/WoWCombatLog.txt");
        let raw_archive = fixture.join("raw");
        let capture_queue = fixture.join("game/Logs/.raidledger-captures");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        let raw = b"11/19 18:30:00.000,SWING_DAMAGE,0x00000000000007DB,TestHunter,0xF130003C9C000042,Sartura,1,Melee,0x1,312,0,1,0,0,0,0,nil,nil\n";
        fs::write(&source, raw).unwrap();
        let mut worker = Worker {
            root: fixture.join("app"),
            game: fixture.join("game"),
            source: source.clone(),
            queue: capture_queue,
            archive: raw_archive.clone(),
            config: Config {
                settle_seconds: 0,
                ..Config::default()
            },
            status: Arc::new(Mutex::new(json!({}))),
            observed: None,
            next_export: Instant::now(),
        };
        worker.tick(|| Ok(true)).unwrap();
        assert_eq!(
            *worker.status.lock().unwrap(),
            json!({"state":"deferred_until_client_exit","game_running":true})
        );
        assert_eq!(fs::read(&source).unwrap(), raw);
        worker.tick(|| Ok(false)).unwrap();
        assert!(!source.exists());
        let reports = raidledger::reports::list_ids(&worker.root).unwrap();
        assert_eq!(reports.len(), 1);
        let backup = sorted_entries(&raw_archive).unwrap();
        assert_eq!(backup.len(), 1);
        assert_eq!(fs::read(&backup[0]).unwrap(), raw);
        worker.tick(|| Ok(false)).unwrap();
        assert_eq!(
            raidledger::reports::list_ids(&worker.root).unwrap(),
            reports
        );
        fs::remove_dir_all(fixture).unwrap();
    }

    #[test]
    fn saved_raid_lockout_uses_the_configured_realm() {
        let root = std::env::temp_dir().join(format!("raidledger-addon-{}", nonce().unwrap()));
        let path = root.join("WTF/Account/TEST/SavedVariables/RaidAutoLog.lua");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, r#"RaidAutoLogDB = { "RAL2|100|6162633a3130|54657374|4f6e797869612773204c616972|3737|30|1|200|99" }"#).unwrap();
        let value =
            saved_manifest(&root, &BTreeMap::from([("abc".into(), "stable".into())])).unwrap();
        fs::remove_dir_all(root).unwrap();
        assert_eq!(value["records"][0]["server"], "stable");
        assert_eq!(value["records"][0]["player"], "Test");
        assert_eq!(value["records"][0]["id"], "77");
    }
}
