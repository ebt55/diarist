//! Fetching a manifest entry with all of its files (docs/plans/16 section
//! 3.6): the desktop app's `pull_model` and `diarist-cli models pull`.
//!
//! Every file goes through [`download_file`] (resume, size, SHA-256, the
//! generic User-Agent) and gets the `<file>.sha256` cache the scanner reads.
//! The other files come first and the primary file last, so the scanner,
//! which keys on the primary file, never lists a model with a part missing;
//! a failed download leaves the verified files in place and the next call
//! fetches only what is missing. A single-file entry is one call of
//! [`download_file`]. This is the only model downloader since 0.1.0-beta.5;
//! the `<file>.partial` an interrupted `nemo-speech pull` left is deleted
//! once its file is in place.

use std::path::{Path, PathBuf};

use super::download::{download_file, record_sha256, sha256_file, Progress};
use super::manifest::ModelEntry;
use super::paths::DataPaths;
use crate::error::{CoreError, Result};
use crate::punct::spec::rel_path;

/// Downloads every file of `entry` into [`ModelEntry::dest_dir`] and returns
/// the primary file. `progress` gets the bytes of all files so far and the
/// entry's total.
pub fn fetch_entry(
    entry: &ModelEntry,
    paths: &DataPaths,
    progress: Progress<'_>,
) -> Result<PathBuf> {
    entry
        .validate()
        .map_err(|e| CoreError::Models(format!("{}: {e}", entry.name)))?;
    let root = entry.dest_dir(&paths.models());
    let total = Some(entry.size_bytes).filter(|s| *s > 0);
    let mut before = 0u64;
    let mut one = |path: &str, url: &str, size: u64, sha: &str, before: u64| -> Result<PathBuf> {
        if url.trim().is_empty() {
            return Err(CoreError::Models(format!(
                "{} has no download address for {path} in the manifest; put it into {} by hand",
                entry.name,
                root.display()
            )));
        }
        let dest = rel_path(&root, path);
        download_file(
            url,
            &dest,
            sha,
            Some(size).filter(|s| *s > 0),
            &mut |d, _| progress(before + d, total),
        )?;
        record(&dest, sha)?;
        // The `<file>.partial` an interrupted `nemo-speech pull` (up to
        // 0.1.0-beta.4) left goes once the file is in place; nothing resumes it.
        let mut partial = dest.as_os_str().to_owned();
        partial.push(".partial");
        let _ = std::fs::remove_file(PathBuf::from(partial));
        Ok(dest)
    };
    for f in &entry.files {
        one(&f.path, &f.url, f.size_bytes, &f.sha256, before)?;
        before += f.size_bytes;
    }
    one(
        &entry.file,
        &entry.url,
        entry.primary_size_bytes(),
        &entry.sha256,
        before,
    )
}

/// The `.sha256` cache of a verified file (computed when the entry gives no
/// hash, as for an "Add from URL" without one).
fn record(dest: &Path, sha: &str) -> Result<()> {
    let sha = if sha.trim().is_empty() {
        sha256_file(dest)?
    } else {
        sha.to_owned()
    };
    record_sha256(dest, &sha);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn sha(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Serves `files` (path -> body) and records the requested paths.
    fn serve(files: Vec<(&'static str, Vec<u8>)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let url = req.url().to_owned();
                log.lock().unwrap().push(url.clone());
                let resp = match files.iter().find(|(p, _)| url == format!("/{p}")) {
                    Some((_, body)) => tiny_http::Response::from_data(body.clone()),
                    None => tiny_http::Response::from_data(Vec::new()).with_status_code(404),
                };
                let _ = req.respond(resp);
            }
        });
        (format!("http://127.0.0.1:{port}"), seen)
    }

    fn entry(base: &str, model: &[u8], tok: &[u8], dll: &[u8]) -> ModelEntry {
        serde_json::from_value(serde_json::json!({
            "name": "punct", "role": "punctuation", "direct": true,
            "file": "onnx/model.onnx", "url": format!("{base}/model.onnx"),
            "size_bytes": model.len() + tok.len() + dll.len(), "sha256": sha(model),
            "files": [
                {"path": "tokenizer.json", "url": format!("{base}/tokenizer.json"),
                 "size_bytes": tok.len(), "sha256": sha(tok)},
                {"path": "diarist-onnxruntime.dll", "url": format!("{base}/onnxruntime.dll"),
                 "size_bytes": dll.len(), "sha256": sha(dll)}
            ],
            "dir": "Horizon-Labs/punct/rev"
        }))
        .unwrap()
    }

    #[test]
    fn fetches_the_other_files_first_and_the_primary_last() {
        let (model, tok, dll) = (vec![1u8; 3000], vec![2u8; 200], vec![3u8; 100]);
        let (base, seen) = serve(vec![
            ("model.onnx", model.clone()),
            ("tokenizer.json", tok.clone()),
            ("onnxruntime.dll", dll.clone()),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        let e = entry(&base, &model, &tok, &dll);
        let mut last = (0, None);
        let got = fetch_entry(&e, &paths, &mut |d, t| last = (d, t)).unwrap();
        let root = paths
            .models()
            .join("Horizon-Labs")
            .join("punct")
            .join("rev");
        assert_eq!(got, root.join("onnx").join("model.onnx"));
        assert_eq!(
            *seen.lock().unwrap(),
            ["/tokenizer.json", "/onnxruntime.dll", "/model.onnx"]
        );
        assert_eq!(last, (3300, Some(3300)), "progress over every file");
        for (p, body) in [
            (root.join("tokenizer.json"), &tok),
            (root.join("diarist-onnxruntime.dll"), &dll),
            (got.clone(), &model),
        ] {
            assert_eq!(sha256_file(&p).unwrap(), sha(body));
            let mut side = p.as_os_str().to_owned();
            side.push(".sha256");
            assert!(std::fs::read_to_string(PathBuf::from(side))
                .unwrap()
                .starts_with(&sha(body)));
        }
        // Everything there: nothing is fetched again.
        seen.lock().unwrap().clear();
        fetch_entry(&e, &paths, &mut |_, _| {}).unwrap();
        assert!(seen.lock().unwrap().is_empty());
    }

    /// The shipped punctuation entry with every address on `base` and stand-in
    /// bytes (`body`) for each file, keeping its folder, file names and
    /// order: the layout and the rule of the real entry, without its 306 MB.
    fn shipped_punct_with(base: &str, body: &dyn Fn(&str) -> Vec<u8>) -> ModelEntry {
        let mut e = crate::models::Manifest::builtin()
            .model("punctuation-horizon-small")
            .unwrap()
            .clone();
        for f in &mut e.files {
            let b = body(&f.path);
            f.url = format!("{base}/{}", f.path);
            f.size_bytes = b.len() as u64;
            f.sha256 = sha(&b);
        }
        let b = body(&e.file);
        e.url = format!("{base}/{}", e.file);
        e.sha256 = sha(&b);
        e.size_bytes = b.len() as u64 + e.files.iter().map(|f| f.size_bytes).sum::<u64>();
        e
    }

    /// A beta.5 install (docs/plans/18 section 8.1 items 1 and 6): the model
    /// and tokenizer of ee31fdf, which v1.1.1 left byte-identical, in the
    /// same folder (the entry's `dir` has no revision), with beta.5's
    /// `LICENSE-Apache-2.0.txt` and without `LICENSE` and `NOTICE`. The next
    /// fetch downloads only those two; the model, the tokenizer and the
    /// runtime are never requested, and the old licence copy is left alone.
    #[test]
    fn a_beta5_install_gets_only_the_two_new_licence_files() {
        let body = |path: &str| format!("bytes of {path}").into_bytes();
        let names: Vec<String> = {
            let e = crate::models::Manifest::builtin()
                .model("punctuation-horizon-small")
                .unwrap()
                .clone();
            e.files.iter().map(|f| f.path.clone()).collect()
        };
        let served: Vec<(&'static str, Vec<u8>)> = names
            .iter()
            .map(|p| p.as_str())
            .chain(["onnx/model_quantized.onnx"])
            .map(|p| (&*String::leak(p.to_owned()), body(p)))
            .collect();
        let (base, seen) = serve(served);
        let e = shipped_punct_with(&base, &body);
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        let root = e.dest_dir(&paths.models());
        assert_eq!(
            root,
            paths
                .models()
                .join("horizon-labs")
                .join("punctuation-restoration-small")
        );
        // What beta.5 put there: every file of its list, verified.
        let beta5 = [
            "tokenizer.json",
            "LICENSE-Apache-2.0.txt",
            "NOTICE-punctuation.txt",
            "onnxruntime-LICENSE.txt",
            "onnxruntime-ThirdPartyNotices.txt",
            "diarist-onnxruntime.dll",
            "onnx/model_quantized.onnx",
        ];
        for p in beta5 {
            let dest = rel_path(&root, p);
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::write(&dest, body(p)).unwrap();
            record_sha256(&dest, &sha(&body(p)));
        }
        let model = root.join("onnx").join("model_quantized.onnx");
        let before = std::fs::metadata(&model).unwrap().modified().unwrap();

        let mut last = (0, None);
        let got = fetch_entry(&e, &paths, &mut |d, t| last = (d, t)).unwrap();
        assert_eq!(got, model);
        assert_eq!(*seen.lock().unwrap(), ["/LICENSE", "/NOTICE"]);
        assert_eq!(last.1, Some(e.size_bytes));
        assert_eq!(
            std::fs::metadata(&model).unwrap().modified().unwrap(),
            before,
            "the model file is not rewritten"
        );
        for p in ["LICENSE", "NOTICE"] {
            assert_eq!(std::fs::read(root.join(p)).unwrap(), body(p), "{p}");
        }
        assert!(
            root.join("LICENSE-Apache-2.0.txt").is_file(),
            "a file the list no longer names stays"
        );
        // Everything there now: nothing is fetched again.
        seen.lock().unwrap().clear();
        fetch_entry(&e, &paths, &mut |_, _| {}).unwrap();
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn a_bad_file_stops_before_the_primary() {
        let (model, tok, dll) = (vec![1u8; 300], vec![2u8; 20], vec![3u8; 10]);
        // The runtime is served with other bytes.
        let (base, seen) = serve(vec![
            ("model.onnx", model.clone()),
            ("tokenizer.json", tok.clone()),
            ("onnxruntime.dll", vec![9u8; 10]),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        let e = entry(&base, &model, &tok, &dll);
        let err = fetch_entry(&e, &paths, &mut |_, _| {})
            .unwrap_err()
            .to_string();
        assert!(err.contains("SHA-256"), "{err}");
        assert!(!seen.lock().unwrap().iter().any(|u| u == "/model.onnx"));
        let root = e.dest_dir(&paths.models());
        assert!(root.join("tokenizer.json").is_file(), "verified files stay");
        assert!(!root.join("onnx").join("model.onnx").exists());
        // An entry with a path out of its folder is refused before any request.
        let mut bad = e.clone();
        bad.files[0].path = "../../evil.dll".into();
        seen.lock().unwrap().clear();
        let err = fetch_entry(&bad, &paths, &mut |_, _| {})
            .unwrap_err()
            .to_string();
        assert!(err.contains("files[0].path"), "{err}");
        assert!(seen.lock().unwrap().is_empty());
    }
}
