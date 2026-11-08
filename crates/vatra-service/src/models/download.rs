//! Downloads the person chose: one at a time, into `.part` files that resume, checked against Hugging Face's SHA-256.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde::Serialize;
use sha2::{Digest, Sha256};
use url::Url;
use vatra_models::gguf::{self, ModelFacts};
use vatra_models::variant::RepoFile;

const PROGRESS_EVERY: Duration = Duration::from_millis(250);
const RETRIES: usize = 3;
const MAX_HEADER: usize = 64 << 20;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub bytes: u64,
    pub total: u64,
    pub file: String,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum DownloadError {
    #[error("another download is running")]
    Busy,
    #[error("not enough disk space: {needed} bytes needed, {free} free")]
    NoSpace { needed: u64, free: u64 },
    #[error("`{file}` didn't match its checksum, so it was deleted; try again")]
    Checksum { file: String },
    #[error("the download was cancelled")]
    Cancelled,
    #[error("the download failed: {0}")]
    Network(String),
    #[error("couldn't write the model: {0}")]
    Disk(String),
}

/// One file to fetch: where from, what it is, and where it goes.
pub struct Job {
    pub url: Url,
    pub file: RepoFile,
    pub dest: PathBuf,
    /// The Hugging Face token, for models that need an account.
    pub token: Option<String>,
}

pub struct Downloader {
    http: reqwest::Client,
    busy: AtomicBool,
    cancelled: AtomicBool,
}

struct BusyGuard<'a>(&'a AtomicBool);
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Default for Downloader {
    fn default() -> Self {
        Downloader {
            http: reqwest::Client::builder()
                .user_agent(concat!("vatra/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(Duration::from_secs(20))
                .read_timeout(Duration::from_secs(60))
                .build()
                .expect("client"),
            busy: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
        }
    }
}

impl Downloader {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    /// Fetches every job. `free` is the space on the destination disk, when known.
    pub async fn run(
        &self,
        jobs: &[Job],
        free: Option<u64>,
        progress: &(dyn Fn(DownloadProgress) + Send + Sync),
    ) -> Result<(), DownloadError> {
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err(DownloadError::Busy);
        }
        let _busy = BusyGuard(&self.busy);
        self.cancelled.store(false, Ordering::SeqCst);
        let total: u64 = jobs.iter().fold(0u64, |sum, job| sum.saturating_add(job.file.size));
        let have: u64 = jobs.iter().map(|job| done_bytes(&job.dest).unwrap_or_else(|| part_len(&job.dest))).sum();
        if let Some(free) = free {
            let needed = total.saturating_sub(have);
            if needed > free {
                return Err(DownloadError::NoSpace { needed, free });
            }
        }
        let mut before = 0;
        for job in jobs {
            if let Err(error) = self.fetch(job, before, total, progress).await {
                if error == DownloadError::Cancelled {
                    let _ = std::fs::remove_file(part_path(&job.dest));
                }
                return Err(error);
            }
            before += job.file.size;
        }
        progress(DownloadProgress { bytes: total, total, file: String::new() });
        Ok(())
    }

    async fn fetch(
        &self,
        job: &Job,
        before: u64,
        total: u64,
        progress: &(dyn Fn(DownloadProgress) + Send + Sync),
    ) -> Result<(), DownloadError> {
        if done_bytes(&job.dest).is_some() {
            let dest = job.dest.clone();
            let expected = job.file.sha256.clone();
            let matches = tokio::task::spawn_blocking(move || match expected {
                Some(expected) => sha256_file(&dest).is_ok_and(|actual| actual.eq_ignore_ascii_case(&expected)),
                None => true,
            })
            .await
            .unwrap_or(false);
            if matches {
                return Ok(());
            }
            std::fs::remove_file(&job.dest).map_err(disk)?;
        }
        if let Some(parent) = job.dest.parent() {
            std::fs::create_dir_all(parent).map_err(disk)?;
        }
        let part = part_path(&job.dest);
        let mut attempts = 0;
        loop {
            match self.fetch_once(job, &part, before, total, progress).await {
                Ok(()) => break,
                Err(DownloadError::Network(message)) => {
                    attempts += 1;
                    if attempts > RETRIES {
                        return Err(DownloadError::Network(message));
                    }
                    tokio::time::sleep(Duration::from_millis(500 * attempts as u64)).await;
                }
                Err(other) => return Err(other),
            }
        }
        let file = job.file.path.clone();
        let dest = job.dest.clone();
        let expected = job.file.sha256.clone();
        tokio::task::spawn_blocking(move || -> Result<(), DownloadError> {
            if let Some(expected) = expected {
                let actual = sha256_file(&part).map_err(disk)?;
                if !actual.eq_ignore_ascii_case(&expected) {
                    let _ = std::fs::remove_file(&part);
                    return Err(DownloadError::Checksum { file });
                }
            }
            std::fs::rename(&part, &dest).map_err(disk)
        })
        .await
        .map_err(|error| DownloadError::Disk(error.to_string()))?
    }

    async fn fetch_once(
        &self,
        job: &Job,
        part: &Path,
        before: u64,
        total: u64,
        progress: &(dyn Fn(DownloadProgress) + Send + Sync),
    ) -> Result<(), DownloadError> {
        let mut have = std::fs::metadata(part).map(|meta| meta.len()).unwrap_or(0);
        if have >= job.file.size && job.file.size > 0 {
            return Ok(());
        }
        let mut request = self.http.get(job.url.clone());
        if let Some(token) = &job.token {
            request = request.bearer_auth(token);
        }
        if have > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let mut response = request.send().await.map_err(network)?;
        // Not append mode: on Windows it can't truncate, which a full response needs.
        let mut output = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(part).map_err(disk)?;
        output.seek(SeekFrom::Start(have)).map_err(disk)?;
        match response.status() {
            StatusCode::PARTIAL_CONTENT => {}
            status if status.is_success() => {
                // The server sent the whole file: start over.
                output.set_len(0).map_err(disk)?;
                output.seek(SeekFrom::Start(0)).map_err(disk)?;
                have = 0;
            }
            StatusCode::RANGE_NOT_SATISFIABLE => {
                output.set_len(0).map_err(disk)?;
                return Err(DownloadError::Network("the server refused to resume; starting over".into()));
            }
            status => return Err(DownloadError::Network(format!("HTTP {status}"))),
        }
        let mut last = Instant::now();
        loop {
            if self.cancelled.load(Ordering::SeqCst) {
                return Err(DownloadError::Cancelled);
            }
            let Some(chunk) = response.chunk().await.map_err(network)? else { break };
            output.write_all(&chunk).map_err(disk)?;
            have += chunk.len() as u64;
            if last.elapsed() >= PROGRESS_EVERY {
                last = Instant::now();
                progress(DownloadProgress { bytes: before + have, total, file: job.file.path.clone() });
            }
        }
        output.sync_all().map_err(disk)?;
        if job.file.size > 0 && have < job.file.size {
            return Err(DownloadError::Network("the connection closed early".into()));
        }
        Ok(())
    }
}

pub fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

fn part_len(dest: &Path) -> u64 {
    std::fs::metadata(part_path(dest)).map(|meta| meta.len()).unwrap_or(0)
}

fn done_bytes(dest: &Path) -> Option<u64> {
    std::fs::metadata(dest).ok().filter(|meta| meta.is_file()).map(|meta| meta.len())
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The facts in a local GGUF file's header, read in growing pieces.
pub fn local_facts(path: &Path) -> Result<ModelFacts, String> {
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut buffer = Vec::new();
    let mut want = 1 << 20;
    loop {
        let have = buffer.len();
        buffer.resize(want, 0);
        let read = read_full(&mut file, &mut buffer[have..]).map_err(|error| error.to_string())?;
        buffer.truncate(have + read);
        match gguf::parse(&buffer) {
            Ok(header) => return Ok(ModelFacts::from_header(&header)),
            Err(gguf::GgufError::NeedMore { at_least }) if read > 0 && at_least <= MAX_HEADER => {
                want = at_least.max(buffer.len() * 2).min(MAX_HEADER);
            }
            Err(gguf::GgufError::NeedMore { .. }) => return Err("the file ends inside its header".into()),
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn read_full(file: &mut std::fs::File, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buffer.len() {
        let read = file.read(&mut buffer[total..])?;
        if read == 0 {
            break;
        }
        total += read;
    }
    Ok(total)
}

fn network(error: reqwest::Error) -> DownloadError {
    DownloadError::Network(error.to_string())
}

fn disk(error: std::io::Error) -> DownloadError {
    DownloadError::Disk(error.to_string())
}
