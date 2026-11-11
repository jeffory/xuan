//! Model files the host downloads for a plugin (`[[models]]` in its
//! manifest): fetched over https into `<id>.part` in the plugin's models
//! folder, checked against the declared size and SHA-256 while streaming,
//! then synced and renamed into place. Nothing is ever unpacked or run.
//!
//! A file is handed to the plugin only while it matches what was verified:
//! [`Cache`] records the size, times and inode seen when its hash was last
//! computed, and a file that changed since is hashed again before use.
//! See "Models" in `docs/PLUGINS.md`.
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use url::Url;

use super::manifest::Model;

/// Most redirects one download follows.
pub const MAX_REDIRECTS: usize = 5;
/// The folder, inside a plugin's data folder, its models are kept in.
pub const MODELS_FOLDER: &str = "models";
/// What a download in progress is called, after the model id.
pub const PART_SUFFIX: &str = ".part";
/// The verification record in the models folder.
const CACHE_FILE: &str = ".verified.json";
const BUFFER: usize = 256 * 1024;

/// The models folder inside a plugin's data folder.
pub fn models_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(MODELS_FOLDER)
}

/// Create `dir` (and its parents) and make it private to the user (0700 on
/// Unix), as model files may be large and are trusted once verified.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Where a model's verified file lives.
pub fn model_path(dir: &Path, model: &Model) -> PathBuf {
    dir.join(model.file_name())
}

/// Where a model is downloaded to before it is verified.
pub fn part_path(dir: &Path, model: &Model) -> PathBuf {
    dir.join(format!("{}{PART_SUFFIX}", model.id))
}

/// An answer to one GET, before following redirects.
pub struct Reply {
    pub status: u16,
    /// The `Location` header, for redirects.
    pub location: Option<String>,
    /// The `Content-Length` header, when the server sent one.
    pub length: Option<u64>,
    pub body: Box<dyn Read + Send>,
}

/// Fetches one URL without following redirects. [`Https`] is the real one;
/// tests use a fake.
pub trait Transport: Send + Sync {
    /// GET `url`. `size` is the declared size of the file, for timeouts.
    fn get(&self, url: &Url, size: u64) -> Result<Reply>;
}

/// [`Transport`] over https with rustls, checking certificates against the
/// system's trust store. Timeouts bound connecting and the wait for the
/// response; the body may take as long as a slow link needs for the
/// declared size, and the user can cancel. [`Https::with_limit`] bounds the
/// whole request instead, for small replies such as the update check's.
pub struct Https {
    agent: ureq::Agent,
    /// The most one request may take, body included, when set.
    limit: Option<Duration>,
}

impl Https {
    pub fn new() -> Self {
        Self::with_roots(ureq::tls::RootCerts::PlatformVerifier)
    }

    /// Trust only these DER certificates, as tests with their own
    /// certificate authority do.
    pub fn with_certificates(certificates: &[Vec<u8>]) -> Self {
        let certificates = certificates
            .iter()
            .map(|der| ureq::tls::Certificate::from_der(der).to_owned())
            .collect();
        Self::with_roots(ureq::tls::RootCerts::Specific(Arc::new(certificates)))
    }

    fn with_roots(roots: ureq::tls::RootCerts) -> Self {
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::Rustls)
            .root_certs(roots)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .https_only(true)
            // Redirects are followed by [`download`], which checks each.
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_resolve(Some(Duration::from_secs(30)))
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_send_request(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .user_agent(format!("Xuan/{}", env!("CARGO_PKG_VERSION")))
            .build();
        Self {
            agent: config.into(),
            limit: None,
        }
    }

    /// Give up on any request that has not finished within `limit`,
    /// connecting and reading the body included.
    pub fn with_limit(mut self, limit: Duration) -> Self {
        self.limit = Some(limit);
        self
    }
}

impl Default for Https {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for Https {
    fn get(&self, url: &Url, size: u64) -> Result<Reply> {
        // Ten minutes, plus the time the file takes at 64 KiB/s.
        let body_time = Duration::from_secs(600 + size / (64 * 1024));
        let response = self
            .agent
            .get(url.as_str())
            .config()
            .timeout_recv_body(Some(body_time))
            .timeout_global(self.limit)
            .build()
            .call()
            .with_context(|| format!("Cannot download {url}"))?;
        let status = response.status().as_u16();
        let location = (response.headers().get("location"))
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let length = response.body().content_length();
        Ok(Reply {
            status,
            location,
            length,
            body: Box::new(response.into_body().into_reader()),
        })
    }
}

/// How far a download or verification got, and the flag that cancels it.
#[derive(Debug, Default)]
pub struct Progress {
    done: AtomicU64,
    cancel: AtomicBool,
}

impl Progress {
    /// Bytes read so far.
    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// The error of a download or verification the user cancelled.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// GET `url` with `transport`, following at most [`MAX_REDIRECTS`]
/// redirects, each of which must stay on https. Returns the URL that
/// answered and its reply, whatever its status. `size` is passed on to
/// [`Transport::get`]; `cancelled` is asked before each request.
pub fn follow(
    transport: &dyn Transport,
    mut url: Url,
    size: u64,
    cancelled: impl Fn() -> bool,
) -> Result<(Url, Reply)> {
    ensure!(url.scheme() == "https", "{url} is not an https URL");
    let start = url.clone();
    let mut redirects = 0;
    loop {
        if cancelled() {
            bail!(Cancelled);
        }
        let reply = transport.get(&url, size)?;
        if !matches!(reply.status, 301 | 302 | 303 | 307 | 308) {
            return Ok((url, reply));
        }
        redirects += 1;
        ensure!(
            redirects <= MAX_REDIRECTS,
            "{start} redirected more than {MAX_REDIRECTS} times"
        );
        let location = reply
            .location
            .with_context(|| format!("{url} redirected without a location"))?;
        let next = url
            .join(&location)
            .with_context(|| format!("{url} redirected to an invalid location"))?;
        ensure!(
            next.scheme() == "https",
            "{url} redirected to {next}, which is not https; Xuan only downloads over https"
        );
        url = next;
    }
}

/// Download `model` into `dir` and verify it. On success the verified file
/// is at [`model_path`], replacing any earlier one; on any failure, or when
/// cancelled, the partial file is removed and an earlier file is untouched.
pub fn download(
    transport: &dyn Transport,
    model: &Model,
    dir: &Path,
    progress: &Progress,
) -> Result<PathBuf> {
    let part = part_path(dir, model);
    let result = fetch(transport, model, dir, &part, progress);
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

fn fetch(
    transport: &dyn Transport,
    model: &Model,
    dir: &Path,
    part: &Path,
    progress: &Progress,
) -> Result<PathBuf> {
    let url = Url::parse(&model.url).context("The model URL is invalid")?;
    let (url, reply) = follow(transport, url, model.size, || progress.cancelled())?;
    ensure!(
        reply.status == 200,
        "{url} answered with HTTP status {}",
        reply.status
    );
    if let Some(length) = reply.length {
        ensure!(
            length == model.size,
            "{url} is {length} bytes, but the plugin declares {} bytes",
            model.size
        );
    }
    create_private_dir(dir).with_context(|| format!("Cannot create {}", dir.display()))?;
    let mut file = create_file(part)?;
    let mut body = reply.body;
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = vec![0; BUFFER];
    let mut total: u64 = 0;
    loop {
        if progress.cancelled() {
            bail!(Cancelled);
        }
        let read = match body.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error).with_context(|| format!("Cannot download {url}")),
        };
        total += read as u64;
        ensure!(
            total <= model.size,
            "{url} sent more than the {} bytes the plugin declares",
            model.size
        );
        digest.update(&buffer[..read]);
        file.write_all(&buffer[..read])
            .with_context(|| format!("Cannot write {}", part.display()))?;
        progress.done.store(total, Ordering::Relaxed);
    }
    ensure!(
        total == model.size,
        "{url} sent {total} bytes, but the plugin declares {}",
        model.size
    );
    let sha256 = hex(digest.finish().as_ref());
    ensure!(
        sha256 == model.sha256(),
        "{url} does not match the SHA-256 the plugin declares (got {sha256})"
    );
    file.sync_all()
        .with_context(|| format!("Cannot write {}", part.display()))?;
    drop(file);
    let path = model_path(dir, model);
    std::fs::rename(part, &path)
        .with_context(|| format!("Cannot move the model to {}", path.display()))?;
    sync_dir(dir);
    Cache::record(dir, &path, &sha256);
    Ok(path)
}

/// A new, empty file only the user can read (0600 on Unix).
fn create_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .with_context(|| format!("Cannot create {}", path.display()))
}

/// Make a rename in `dir` durable. Only Unix can sync a folder.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of a file, reporting progress and stopping when cancelled.
pub fn hash_file(path: &Path, progress: &Progress) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = vec![0; BUFFER];
    let mut total = 0;
    loop {
        if progress.cancelled() {
            bail!(Cancelled);
        }
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("Cannot read {}", path.display()));
            }
        };
        digest.update(&buffer[..read]);
        total += read as u64;
        progress.done.store(total, Ordering::Relaxed);
    }
    Ok(hex(digest.finish().as_ref()))
}

/// Hash a model's file again and record the result. Returns whether it
/// matches the declared size and SHA-256.
pub fn verify(model: &Model, dir: &Path, progress: &Progress) -> Result<bool> {
    let path = model_path(dir, model);
    let before = Stamp::of(&path).with_context(|| format!("Cannot read {}", path.display()))?;
    if before.size != model.size {
        return Ok(false);
    }
    let sha256 = hash_file(&path, progress)?;
    // Only record the hash of the file as it was while it was read.
    if Stamp::of(&path).ok() == Some(before) {
        Cache::record(dir, &path, &sha256);
    }
    Ok(sha256 == model.sha256())
}

/// What a model's file looks like now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Missing,
    /// The size is right but the file changed since it was last hashed.
    Unverified,
    /// Hashed, unchanged since, and it matches the manifest.
    Ready,
    /// The wrong size, or hashed and different from the manifest.
    Corrupt,
}

/// The state of a model's file and its size on disk, without reading it.
pub fn state(model: &Model, dir: &Path) -> (State, Option<u64>) {
    let path = model_path(dir, model);
    let Ok(stamp) = Stamp::of(&path) else {
        return (State::Missing, None);
    };
    let state = if stamp.size != model.size {
        State::Corrupt
    } else {
        match Cache::lookup(dir, &path, stamp) {
            Some(sha256) if sha256 == model.sha256() => State::Ready,
            Some(_) => State::Corrupt,
            None => State::Unverified,
        }
    };
    (state, Some(stamp.size))
}

/// The paths of the models that are [`State::Ready`], by id.
pub fn ready_paths(models: &[Model], dir: &Path) -> BTreeMap<String, PathBuf> {
    models
        .iter()
        .filter(|model| state(model, dir).0 == State::Ready)
        .map(|model| (model.id.clone(), model_path(dir, model)))
        .collect()
}

/// Remove a model's file and any partial download of it.
pub fn delete(model: &Model, dir: &Path) -> Result<()> {
    let path = model_path(dir, model);
    for file in [&path, &part_path(dir, model)] {
        match std::fs::remove_file(file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("Cannot delete {}", file.display()));
            }
        }
    }
    Cache::forget(dir, &path);
    Ok(())
}

/// Remove the whole models folder.
pub fn delete_all(dir: &Path) -> Result<()> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("Cannot delete {}", dir.display())),
    }
}

/// What identifies one version of a file without reading it: its size,
/// modification time, and on Unix its change time and inode, which a
/// process cannot set back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    size: u64,
    modified: u64,
    #[serde(default)]
    changed: i64,
    #[serde(default)]
    inode: u64,
}

impl Stamp {
    fn of(path: &Path) -> std::io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        if !metadata.is_file() {
            return Err(std::io::Error::other("not a file"));
        }
        let modified = (metadata.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |time| time.as_nanos() as u64);
        #[cfg(unix)]
        let (changed, inode) = {
            use std::os::unix::fs::MetadataExt;
            (
                metadata.ctime().saturating_mul(1_000_000_000) + metadata.ctime_nsec(),
                metadata.ino(),
            )
        };
        #[cfg(not(unix))]
        let (changed, inode) = (0, 0);
        Ok(Self {
            size: metadata.len(),
            modified,
            changed,
            inode,
        })
    }
}

/// The hashes computed for the files in a models folder, kept in
/// `.verified.json` there, each with the [`Stamp`] of the file it was
/// computed for.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    #[serde(default)]
    files: BTreeMap<String, Entry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    stamp: Stamp,
    sha256: String,
}

/// Downloads and verifications of one plugin's models may finish together.
static CACHE_LOCK: Mutex<()> = Mutex::new(());

impl Cache {
    fn load(dir: &Path) -> Self {
        std::fs::read(dir.join(CACHE_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self, dir: &Path) {
        let Ok(text) = serde_json::to_vec_pretty(self) else {
            return;
        };
        let temporary = dir.join(format!("{CACHE_FILE}.tmp"));
        if std::fs::write(&temporary, text).is_ok() {
            let _ = std::fs::rename(&temporary, dir.join(CACHE_FILE));
        }
    }

    fn name(path: &Path) -> Option<String> {
        Some(path.file_name()?.to_str()?.to_owned())
    }

    /// The recorded hash of `path`, if it is unchanged since.
    fn lookup(dir: &Path, path: &Path, stamp: Stamp) -> Option<String> {
        let _lock = CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let entry = Self::load(dir).files.remove(&Self::name(path)?)?;
        (entry.stamp == stamp).then_some(entry.sha256)
    }

    fn record(dir: &Path, path: &Path, sha256: &str) {
        let (Some(name), Ok(stamp)) = (Self::name(path), Stamp::of(path)) else {
            return;
        };
        let _lock = CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut cache = Self::load(dir);
        cache.files.insert(
            name,
            Entry {
                stamp,
                sha256: sha256.to_owned(),
            },
        );
        cache.save(dir);
    }

    fn forget(dir: &Path, path: &Path) {
        let Some(name) = Self::name(path) else {
            return;
        };
        let _lock = CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut cache = Self::load(dir);
        if cache.files.remove(&name).is_some() {
            cache.save(dir);
        }
    }
}

#[cfg(test)]
mod tests;
