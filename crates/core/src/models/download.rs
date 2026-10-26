//! HTTP download with resume and SHA-256 verification, the engine zip
//! install and the model downloads (`docs/01`, "Install model").
//!
//! - ureq with rustls and the platform verifier (Windows certificate store),
//!   proxies from the environment, https only, redirects followed (GitHub
//!   release assets and Hugging Face files both redirect to a download host;
//!   a redirect to plain http is refused), User-Agent `diarist/<version>`
//!   and nothing else about the user.
//! - Partial data lives in `<dest>.part`; a later call resumes with a Range
//!   request (a server that ignores it, or answers from another offset,
//!   restarts the file). `<dest>.part.json` records the URL and hash it
//!   belongs to; a `.part` from another URL or hash is not resumed.
//!   `<dest>.part.lock` keeps two programs from writing the same `.part`.
//!   Each request runs at most two minutes and the download continues with
//!   another Range request, so a dead connection ends as "stalled" instead
//!   of hanging. Every request starts from the original URL, so a resumed
//!   download gets a fresh redirect (signed asset links expire).
//! - The whole file is hashed before it is renamed into place; a mismatch
//!   deletes the partial file.
//! - The engine zip is extracted into `engine\<name>.tmp` and renamed to
//!   `engine\<name>` (a single top-level folder in the zip is stripped).
//! - Models (every file of an entry, through [`super::fetch::fetch_entry`])
//!   land where the manifest puts them and get the `<file>.sha256` cache
//!   the scanner reads (`record_sha256`). Since 0.1.0-beta.5 this
//!   replaces `nemo-speech pull`, whose downloader (System32 `curl.exe`)
//!   fails on networks that block certificate revocation checks.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::manifest::{check_engine_name, EngineEntry, Manifest};
use super::paths::DataPaths;
use crate::engine::process;
use crate::error::{CoreError, Result};

fn dl_err(msg: impl Into<String>) -> CoreError {
    CoreError::Models(msg.into())
}

/// Progress callback: (bytes on disk, total bytes when known).
pub type Progress<'a> = &'a mut dyn FnMut(u64, Option<u64>);

/// The User-Agent of every request: the app and its version, nothing else.
pub(crate) fn user_agent() -> String {
    format!("diarist/{}", crate::VERSION)
}

#[cfg(test)]
thread_local! {
    /// The tests' fake servers speak plain http on 127.0.0.1, so tests allow
    /// it unless a test turns this off to check the https-only rule. (Each
    /// test runs on its own thread, and downloads run on the caller's.)
    static TEST_ALLOW_HTTP: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Every request, and every redirect it follows, must be https: a redirect
/// to plain http is refused instead of followed. Off only in tests.
fn https_only() -> bool {
    #[cfg(test)]
    return !TEST_ALLOW_HTTP.with(|a| a.get());
    #[cfg(not(test))]
    true
}

/// An HTTPS client; `limit` caps each request (connect to last byte).
pub(crate) fn agent(limit: Option<Duration>) -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .https_only(https_only())
        .user_agent(user_agent())
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_global(limit)
        .http_status_as_error(false)
        .build()
        .into()
}

/// Lowercase hex SHA-256 of a file.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = File::open(path).map_err(|e| dl_err(format!("open {}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Checks size (when given) and SHA-256 of a file.
pub fn verify_file(path: &Path, size: Option<u64>, sha256: &str) -> Result<()> {
    let len = std::fs::metadata(path)?.len();
    if let Some(want) = size {
        if len != want {
            return Err(dl_err(format!(
                "{}: size {len} bytes, expected {want}",
                path.display()
            )));
        }
    }
    let got = sha256_file(path)?;
    if !got.eq_ignore_ascii_case(sha256.trim()) {
        return Err(dl_err(format!(
            "{}: SHA-256 {got} does not match the expected {}",
            path.display(),
            sha256.trim()
        )));
    }
    Ok(())
}

/// `dest` with `suffix` appended to its file name.
fn suffixed(dest: &Path, suffix: &str) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn part_path(dest: &Path) -> PathBuf {
    suffixed(dest, ".part")
}

/// `<dest>.part.json`: where the bytes in `<dest>.part` came from.
fn part_source_path(dest: &Path) -> PathBuf {
    suffixed(dest, ".part.json")
}

/// The download a `.part` belongs to. A `.part` is resumed only by a
/// download of the same address for the same expected file; anything else
/// (another URL or hash, or no record: a `.part` from before 0.1.0-beta.5)
/// starts from byte 0.
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PartSource {
    url: String,
    sha256: String,
}

impl PartSource {
    fn new(url: &str, sha256: &str) -> Self {
        Self {
            url: url.to_owned(),
            sha256: sha256.trim().to_ascii_lowercase(),
        }
    }

    /// True when `<dest>.part.json` records this download.
    fn wrote(&self, dest: &Path) -> bool {
        std::fs::read(part_source_path(dest))
            .ok()
            .and_then(|b| serde_json::from_slice::<PartSource>(&b).ok())
            .is_some_and(|s| s == *self)
    }

    fn record(&self, dest: &Path) -> Result<()> {
        let json = serde_json::to_vec(self).map_err(|e| dl_err(e.to_string()))?;
        std::fs::write(part_source_path(dest), json)?;
        Ok(())
    }
}

/// `<dest>.part.lock`, locked while one download writes `<dest>.part`, so
/// two programs (the app and `diarist-cli models pull`) never write the
/// same partial file. The lock is the operating system's, so it goes away
/// with a program that crashed. The file is removed when the download ends.
struct PartLock {
    file: File,
    path: PathBuf,
}

impl PartLock {
    fn acquire(dest: &Path) -> Result<Self> {
        let path = suffixed(dest, ".part.lock");
        let name = dest.file_name().map_or_else(
            || dest.display().to_string(),
            |n| n.to_string_lossy().into(),
        );
        let busy = || {
            dl_err(format!(
                "{name} is already downloading in another Diarist program (the app or \
                 diarist-cli); wait for it to finish, then try again"
            ))
        };
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| match e.kind() {
                // A lock file another download is removing right now.
                std::io::ErrorKind::PermissionDenied => busy(),
                _ => dl_err(format!("open {}: {e}", path.display())),
            })?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file, path }),
            Err(std::fs::TryLockError::WouldBlock) => Err(busy()),
            Err(std::fs::TryLockError::Error(e)) => {
                Err(dl_err(format!("lock {}: {e}", path.display())))
            }
        }
    }
}

impl Drop for PartLock {
    fn drop(&mut self) {
        // Removed while still locked: a program that opened it meanwhile
        // finds it locked, one that comes later makes a new one.
        let _ = std::fs::remove_file(&self.path);
        let _ = self.file.unlock();
    }
}

/// The first byte of a `Content-Range: bytes <first>-<last>/<total>` value.
fn range_start(value: &str) -> Option<u64> {
    let rest = value.trim().strip_prefix("bytes")?.trim_start();
    rest.split('-').next()?.trim().parse().ok()
}

/// How long one request may run. A download that takes longer continues
/// with a new Range request from where it stopped, so a dead connection
/// cannot hang it forever.
const ATTEMPT_LIMIT: Duration = Duration::from_secs(120);
/// Requests in a row that added no bytes before the download counts as stalled.
const MAX_IDLE_ATTEMPTS: u32 = 3;

/// How one request ended.
enum Attempt {
    /// The server sent the whole remaining file.
    Complete,
    /// It stopped early (deadline, reset, short body); retry from `have`.
    Interrupted(String),
    /// The server ignores Range: the next request must not have a deadline.
    NoRange,
    /// The server answered a Range request with bytes from another offset:
    /// the partial file was emptied and the next request starts at byte 0.
    Restart,
}

/// One GET from `*have` (appending to `part`), reporting progress.
fn attempt(
    agent: &ureq::Agent,
    url: &str,
    part: &Path,
    have: &mut u64,
    size: Option<u64>,
    progress: Progress<'_>,
) -> Result<Attempt> {
    let mut req = agent.get(url);
    if *have > 0 {
        req = req.header("Range", format!("bytes={}-", *have));
    }
    let mut resp = req.call().map_err(|e| dl_err(format!("GET {url}: {e}")))?;
    let status = resp.status().as_u16();
    let append = match status {
        // Appended only when the range starts at the bytes on disk.
        206 => {
            let start = resp
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .and_then(range_start);
            if start != Some(*have) {
                if *have == 0 {
                    let from = start.map_or("an unknown byte".into(), |s| format!("byte {s}"));
                    return Err(dl_err(format!(
                        "GET {url}: HTTP 206 from {from} for a request of the whole file"
                    )));
                }
                OpenOptions::new().write(true).truncate(true).open(part)?;
                *have = 0;
                return Ok(Attempt::Restart);
            }
            true
        }
        200 => false,
        416 if *have > 0 => return Ok(Attempt::Complete), // already complete
        _ => return Err(dl_err(format!("GET {url}: HTTP {status}"))),
    };
    let restarted = !append && *have > 0;
    if !append {
        *have = 0;
    }
    let mut out = OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(part)?;
    let mut reader = resp.body_mut().as_reader();
    let mut buf = vec![0u8; 256 * 1024];
    progress(*have, size);
    loop {
        let n = match reader.read(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                out.flush()?;
                return Ok(if restarted {
                    Attempt::NoRange
                } else {
                    Attempt::Interrupted(e.to_string())
                });
            }
        };
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        *have += n as u64;
        progress(*have, size);
    }
    out.flush()?;
    Ok(match size {
        Some(s) if *have < s => {
            Attempt::Interrupted(format!("the server sent {} of {s} bytes", *have))
        }
        _ => Attempt::Complete,
    })
}

/// Downloads `url` to `dest`, resuming `<dest>.part` when present, and
/// verifies size and SHA-256 before the atomic rename. An existing `dest`
/// with the right hash is kept as is. Each request is limited to two
/// minutes and continues with a Range request; three requests in a row that
/// bring no data end the download as stalled (the partial file stays, so
/// calling it again resumes). A `.part` is resumed only by a download of
/// the same URL and hash (`<dest>.part.json`, [`PartSource`]) and a range
/// is appended only when it starts at the bytes on disk; otherwise the
/// download starts from byte 0. While it runs, `<dest>.part.lock` is held
/// ([`PartLock`]): a second download of the same file fails at once with
/// "already downloading".
pub fn download_file(
    url: &str,
    dest: &Path,
    sha256: &str,
    size: Option<u64>,
    progress: Progress<'_>,
) -> Result<()> {
    // An empty `sha256` (a user's "Add from URL" without a hash) skips the
    // hash check; the caller records the hash afterwards.
    let check = |p: &Path| {
        if sha256.trim().is_empty() {
            match size {
                Some(want) if std::fs::metadata(p)?.len() != want => Err(dl_err(format!(
                    "{}: size differs from the expected {want} bytes",
                    p.display()
                ))),
                _ => Ok(()),
            }
        } else {
            verify_file(p, size, sha256)
        }
    };
    let existed = dest.is_file();
    if existed && check(dest).is_ok() {
        return Ok(());
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _lock = PartLock::acquire(dest)?;
    // Another program may have finished it while this one waited to start.
    if !existed && dest.is_file() && check(dest).is_ok() {
        return Ok(());
    }
    let part = part_path(dest);
    let source = PartSource::new(url, sha256);
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > 0 && (!source.wrote(dest) || size.is_some_and(|s| have > s)) {
        std::fs::remove_file(&part)?;
        have = 0;
    }
    source.record(dest)?;
    let source_path = part_source_path(dest);
    if size != Some(have) || have == 0 {
        // Show what a resumed download already has while the connection opens.
        progress(have, size);
        let mut agent = agent(Some(ATTEMPT_LIMIT));
        let mut idle = 0;
        let fetched = loop {
            let before = have;
            match attempt(&agent, url, &part, &mut have, size, progress) {
                Err(e) => break Err(e),
                Ok(Attempt::Complete) => break Ok(()),
                Ok(Attempt::NoRange) => agent = self::agent(None),
                Ok(Attempt::Restart) => {}
                Ok(Attempt::Interrupted(why)) => {
                    idle = if have > before { 0 } else { idle + 1 };
                    if idle >= MAX_IDLE_ATTEMPTS {
                        break Err(dl_err(format!(
                            "download {url} stalled at {have} bytes ({why}); retrying resumes from there"
                        )));
                    }
                }
            }
        };
        if let Err(e) = fetched {
            // Nothing to resume: leave no empty `.part` or record behind.
            if have == 0 {
                let _ = std::fs::remove_file(&part);
                let _ = std::fs::remove_file(&source_path);
            }
            return Err(e);
        }
    }
    if let Err(e) = check(&part) {
        let _ = std::fs::remove_file(&part);
        let _ = std::fs::remove_file(&source_path);
        return Err(e);
    }
    std::fs::rename(&part, dest)
        .map_err(|e| dl_err(format!("rename to {}: {e}", dest.display())))?;
    let _ = std::fs::remove_file(&source_path);
    Ok(())
}

/// Writes the `<file>.sha256` cache the scanner trusts while it is not older
/// than the file (`scan::own_sha`), so a verified download is not hashed again.
pub(crate) fn record_sha256(file: &Path, sha256: &str) {
    let mut side = file.as_os_str().to_owned();
    side.push(".sha256");
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let _ = std::fs::write(
        PathBuf::from(side),
        format!("{}  {name}\n", sha256.trim().to_ascii_lowercase()),
    );
}

/// Extracts `zip_path` into `dest` (created). A single top-level folder
/// common to every entry is stripped. Entries escaping `dest` are rejected.
pub fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    let f = File::open(zip_path)?;
    let mut archive =
        zip::ZipArchive::new(f).map_err(|e| dl_err(format!("{}: {e}", zip_path.display())))?;
    let mut names = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| dl_err(format!("zip entry {i}: {e}")))?;
        let name = entry
            .enclosed_name()
            .ok_or_else(|| dl_err(format!("unsafe path in zip entry {i}")))?;
        names.push(name);
    }
    let first = |p: &Path| -> Option<std::ffi::OsString> {
        match p.components().next() {
            Some(Component::Normal(s)) => Some(s.to_owned()),
            _ => None,
        }
    };
    let prefix = names.first().and_then(|p| first(p)).filter(|top| {
        names
            .iter()
            .all(|n| first(n).as_ref() == Some(top) && n.components().count() >= 1)
            && names.iter().any(|n| n.components().count() > 1)
    });
    std::fs::create_dir_all(dest)?;
    for (i, name) in names.iter().enumerate() {
        let rel = match &prefix {
            Some(top) => name.strip_prefix(top).unwrap_or(name).to_path_buf(),
            None => name.clone(),
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out_path = dest.join(&rel);
        let mut entry = archive
            .by_index(i)
            .map_err(|e| dl_err(format!("zip entry {i}: {e}")))?;
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = File::create(&out_path)?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| dl_err(format!("extract {}: {e}", rel.display())))?;
    }
    Ok(())
}

/// Downloads and installs a runtime zip into `engine\<entry.name>`.
/// Returns the engine folder. An already installed folder is reused.
pub fn install_engine(
    entry: &EngineEntry,
    paths: &DataPaths,
    progress: Progress<'_>,
) -> Result<PathBuf> {
    // Checked at load for the manifests; this covers entries made from the
    // GitHub release list too, before the name becomes a folder.
    check_engine_name(&entry.name)?;
    let engine_root = paths.engine();
    let target = engine_root.join(&entry.name);
    if process::is_engine_dir(&target) {
        return Ok(target);
    }
    let zip_path = engine_root
        .join("downloads")
        .join(format!("{}.zip", entry.name));
    download_file(
        &entry.url,
        &zip_path,
        &entry.sha256,
        Some(entry.size_bytes).filter(|s| *s > 0),
        progress,
    )?;
    let tmp = engine_root.join(format!("{}.tmp", entry.name));
    let _ = std::fs::remove_dir_all(&tmp);
    extract_zip(&zip_path, &tmp)?;
    if !process::is_engine_dir(&tmp) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(dl_err(format!(
            "{} does not contain bin/{}",
            zip_path.display(),
            process::EXE_NAME
        )));
    }
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    std::fs::rename(&tmp, &target)
        .map_err(|e| dl_err(format!("rename to {}: {e}", target.display())))?;
    let _ = std::fs::remove_file(&zip_path);
    if let Some(dir) = zip_path.parent() {
        let _ = std::fs::remove_dir(dir); // only when empty
    }
    Ok(target)
}

/// Runtime zips put into `engine\` (or `engine\downloads\`) by hand, for
/// manifest runtimes that are not installed: each is checked against the
/// manifest's SHA-256 and extracted like a download (Refresh after "I
/// already have the files"). A zip that does not match is left alone and
/// reported. Returns what happened per zip found.
pub fn install_dropped_engines(paths: &DataPaths, manifest: &Manifest) -> Vec<Result<PathBuf>> {
    let root = paths.engine();
    let mut out = Vec::new();
    for entry in &manifest.engines {
        if process::is_engine_dir(&root.join(&entry.name)) {
            continue;
        }
        let file = format!("{}.zip", entry.name);
        let target = root.join("downloads").join(&file);
        let found = [root.join(&file), target.clone()]
            .into_iter()
            .find(|p| p.is_file())
            .or_else(|| {
                std::fs::read_dir(&root)
                    .ok()?
                    .flatten()
                    .map(|e| e.path())
                    .find(|p| {
                        p.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip"))
                            && std::fs::metadata(p).is_ok_and(|m| m.len() == entry.size_bytes)
                    })
            });
        let Some(zip) = found else { continue };
        let result = verify_file(&zip, Some(entry.size_bytes), &entry.sha256)
            .map_err(|e| dl_err(format!("{e} (expected {} {})", entry.name, entry.version)))
            .and_then(|()| {
                if zip != target {
                    std::fs::create_dir_all(root.join("downloads"))?;
                    std::fs::rename(&zip, &target)?;
                }
                install_engine(entry, paths, &mut |_, _| {})
            });
        out.push(result);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::fetch::fetch_entry;
    use crate::models::manifest::ModelEntry;
    use std::sync::{Arc, Mutex};

    /// Serves `body` on 127.0.0.1 with Range support; records Range headers.
    fn serve(body: Vec<u8>, honour_range: bool) -> (String, Arc<Mutex<Vec<String>>>) {
        serve_with(body, honour_range, None)
    }

    /// [`serve`], sending at most `chunk` bytes per response (a server that
    /// cuts every transfer short).
    fn serve_with(
        body: Vec<u8>,
        honour_range: bool,
        chunk: Option<usize>,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let (url, ranges, _) = serve_recording(body, honour_range, chunk);
        (url, ranges)
    }

    /// The value of request header `name`, or "".
    fn header(req: &tiny_http::Request, name: &'static str) -> String {
        req.headers()
            .iter()
            .find(|h| h.field.equiv(name))
            .map(|h| h.value.as_str().to_owned())
            .unwrap_or_default()
    }

    /// [`serve_with`], also recording each request's User-Agent.
    #[allow(clippy::type_complexity)]
    fn serve_recording(
        body: Vec<u8>,
        honour_range: bool,
        chunk: Option<usize>,
    ) -> (String, Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<String>>>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let agents = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&ranges);
        let seen_agents = Arc::clone(&agents);
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let range = Some(header(&req, "Range")).filter(|r| !r.is_empty());
                seen.lock().unwrap().push(range.clone().unwrap_or_default());
                seen_agents.lock().unwrap().push(header(&req, "User-Agent"));
                let start = range
                    .as_deref()
                    .and_then(|r| r.strip_prefix("bytes="))
                    .and_then(|r| r.trim_end_matches('-').parse::<usize>().ok())
                    .filter(|_| honour_range);
                let resp = match start {
                    Some(s) if s >= body.len() => {
                        tiny_http::Response::from_data(Vec::new()).with_status_code(416)
                    }
                    Some(s) => {
                        let end = chunk.map_or(body.len(), |c| (s + c).min(body.len()));
                        let header = tiny_http::Header::from_bytes(
                            &b"Content-Range"[..],
                            format!("bytes {s}-{}/{}", end.max(s + 1) - 1, body.len()).as_bytes(),
                        )
                        .unwrap();
                        tiny_http::Response::from_data(body[s..end].to_vec())
                            .with_status_code(206)
                            .with_header(header)
                    }
                    None => tiny_http::Response::from_data(
                        body[..chunk.map_or(body.len(), |c| c.min(body.len()))].to_vec(),
                    ),
                };
                let _ = req.respond(resp);
            }
        });
        (format!("http://127.0.0.1:{port}/file.bin"), ranges, agents)
    }

    /// A server that answers every request with `302 Found` to `target`
    /// (another host:port), as `github.com` does for release assets and
    /// `huggingface.co` for model files. Records Range and User-Agent.
    #[allow(clippy::type_complexity)]
    fn serve_redirect(target: String) -> (String, Arc<Mutex<Vec<(String, String)>>>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                log.lock()
                    .unwrap()
                    .push((header(&req, "Range"), header(&req, "User-Agent")));
                let location =
                    tiny_http::Header::from_bytes(&b"Location"[..], target.as_bytes()).unwrap();
                let _ = req.respond(
                    tiny_http::Response::from_data(Vec::new())
                        .with_status_code(302)
                        .with_header(location),
                );
            }
        });
        (
            format!("http://127.0.0.1:{port}/releases/download/v1/file.bin"),
            seen,
        )
    }

    /// A server that answers every Range request with `206` and the whole
    /// file from byte 0 (`Content-Range: bytes 0-...`), whatever was asked,
    /// and a request without Range with `200`. Records Range headers.
    fn serve_wrong_range(body: Vec<u8>) -> (String, Arc<Mutex<Vec<String>>>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&ranges);
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let range = header(&req, "Range");
                seen.lock().unwrap().push(range.clone());
                let resp = tiny_http::Response::from_data(body.clone());
                let resp = if range.is_empty() {
                    resp
                } else {
                    let cr = format!("bytes 0-{}/{}", body.len() - 1, body.len());
                    resp.with_status_code(206).with_header(
                        tiny_http::Header::from_bytes(&b"Content-Range"[..], cr.as_bytes())
                            .unwrap(),
                    )
                };
                let _ = req.respond(resp);
            }
        });
        (format!("http://127.0.0.1:{port}/file.bin"), ranges)
    }

    fn payload() -> Vec<u8> {
        (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect()
    }

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    /// A `.part` holding `bytes`, recorded as written by a download of `url`
    /// for `sha256` (what an interrupted [`download_file`] leaves).
    fn seed_part(dest: &Path, bytes: &[u8], url: &str, sha256: &str) {
        std::fs::write(part_path(dest), bytes).unwrap();
        PartSource::new(url, sha256).record(dest).unwrap();
    }

    /// Neither the `.part`, its source record nor its lock is left.
    fn assert_no_leftovers(dest: &Path) {
        for p in [
            part_path(dest),
            part_source_path(dest),
            suffixed(dest, ".part.lock"),
        ] {
            assert!(!p.exists(), "{} is left", p.display());
        }
    }

    #[test]
    fn range_start_reads_the_first_byte() {
        assert_eq!(range_start("bytes 100000-299999/300000"), Some(100_000));
        assert_eq!(range_start(" bytes 0-0/1"), Some(0));
        assert_eq!(range_start("bytes */300000"), None);
        assert_eq!(range_start("items 1-2/3"), None);
    }

    #[test]
    fn resumes_a_partial_download() {
        let body = payload();
        let (url, ranges) = serve(body.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        seed_part(&dest, &body[..100_000], &url, &sha(&body).to_uppercase());
        let mut last = 0;
        download_file(
            &url,
            &dest,
            &sha(&body),
            Some(body.len() as u64),
            &mut |n, _| last = n,
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_no_leftovers(&dest);
        assert_eq!(last, body.len() as u64);
        assert_eq!(ranges.lock().unwrap().as_slice(), ["bytes=100000-"]);
        // A second call finds the verified file and does nothing.
        download_file(
            &url,
            &dest,
            &sha(&body),
            Some(body.len() as u64),
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(ranges.lock().unwrap().len(), 1);
    }

    #[test]
    fn restarts_when_the_server_ignores_range() {
        let body = payload();
        let (url, ranges) = serve(body.clone(), false);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        seed_part(&dest, b"garbage that is not a prefix", &url, &sha(&body));
        download_file(&url, &dest, &sha(&body), None, &mut |_, _| {}).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(ranges.lock().unwrap().as_slice(), ["bytes=28-"]);
        assert_no_leftovers(&dest);
    }

    #[test]
    fn a_range_from_another_offset_restarts_the_file() {
        let body = payload();
        let (url, ranges) = serve_wrong_range(body.clone());
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        // Appending the 206 to these bytes would make a file of 400 kB.
        seed_part(&dest, &[0xAA; 100_000], &url, &sha(&body));
        download_file(
            &url,
            &dest,
            &sha(&body),
            Some(body.len() as u64),
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        // The 206 from byte 0 was not appended: the next request asked for
        // the whole file.
        assert_eq!(ranges.lock().unwrap().as_slice(), ["bytes=100000-", ""]);
        assert_no_leftovers(&dest);
    }

    #[test]
    fn a_part_from_another_url_or_hash_or_of_unknown_origin_restarts() {
        let body = payload();
        let (url, ranges) = serve(body.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let fetch = |dest: &Path| {
            download_file(
                &url,
                dest,
                &sha(&body),
                Some(body.len() as u64),
                &mut |_, _| {},
            )
            .unwrap();
            assert_eq!(std::fs::read(dest).unwrap(), body);
            assert_no_leftovers(dest);
        };
        // Bytes of another file, from another address.
        let other_url = dir.path().join("other-url.bin");
        seed_part(
            &other_url,
            &[0x55; 100_000],
            "https://elsewhere.example/file.bin",
            &sha(&body),
        );
        fetch(&other_url);
        // The same address, but written for another expected file.
        let other_hash = dir.path().join("other-hash.bin");
        seed_part(&other_hash, &[0x55; 100_000], &url, &"1".repeat(64));
        fetch(&other_hash);
        // No record at all (a `.part` left by 0.1.0-beta.4).
        let unknown = dir.path().join("unknown.bin");
        std::fs::write(part_path(&unknown), &body[..100_000]).unwrap();
        fetch(&unknown);
        // Every one started from byte 0.
        assert_eq!(ranges.lock().unwrap().as_slice(), ["", "", ""]);
    }

    #[test]
    fn a_second_download_of_the_same_file_says_it_is_already_downloading() {
        let body = payload();
        let (url, ranges) = serve(body.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        seed_part(&dest, &body[..1000], &url, &sha(&body));
        let fetch = || {
            download_file(
                &url,
                &dest,
                &sha(&body),
                Some(body.len() as u64),
                &mut |_, _| {},
            )
        };
        // Another program (the app, or `diarist-cli models pull`) holds it.
        let held = PartLock::acquire(&dest).unwrap();
        let err = fetch().unwrap_err().to_string();
        assert!(err.contains("out.bin is already downloading"), "{err}");
        assert!(ranges.lock().unwrap().is_empty());
        assert_eq!(std::fs::metadata(part_path(&dest)).unwrap().len(), 1000);
        drop(held);
        fetch().unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(ranges.lock().unwrap().as_slice(), ["bytes=1000-"]);
        assert_no_leftovers(&dest);
    }

    #[test]
    fn plain_http_is_refused_outside_the_tests() {
        let body = payload();
        let (url, ranges) = serve(body.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        TEST_ALLOW_HTTP.with(|a| a.set(false));
        let err = download_file(&url, &dest, &sha(&body), None, &mut |_, _| {});
        TEST_ALLOW_HTTP.with(|a| a.set(true));
        // ureq checks every hop, so a redirect to http ends the same way.
        let err = err.unwrap_err().to_string();
        assert!(err.contains("https only"), "{err}");
        assert!(ranges.lock().unwrap().is_empty());
        assert!(!dest.exists());
        assert_no_leftovers(&dest);
    }

    #[test]
    fn continues_after_short_transfers_and_reports_a_stall() {
        let body = payload();
        // Every response carries at most 70 kB: five requests finish the file.
        let (url, ranges) = serve_with(body.clone(), true, Some(70_000));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        download_file(
            &url,
            &dest,
            &sha(&body),
            Some(body.len() as u64),
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        let seen = ranges.lock().unwrap().clone();
        assert_eq!(seen.len(), 5, "{seen:?}");
        assert_eq!(seen[1], "bytes=70000-");

        // A server that sends nothing: three empty answers end it as stalled,
        // and the partial file stays for the next try.
        let (url, _) = serve_with(body.clone(), true, Some(0));
        let dest = dir.path().join("stalled.bin");
        seed_part(&dest, &body[..1000], &url, &sha(&body));
        let err = download_file(
            &url,
            &dest,
            &sha(&body),
            Some(body.len() as u64),
            &mut |_, _| {},
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("stalled at 1000 bytes"), "{err}");
        assert_eq!(std::fs::metadata(part_path(&dest)).unwrap().len(), 1000);
        // The record stays with it, so the next try resumes; the lock goes.
        assert!(PartSource::new(&url, &sha(&body)).wrote(&dest));
        assert!(!suffixed(&dest, ".part.lock").exists());
    }

    #[test]
    fn installs_a_zip_dropped_into_the_engine_folder() {
        let body = engine_zip();
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        let entry = EngineEntry {
            name: "nemo-speech-9.9.9-test".into(),
            version: "9.9.9".into(),
            backend: "cuda".into(),
            url: "http://127.0.0.1:9/never-used".into(),
            sha256: sha(&body),
            size_bytes: body.len() as u64,
            licence: "Apache-2.0".into(),
            tested: false,
        };
        let manifest = Manifest {
            models: Vec::new(),
            engines: vec![entry.clone()],
        };
        std::fs::create_dir_all(paths.engine()).unwrap();
        // Saved by a browser under another name.
        std::fs::write(paths.engine().join("download (1).zip"), &body).unwrap();
        let r = install_dropped_engines(&paths, &manifest);
        assert_eq!(r.len(), 1);
        let dir_installed = r.into_iter().next().unwrap().unwrap();
        assert!(process::is_engine_dir(&dir_installed));
        assert!(!paths.engine().join("download (1).zip").exists());
        assert!(!paths.engine().join("downloads").exists());
        assert!(
            install_dropped_engines(&paths, &manifest).is_empty(),
            "already installed"
        );

        // A zip with the right size but other bytes is reported and left alone.
        let other = DataPaths::at(dir.path().join("other"));
        std::fs::create_dir_all(other.engine()).unwrap();
        let mut wrong = body.clone();
        wrong[10] ^= 0xff;
        std::fs::write(other.engine().join("nemo-speech-9.9.9-test.zip"), &wrong).unwrap();
        let r = install_dropped_engines(&other, &manifest);
        let err = r.into_iter().next().unwrap().unwrap_err().to_string();
        assert!(err.contains("SHA-256"), "{err}");
        assert!(other.engine().join("nemo-speech-9.9.9-test.zip").is_file());
    }

    #[test]
    fn rejects_a_sha_mismatch() {
        let body = payload();
        let (url, _) = serve(body.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        let err = download_file(&url, &dest, &"0".repeat(64), None, &mut |_, _| {})
            .unwrap_err()
            .to_string();
        assert!(err.contains("SHA-256"), "{err}");
        assert!(!dest.exists());
        assert_no_leftovers(&dest);
    }

    #[test]
    fn resumes_across_a_redirect_to_another_host() {
        let body = payload();
        // The asset host cuts every answer at 70 kB, so the download needs
        // several requests, each sent to the original URL and redirected.
        let (asset, asset_ranges, asset_agents) = serve_recording(body.clone(), true, Some(70_000));
        let (url, redirects) = serve_redirect(format!("{asset}?signature=abc"));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        seed_part(&dest, &body[..100_000], &url, &sha(&body));
        download_file(
            &url,
            &dest,
            &sha(&body),
            Some(body.len() as u64),
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        let want = ["bytes=100000-", "bytes=170000-", "bytes=240000-"];
        // The Range header survives the redirect: the asset host resumes.
        assert_eq!(asset_ranges.lock().unwrap().as_slice(), want);
        let seen = redirects.lock().unwrap().clone();
        assert_eq!(
            seen.iter().map(|(r, _)| r.as_str()).collect::<Vec<_>>(),
            want
        );
        // Both hosts see the app's own User-Agent and nothing else.
        let ua = format!("diarist/{}", crate::VERSION);
        assert!(seen.iter().all(|(_, a)| *a == ua), "{seen:?}");
        let agents = asset_agents.lock().unwrap().clone();
        assert!(agents.iter().all(|a| *a == ua), "{agents:?}");
    }

    /// A manifest model entry as `models.json` writes it (fields added
    /// later take their serde defaults).
    fn model_entry(url: &str, body: &[u8], direct: bool) -> ModelEntry {
        serde_json::from_value(serde_json::json!({
            "name": "test-asr",
            "pull_name": if direct { "" } else { "test-asr" },
            "role": "asr",
            "direct": direct,
            "repo": "nvidia/Test-ASR",
            "revision": "0123456789abcdef0123456789abcdef01234567",
            "file": "test-asr.q8_0.gguf",
            "url": url,
            "size_bytes": body.len(),
            "sha256": sha(body),
            "licence": "test",
            "languages": ["en"],
            "file_right_context": 13
        }))
        .unwrap()
    }

    /// A valid GGUF header followed by filler, big enough to resume.
    fn gguf_body() -> Vec<u8> {
        use super::super::gguf::{build, GgufValue};
        let mut body = build::header(
            3,
            0,
            &[
                ("general.architecture", GgufValue::Str("asr".into())),
                ("general.name", GgufValue::Str("test-asr".into())),
                ("asr.head_type", GgufValue::Str("rnnt".into())),
            ],
        );
        body.extend(payload());
        body
    }

    #[test]
    fn downloads_a_model_into_the_manifest_layout_and_the_scanner_lists_it() {
        use crate::store::{MemoryStore, Store};
        use crate::types::ModelSource;
        let body = gguf_body();
        let (asset, ranges) = serve(body.clone(), true);
        let (url, _) = serve_redirect(asset);
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path().join("root"));
        let entry = model_entry(&url, &body, false);
        let dest = entry.dest_dir(&paths.models()).join(&entry.file);
        // `<models>\<repo>\<revision>\<file>`, where `nemo-speech pull` put it.
        assert_eq!(
            dest,
            paths
                .models()
                .join("nvidia")
                .join("Test-ASR")
                .join("0123456789abcdef0123456789abcdef01234567")
                .join("test-asr.q8_0.gguf")
        );
        // An interrupted earlier download resumes. The `.partial` an
        // interrupted `nemo-speech pull` left goes once the model is there.
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        seed_part(&dest, &body[..50_000], &entry.url, &entry.sha256);
        let curl_partial = suffixed(&dest, ".partial");
        std::fs::write(&curl_partial, &body[..20_000]).unwrap();
        let mut last = 0;
        let got = fetch_entry(&entry, &paths, &mut |n, _| last = n).unwrap();
        assert_eq!(got, dest);
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(last, body.len() as u64);
        assert_eq!(ranges.lock().unwrap().as_slice(), ["bytes=50000-"]);
        assert_no_leftovers(&dest);
        assert!(!curl_partial.exists());
        let mut side = dest.as_os_str().to_owned();
        side.push(".sha256");
        let cached = std::fs::read_to_string(PathBuf::from(side)).unwrap();
        assert_eq!(cached, format!("{}  test-asr.q8_0.gguf\n", sha(&body)));

        // The scanner takes it as the manifest's model.
        let store = MemoryStore::new();
        let manifest = Manifest {
            models: vec![entry.clone()],
            engines: Vec::new(),
        };
        let r = super::super::scan::scan(&paths, &store, &manifest).unwrap();
        let f = r
            .files
            .iter()
            .find(|f| f.path.ends_with("test-asr.q8_0.gguf"))
            .unwrap_or_else(|| panic!("{r:?}"));
        assert_eq!(f.manifest_name.as_deref(), Some("test-asr"));
        assert_eq!(f.source, ModelSource::Manifest);
        assert_eq!(f.capabilities.file_right_context, Some(13));
        assert_eq!(store.list_model_files().unwrap().len(), 1);

        // Present and verified: a second call fetches nothing.
        fetch_entry(&entry, &paths, &mut |_, _| {}).unwrap();
        assert_eq!(ranges.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_direct_model_lands_in_the_user_folder_and_a_wrong_hash_is_refused() {
        let body = gguf_body();
        let (url, _) = serve(body.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        let entry = model_entry(&url, &body, true);
        let got = fetch_entry(&entry, &paths, &mut |_, _| {}).unwrap();
        assert_eq!(got, entry.dest_dir(&paths.models()).join(&entry.file));
        assert_eq!(got, paths.models().join("user").join("test-asr.q8_0.gguf"));

        let mut wrong = model_entry(&url, &body, false);
        wrong.sha256 = "0".repeat(64);
        let dest = wrong.dest_dir(&paths.models()).join(&wrong.file);
        let err = fetch_entry(&wrong, &paths, &mut |_, _| {})
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not match the expected"), "{err}");
        assert!(!dest.exists());
        assert_no_leftovers(&dest);
        let mut side = dest.as_os_str().to_owned();
        side.push(".sha256");
        assert!(!PathBuf::from(side).exists());

        // An entry without an address is refused before any request.
        let mut no_url = model_entry("", &body, false);
        no_url.url = String::new();
        let err = fetch_entry(&no_url, &paths, &mut |_, _| {})
            .unwrap_err()
            .to_string();
        assert!(err.contains("test-asr: url: empty"), "{err}");
    }

    fn engine_zip() -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            z.add_directory("nemo-speech-9.9.9-test/bin/", opts)
                .unwrap();
            z.start_file("nemo-speech-9.9.9-test/bin/nemo-speech.exe", opts)
                .unwrap();
            z.write_all(b"MZ fake binary").unwrap();
            z.start_file("nemo-speech-9.9.9-test/share/readme.txt", opts)
                .unwrap();
            z.write_all(b"hello").unwrap();
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn installs_an_engine_zip() {
        let body = engine_zip();
        let (url, _) = serve(body.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        let entry = EngineEntry {
            name: "nemo-speech-9.9.9-test".into(),
            version: "9.9.9".into(),
            backend: "cuda".into(),
            url,
            sha256: sha(&body),
            size_bytes: body.len() as u64,
            licence: "Apache-2.0".into(),
            tested: false,
        };
        let installed = install_engine(&entry, &paths, &mut |_, _| {}).unwrap();
        assert_eq!(installed, paths.engine().join("nemo-speech-9.9.9-test"));
        assert!(process::is_engine_dir(&installed));
        assert_eq!(
            std::fs::read_to_string(installed.join("share").join("readme.txt")).unwrap(),
            "hello"
        );
        assert!(!paths.engine().join("nemo-speech-9.9.9-test.tmp").exists());
        assert!(!paths
            .engine()
            .join("downloads")
            .join("nemo-speech-9.9.9-test.zip")
            .exists());
    }
}
