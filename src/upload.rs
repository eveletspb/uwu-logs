use crate::{App, Error, blocking};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use minijinja::context;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use uwu_logs::reports::import_log;

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
    chunks: usize,
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
                &data.filename,
                &data.server,
                current_year(),
            );
            let mut uploads = app.uploads.0.lock().unwrap();
            if let Some(upload) = uploads.get_mut(&peer) {
                upload.processing = false;
                upload.status = Some(match result {
                    Ok(id) => {
                        json!({"done": 1, "status": "Report saved", "slices": { id: { "status": "Saved", "done": 1 } }})
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

pub fn import_file(
    root: &Path,
    source: &Path,
    filename: &str,
    realm: &str,
    year: u32,
) -> io::Result<String> {
    let filename = filename.to_ascii_lowercase();
    if filename.ends_with(".txt") {
        return import_log(root, BufReader::new(File::open(source)?), year, realm);
    }
    let sevenzip = std::env::var_os("UWU_7Z").unwrap_or_else(|| "7zz".into());
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
    let readers = entries.into_iter().map(|entry| {
        let mut child = Command::new(&sevenzip)
            .args(["x", "-so", "-spd", "-p-", "--"])
            .arg(source)
            .arg(entry)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().unwrap();
        Ok(Box::new(BufReader::new(ArchiveEntry { child, stdout })) as Box<dyn io::BufRead>)
    });
    uwu_logs::reports::import_logs(root, readers, year, realm)
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

fn current_year() -> u32 {
    let mut remaining = SystemTime::now()
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
