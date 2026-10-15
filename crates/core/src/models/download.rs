//! HTTP download with resume and SHA-256 verification, and the engine zip
//! install (`docs/01`, "Install model").
//!
//! - ureq with rustls and the platform verifier (Windows certificate store),
//!   proxies from the environment.
//! - Partial data lives in `<dest>.part`; a later call resumes with a Range
//!   request (a server that ignores it restarts the file).
//! - The whole file is hashed before it is renamed into place; a mismatch
//!   deletes the partial file.
//! - The engine zip is extracted into `engine\<name>.tmp` and renamed to
//!   `engine\<name>` (a single top-level folder in the zip is stripped).

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::manifest::EngineEntry;
use super::paths::DataPaths;
use crate::engine::process;
use crate::error::{CoreError, Result};

fn dl_err(msg: impl Into<String>) -> CoreError {
    CoreError::Models(msg.into())
}

/// Progress callback: (bytes on disk, total bytes when known).
pub type Progress<'a> = &'a mut dyn FnMut(u64, Option<u64>);

fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
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

fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

/// Downloads `url` to `dest`, resuming `<dest>.part` when present, and
/// verifies size and SHA-256 before the atomic rename. An existing `dest`
/// with the right hash is kept as is.
pub fn download_file(
    url: &str,
    dest: &Path,
    sha256: &str,
    size: Option<u64>,
    progress: Progress<'_>,
) -> Result<()> {
    if dest.is_file() && verify_file(dest, size, sha256).is_ok() {
        return Ok(());
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = part_path(dest);
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if size.is_some_and(|s| have > s) {
        std::fs::remove_file(&part)?;
        have = 0;
    }
    let agent = agent();
    if size != Some(have) || have == 0 {
        let mut req = agent.get(url);
        if have > 0 {
            req = req.header("Range", format!("bytes={have}-"));
        }
        let mut resp = req.call().map_err(|e| dl_err(format!("GET {url}: {e}")))?;
        let status = resp.status().as_u16();
        let append = match status {
            206 => true,
            200 => false,
            416 if have > 0 => true, // already complete
            _ => return Err(dl_err(format!("GET {url}: HTTP {status}"))),
        };
        if status != 416 {
            if !append {
                have = 0;
            }
            let mut out = OpenOptions::new()
                .create(true)
                .write(true)
                .append(append)
                .truncate(!append)
                .open(&part)?;
            let mut reader = resp.body_mut().as_reader();
            let mut buf = vec![0u8; 256 * 1024];
            progress(have, size);
            loop {
                let n = reader
                    .read(&mut buf)
                    .map_err(|e| dl_err(format!("download {url}: {e} (resume by retrying)")))?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
                have += n as u64;
                progress(have, size);
            }
            out.flush()?;
        }
    }
    if let Err(e) = verify_file(&part, size, sha256) {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    std::fs::rename(&part, dest)
        .map_err(|e| dl_err(format!("rename to {}: {e}", dest.display())))?;
    Ok(())
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
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Serves `body` on 127.0.0.1 with Range support; records Range headers.
    fn serve(body: Vec<u8>, honour_range: bool) -> (String, Arc<Mutex<Vec<String>>>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&ranges);
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let range = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Range"))
                    .map(|h| h.value.as_str().to_owned());
                seen.lock().unwrap().push(range.clone().unwrap_or_default());
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
                        let header = tiny_http::Header::from_bytes(
                            &b"Content-Range"[..],
                            format!("bytes {s}-{}/{}", body.len() - 1, body.len()).as_bytes(),
                        )
                        .unwrap();
                        tiny_http::Response::from_data(body[s..].to_vec())
                            .with_status_code(206)
                            .with_header(header)
                    }
                    None => tiny_http::Response::from_data(body.clone()),
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

    #[test]
    fn resumes_a_partial_download() {
        let body = payload();
        let (url, ranges) = serve(body.clone(), true);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        std::fs::write(part_path(&dest), &body[..100_000]).unwrap();
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
        assert!(!part_path(&dest).exists());
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
        let (url, _) = serve(body.clone(), false);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        std::fs::write(part_path(&dest), b"garbage that is not a prefix").unwrap();
        download_file(&url, &dest, &sha(&body), None, &mut |_, _| {}).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
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
        assert!(!dest.exists() && !part_path(&dest).exists());
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
