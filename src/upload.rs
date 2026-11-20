use crate::{App, Error, blocking};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use minijinja::context;
use raidledger::lockouts::Manifest;
use raidledger::reports::{import_captures, valid_capture};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const CHUNK_SIZE: usize = 256 * 1024;
const MAX_CHUNKS: usize = 32_768;

#[derive(Default)]
pub struct Uploads(Mutex<HashMap<IpAddr, Upload>>);

struct Upload {
    key: String,
    chunks: usize,
    last_chunk: usize,
    path: PathBuf,
    status: Option<Value>,
    processing: bool,
}

impl Drop for Upload {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Deserialize)]
struct UploadMetadata {
    filename: String,
    server: String,
    #[serde(default)]
    timezone: String,
    chunks: usize,
    capture: Option<String>,
    year: Option<u32>,
}

pub async fn form(State(app): State<App>) -> Result<Html<String>, Error> {
    app.render(
        "upload.html",
        context! { request => context! { path => "/upload" }, SERVERS => ["stable", "develop"] },
    )
}

pub async fn progress(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    let state = app.uploads.0.lock().unwrap();
    match state
        .get(&peer.ip())
        .and_then(|current| current.status.as_ref())
    {
        Some(result) => Json(result.clone()).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

pub async fn receive(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Error> {
    let peer = peer.ip();
    if headers
        .get(header::CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes() == b"application/json")
    {
        let data: UploadMetadata = serde_json::from_slice(&body)
            .map_err(|_| Error(StatusCode::BAD_REQUEST, "Invalid upload metadata"))?;
        if data
            .capture
            .as_deref()
            .is_some_and(|capture| !valid_capture(capture))
            || data.year.is_some_and(|year| !(2000..2100).contains(&year))
        {
            return Err(Error(StatusCode::BAD_REQUEST, "Invalid capture metadata"));
        }
        let path = {
            let mut uploads = app.uploads.0.lock().unwrap();
            let upload = uploads
                .get_mut(&peer)
                .ok_or(Error(StatusCode::CONFLICT, "No upload in progress"))?;
            if upload.chunks == 0 || upload.chunks != data.chunks {
                return Err(Error(StatusCode::CONFLICT, "Upload chunks are missing"));
            }
            if upload.status.is_some() {
                return Ok(StatusCode::CREATED.into_response());
            }
            upload.processing = true;
            upload.status =
                Some(json!({"done": 0, "status": "Processing combat log", "slices": {}}));
            upload.path.clone()
        };
        tokio::task::spawn_blocking(move || {
            let result = import_file(
                &app.root,
                &path,
                &data,
                data.year.unwrap_or_else(current_year),
            );
            let mut uploads = app.uploads.0.lock().unwrap();
            if let Some(upload) = uploads.get_mut(&peer) {
                upload.processing = false;
                upload.status = Some(match result {
                    Ok(ids) => {
                        let slices = ids
                            .into_iter()
                            .map(|id| (id, json!({"status": "Saved", "done": 1})))
                            .collect::<serde_json::Map<_, _>>();
                        json!({"done": 1, "status": "Reports saved", "slices": slices})
                    }
                    Err(error) => {
                        json!({"done": -1, "status": format!("Error: {error}"), "slices": {}})
                    }
                });
                let _ = fs::remove_file(&upload.path);
            }
        });
        return Ok(StatusCode::CREATED.into_response());
    }

    let key = headers
        .get("x-upload-id")
        .and_then(|value| value.to_str().ok())
        .filter(|key| {
            !key.is_empty() && key.len() <= 40 && key.bytes().all(|byte| byte.is_ascii_digit())
        })
        .ok_or(Error(StatusCode::BAD_REQUEST, "Invalid upload ID"))?
        .to_owned();
    let index = headers
        .get("x-chunk")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|index| *index < MAX_CHUNKS)
        .ok_or(Error(StatusCode::BAD_REQUEST, "Invalid upload chunk"))?;
    if body.is_empty() || body.len() > CHUNK_SIZE {
        return Err(Error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Upload chunk exceeds the size limit",
        ));
    }
    blocking(move || {
        let mut uploads = app.uploads.0.lock().unwrap();
        if uploads.get(&peer).is_some_and(|upload| upload.processing) {
            return Err(Error(StatusCode::CONFLICT, "An upload is processing"));
        }
        if !uploads.get(&peer).is_some_and(|upload| upload.key == key) {
            if index != 0 {
                return Err(Error(StatusCode::CONFLICT, "First upload chunk is missing"));
            }
            let dir = app.root.join("uploads").join("rust-staging");
            fs::create_dir_all(&dir).map_err(write_error)?;
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = dir.join(format!("{nonce}-{}-{key}", std::process::id()));
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(write_error)?;
            uploads.insert(
                peer,
                Upload {
                    key,
                    path,
                    chunks: 0,
                    last_chunk: CHUNK_SIZE,
                    status: None,
                    processing: false,
                },
            );
        }
        let upload = uploads.get_mut(&peer).unwrap();
        if index < upload.chunks {
            return Ok(StatusCode::OK.into_response());
        }
        if index != upload.chunks || upload.last_chunk != CHUNK_SIZE || upload.status.is_some() {
            return Err(Error(StatusCode::CONFLICT, "Unexpected upload chunk"));
        }
        let mut file = OpenOptions::new()
            .write(true)
            .open(&upload.path)
            .map_err(write_error)?;
        file.seek(SeekFrom::Start((index * CHUNK_SIZE) as u64))
            .map_err(write_error)?;
        file.write_all(&body).map_err(write_error)?;
        upload.chunks += 1;
        upload.last_chunk = body.len();
        Ok(StatusCode::OK.into_response())
    })
    .await
}

fn write_error(error: io::Error) -> Error {
    eprintln!("Upload write failed: {error}");
    Error(
        StatusCode::INSUFFICIENT_STORAGE,
        "Could not store upload chunk",
    )
}

fn import_file(
    root: &Path,
    source: &Path,
    request: &UploadMetadata,
    year: u32,
) -> io::Result<Vec<String>> {
    let realm = &request.server;
    let digest = sha256(source)?;
    let archive = root
        .join("uploads")
        .join("raidledger-originals")
        .join(&digest);
    let provenance = archive.join(format!("{year}-{}.json", crate::path_segment(realm)));
    let existing = File::open(&provenance)
        .ok()
        .and_then(|file| serde_json::from_reader::<_, Receipt>(BufReader::new(file)).ok());
    if archive.join("original").is_file()
        && let Some(existing) = existing
        && existing.capture == request.capture
        && existing.timezone == request.timezone
        && !existing.reports.is_empty()
        && existing.reports.iter().all(|id| {
            raidledger::reports::valid_id(id)
                && root.join("LogsDir").join(format!("{id}.uwu")).is_file()
        })
    {
        return Ok(existing.reports);
    }
    let mut captures = BTreeSet::from([format!("upload-{year}-{digest}")]);
    captures.extend(request.capture.iter().cloned());
    let ids = import_payload(root, source, request, year, captures)?;
    fs::create_dir_all(&archive)?;
    match fs::hard_link(source, archive.join("original")) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let stage = archive.join(source.file_name().unwrap());
    let mut record = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stage)?;
    let result = (|| {
        serde_json::to_writer(
            &mut record,
            &json!({
                "sha256": digest, "filename": request.filename, "server": realm, "year": year,
                "timezone": request.timezone, "capture": request.capture, "reports": ids,
            }),
        )?;
        record.sync_all()?;
        drop(record);
        fs::rename(&stage, provenance)
    })();
    if result.is_err() {
        let _ = fs::remove_file(stage);
    }
    result?;
    Ok(ids)
}

#[derive(Deserialize)]
struct Receipt {
    capture: Option<String>,
    timezone: String,
    reports: Vec<String>,
}

fn sha256(source: &Path) -> io::Result<String> {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("shasum");
        command.args(["-a", "256"]);
        command
    } else if cfg!(windows) {
        let mut command = Command::new("certutil.exe");
        command.arg("-hashfile");
        command
    } else {
        Command::new("sha256sum")
    };
    command.arg(source);
    if cfg!(windows) {
        command.arg("SHA256");
    }
    let output = command.stdin(Stdio::null()).output()?;
    if output.status.success()
        && let Some(value) = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .find(|word| word.len() == 64 && word.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Ok(value.to_ascii_lowercase());
    }
    Err(io::Error::other("could not calculate the upload checksum"))
}

fn import_payload(
    root: &Path,
    source: &Path,
    request: &UploadMetadata,
    year: u32,
    captures: BTreeSet<String>,
) -> io::Result<Vec<String>> {
    let filename = request.filename.to_ascii_lowercase();
    let realm = &request.server;
    if filename.ends_with(".txt") {
        return import_captures(
            root,
            [Ok(BufReader::new(File::open(source)?))],
            year,
            realm,
            None,
            captures,
        );
    }
    let sevenzip = sevenzip();
    let mut listing = Command::new(&sevenzip)
        .args(["l", "-ba", "-slt", "-p-", "--"])
        .arg(source)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut text = String::new();
    listing
        .stdout
        .take()
        .unwrap()
        .take(4 * 1024 * 1024 + 1)
        .read_to_string(&mut text)?;
    if text.len() > 4 * 1024 * 1024 {
        let _ = listing.kill();
        let _ = listing.wait();
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "archive listing is too large",
        ));
    }
    if !listing.wait()?.success() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "could not read archive",
        ));
    }
    let entries = text
        .lines()
        .filter_map(|line| line.strip_prefix("Path = "))
        .filter(|name| name.to_ascii_lowercase().ends_with(".txt"))
        .filter(|name| !name.starts_with(['-', '@']) && !name.chars().any(char::is_control))
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "archive does not contain a combat log",
        ));
    }
    let manifest = if text.lines().any(|line| line == "Path = uwu-lockouts.json") {
        let mut raw = Vec::new();
        let mut member = archive_entry(&sevenzip, source, "uwu-lockouts.json")?.take(2_000_001);
        member.read_to_end(&mut raw)?;
        (raw.len() <= 2_000_000)
            .then(|| Manifest::parse(&raw).ok())
            .flatten()
    } else {
        None
    };
    let readers = entries
        .into_iter()
        .map(|entry| archive_entry(&sevenzip, source, entry).map(BufReader::new));
    import_captures(
        root,
        readers,
        year,
        realm,
        manifest
            .as_ref()
            .map(|manifest| (manifest, request.timezone.as_str())),
        captures,
    )
}

pub(crate) fn sevenzip() -> std::ffi::OsString {
    crate::app_env("RAIDLEDGER_7Z", "UWU_7Z").unwrap_or_else(|| {
        if cfg!(windows) {
            "7z.exe".into()
        } else {
            "7zz".into()
        }
    })
}

fn archive_entry(executable: &OsStr, source: &Path, entry: &str) -> io::Result<ArchiveEntry> {
    let mut child = Command::new(executable)
        .args(["x", "-so", "-spd", "-p-", "--"])
        .arg(source)
        .arg(entry)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    Ok(ArchiveEntry { child, stdout })
}

struct ArchiveEntry {
    child: Child,
    stdout: ChildStdout,
}

impl Read for ArchiveEntry {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let len = self.stdout.read(bytes)?;
        if len == 0 && !self.child.wait()?.success() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "could not extract combat log",
            ));
        }
        Ok(len)
    }
}

impl Drop for ArchiveEntry {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(crate) fn current_year() -> u32 {
    year_at(SystemTime::now())
}

pub(crate) fn year_at(stamp: SystemTime) -> u32 {
    let mut remaining = stamp
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86_400;
    let mut year = 1970u32;
    loop {
        let leap =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let days = 365 + u64::from(leap);
        if remaining < days {
            return year;
        }
        remaining -= days;
        year += 1;
    }
}
