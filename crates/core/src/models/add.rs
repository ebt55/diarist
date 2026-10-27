//! Models the app has never heard of (Settings, Models): "Add from URL"
//! downloads a direct link into `models\user\` (resumable, the hash checked
//! when one is given, recorded either way), "Add from file" copies or
//! hard-links a chosen file there. The scanner then classifies the file
//! from its GGUF header like anything else in `models\`.

use std::path::{Path, PathBuf};

use super::download::{download_file, record_sha256, sha256_file, Progress};
use super::paths::DataPaths;
use crate::error::{CoreError, Result};

/// Where added models go.
pub fn user_dir(paths: &DataPaths) -> PathBuf {
    paths.models().join("user")
}

/// A safe file name from the last path segment of `url` (query and
/// fragment dropped; characters Windows forbids replaced).
pub fn file_name_from_url(url: &str) -> Result<String> {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    // The downloader is https only (`download::https_only`); say so here
    // rather than fail in the request.
    if !path.starts_with("https://") {
        return Err(CoreError::Models(format!(
            "{url} is not an https link; Diarist downloads over https only, so paste the \
             file's direct https address"
        )));
    }
    let after_host = path
        .split_once("://")
        .and_then(|(_, rest)| rest.split_once('/'))
        .map_or("", |(_, p)| p);
    let last = after_host.rsplit('/').next().unwrap_or("");
    let decoded = last.replace("%20", " ");
    let name: String = decoded
        .chars()
        .map(|c| {
            if r#"<>:"/\|?*"#.contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = name.trim().trim_matches('.').to_owned();
    if name.is_empty() || !name.contains('.') {
        return Err(CoreError::Models(format!(
            "{url} does not end in a file name; use the direct link to the .gguf file"
        )));
    }
    Ok(name)
}

/// Downloads `url` into `models\user\`; with `sha256` the file must match
/// it. A partial download resumes on the next call. Returns the file.
pub fn add_from_url(
    paths: &DataPaths,
    url: &str,
    sha256: Option<&str>,
    progress: Progress<'_>,
) -> Result<PathBuf> {
    let name = file_name_from_url(url)?;
    let sha = sha256
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty());
    if let Some(s) = &sha {
        if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(CoreError::Models(
                "the SHA-256 must be 64 hexadecimal characters; leave it empty to skip the check"
                    .into(),
            ));
        }
    }
    let dest = user_dir(paths).join(&name);
    download_file(url, &dest, sha.as_deref().unwrap_or(""), None, progress)?;
    let got = match sha {
        Some(s) => s,
        None => sha256_file(&dest)?,
    };
    record_sha256(&dest, &got);
    Ok(dest)
}

/// Copies `src` into `models\user\` (or hard-links it with `link`, which
/// needs the same drive and takes no space). Returns the new file.
pub fn add_from_file(paths: &DataPaths, src: &Path, link: bool) -> Result<PathBuf> {
    if !src.is_file() {
        return Err(CoreError::Models(format!(
            "{} is not a file; choose the .gguf file itself",
            src.display()
        )));
    }
    let name = src
        .file_name()
        .ok_or_else(|| CoreError::Models(format!("{} has no file name", src.display())))?;
    let dir = user_dir(paths);
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(name);
    if dest.exists() {
        return Err(CoreError::Models(format!(
            "{} already exists; rename one of them, or press Refresh to use the one that is there",
            dest.display()
        )));
    }
    if link {
        std::fs::hard_link(src, &dest).map_err(|e| {
            CoreError::Models(format!(
                "cannot link {} into {} ({e}); links only work on the same drive, so copy it instead",
                src.display(),
                dir.display()
            ))
        })?;
    } else {
        std::fs::copy(src, &dest).map_err(|e| {
            CoreError::Models(format!(
                "cannot copy {} into {} ({e}); check the free space and try again",
                src.display(),
                dir.display()
            ))
        })?;
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_urls() {
        assert_eq!(
            file_name_from_url(
                "https://huggingface.co/x/y/resolve/abc/model.q8_0.gguf?download=true"
            )
            .unwrap(),
            "model.q8_0.gguf"
        );
        assert_eq!(
            file_name_from_url("https://h/a%20b.gguf").unwrap(),
            "a b.gguf"
        );
        assert!(file_name_from_url("ftp://x/y.gguf").is_err());
        let err = file_name_from_url("http://example.com/model.gguf")
            .unwrap_err()
            .to_string();
        assert!(err.contains("https only"), "{err}");
        assert!(file_name_from_url("https://example.com/").is_err());
        assert!(file_name_from_url("https://example.com/folder").is_err());
    }

    #[test]
    fn copies_and_links_files_in() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path().join("root"));
        let src = dir.path().join("mine.gguf");
        std::fs::write(&src, b"GGUF fake").unwrap();
        let copied = add_from_file(&paths, &src, false).unwrap();
        assert_eq!(copied, user_dir(&paths).join("mine.gguf"));
        assert_eq!(std::fs::read(&copied).unwrap(), b"GGUF fake");
        let again = add_from_file(&paths, &src, false).unwrap_err().to_string();
        assert!(again.contains("already exists"), "{again}");
        std::fs::remove_file(&copied).unwrap();
        let linked = add_from_file(&paths, &src, true).unwrap();
        assert_eq!(std::fs::read(&linked).unwrap(), b"GGUF fake");
        assert!(add_from_file(&paths, dir.path(), false).is_err());
    }

    #[test]
    fn rejects_a_malformed_hash_before_downloading() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        let err = add_from_url(
            &paths,
            "https://127.0.0.1:9/x.gguf",
            Some("abc"),
            &mut |_, _| {},
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("64 hexadecimal"), "{err}");
    }
}
