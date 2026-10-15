//! "Check for newer runtime": the NeMo-Speech.cpp releases on GitHub,
//! through the unauthenticated API (60 requests an hour per address),
//! Windows zips only. A chosen package is installed like the pinned one
//! (`download::install_engine`) into `engine\<zip name>\`, with the SHA-256
//! from the release's `<zip>.sha256` asset. Versions outside
//! [`super::TESTED_ENGINE_VERSIONS`] carry a warning, nothing more.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::manifest::{EngineEntry, TESTED_ENGINE_VERSIONS};
use crate::error::{CoreError, Result};

/// The releases API of the runtime.
pub const RELEASES_API: &str =
    "https://api.github.com/repos/NVIDIA/NeMo-Speech.cpp/releases?per_page=10";
/// Where a person can look at the releases.
pub const RELEASES_PAGE: &str = "https://github.com/NVIDIA/NeMo-Speech.cpp/releases";

/// One Windows runtime zip of a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RuntimePackage {
    /// The zip's name without `.zip`, which is also the folder it is
    /// extracted to, e.g. `nemo-speech-0.2.0-windows-x86_64-vulkan`.
    pub name: String,
    pub version: String,
    /// `cuda`, `vulkan` or `cpu`.
    pub backend: String,
    pub size_bytes: u64,
    pub url: String,
    /// The release's `<zip>.sha256` asset.
    pub sha_url: Option<String>,
    /// A folder of that name is under `engine\` (filled in by the caller).
    pub installed: bool,
}

/// One release with its Windows packages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct RuntimeRelease {
    pub tag: String,
    pub version: String,
    pub prerelease: bool,
    pub published_at: String,
    /// The version is one this build was tested with.
    pub tested: bool,
    pub packages: Vec<RuntimePackage>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

/// `nemo-speech-<version>-windows-x86_64-<backend>.zip` -> (version, backend).
fn parse_asset_name(name: &str) -> Option<(String, String)> {
    let stem = name.strip_suffix(".zip")?.strip_prefix("nemo-speech-")?;
    let (version, backend) = stem.rsplit_once("-windows-x86_64-")?;
    (!version.is_empty() && !backend.is_empty()).then(|| (version.to_owned(), backend.to_owned()))
}

/// The releases API answer as releases with their Windows packages
/// (drafts and releases without Windows zips left out), in API order
/// (newest first).
pub fn parse_releases(json: &str) -> Result<Vec<RuntimeRelease>> {
    let raw: Vec<GhRelease> = serde_json::from_str(json)
        .map_err(|e| CoreError::Models(format!("GitHub releases answer: {e}")))?;
    let mut out = Vec::new();
    for r in raw.into_iter().filter(|r| !r.draft) {
        let mut packages = Vec::new();
        for a in &r.assets {
            let Some((version, backend)) = parse_asset_name(&a.name) else {
                continue;
            };
            let sha_name = format!("{}.sha256", a.name);
            packages.push(RuntimePackage {
                name: a.name.trim_end_matches(".zip").to_owned(),
                version,
                backend,
                size_bytes: a.size,
                url: a.browser_download_url.clone(),
                sha_url: r
                    .assets
                    .iter()
                    .find(|x| x.name == sha_name)
                    .map(|x| x.browser_download_url.clone()),
                installed: false,
            });
        }
        if packages.is_empty() {
            continue;
        }
        let version = packages[0].version.clone();
        out.push(RuntimeRelease {
            tested: TESTED_ENGINE_VERSIONS.contains(&version.as_str()),
            tag: r.tag_name,
            version,
            prerelease: r.prerelease,
            published_at: r.published_at.unwrap_or_default(),
            packages,
        });
    }
    Ok(out)
}

fn get_text(url: &str) -> Result<String> {
    let agent = super::download::agent(Some(std::time::Duration::from_secs(30)));
    let mut resp = agent
        .get(url)
        .header("User-Agent", format!("diarist/{}", crate::VERSION))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| {
            CoreError::Models(format!(
                "GET {url}: {e}. Check the connection (a firewall may need to allow diarist.exe), \
                 or download a zip from {RELEASES_PAGE} and put it into the engine folder"
            ))
        })?;
    let status = resp.status().as_u16();
    if status != 200 {
        return Err(CoreError::Models(format!(
            "GET {url}: HTTP {status}{}. Try again later, or download a zip from {RELEASES_PAGE} \
             and put it into the engine folder",
            if status == 403 || status == 429 {
                " (GitHub allows 60 requests an hour without an account)"
            } else {
                ""
            }
        )));
    }
    resp.body_mut()
        .read_to_string()
        .map_err(|e| CoreError::Models(format!("GET {url}: {e}")))
}

/// The runtime releases on GitHub, newest first.
pub fn fetch_releases() -> Result<Vec<RuntimeRelease>> {
    parse_releases(&get_text(RELEASES_API)?)
}

/// The SHA-256 in a `<zip>.sha256` asset (`<hex>  <name>`).
pub fn fetch_sha(url: &str) -> Result<String> {
    let text = get_text(url)?;
    text.split_whitespace()
        .next()
        .map(str::to_ascii_lowercase)
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| CoreError::Models(format!("{url} holds no SHA-256")))
}

/// The manifest-style entry for a package, with its fetched hash.
pub fn package_entry(p: &RuntimePackage, sha256: String) -> EngineEntry {
    EngineEntry {
        name: p.name.clone(),
        version: p.version.clone(),
        backend: p.backend.clone(),
        url: p.url.clone(),
        sha256,
        size_bytes: p.size_bytes,
        licence: "Apache-2.0".into(),
        tested: TESTED_ENGINE_VERSIONS.contains(&p.version.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
      {"tag_name": "nightly", "prerelease": true, "draft": false, "published_at": "2026-10-12T07:14:54Z",
       "assets": [
         {"name": "nemo-speech-nightly-windows-x86_64-cuda.zip", "size": 102196444,
          "browser_download_url": "https://github.com/x/releases/download/nightly/nemo-speech-nightly-windows-x86_64-cuda.zip"},
         {"name": "nemo-speech-nightly-windows-x86_64-cuda.zip.sha256", "size": 110,
          "browser_download_url": "https://github.com/x/releases/download/nightly/nemo-speech-nightly-windows-x86_64-cuda.zip.sha256"},
         {"name": "nemo-speech-nightly-linux-x86_64-cuda.tar.gz", "size": 1, "browser_download_url": "u"}
       ]},
      {"tag_name": "v0.2.0", "prerelease": false, "published_at": "2026-10-02T15:42:06Z",
       "assets": [
         {"name": "nemo-speech-0.2.0-windows-x86_64-vulkan.zip", "size": 19302464, "browser_download_url": "https://v"},
         {"name": "nemo-speech-0.2.0-windows-x86_64-cpu.zip", "size": 5528418, "browser_download_url": "https://c"}
       ]},
      {"tag_name": "v9", "draft": true, "assets": []},
      {"tag_name": "linux-only", "assets": [{"name": "nemo-speech-1.0-linux.tar.gz", "size": 1, "browser_download_url": "u"}]}
    ]"#;

    #[test]
    fn parses_windows_packages_of_each_release() {
        let r = parse_releases(SAMPLE).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].version, "nightly");
        assert!(r[0].prerelease && !r[0].tested);
        assert_eq!(r[0].packages.len(), 1);
        let p = &r[0].packages[0];
        assert_eq!(p.name, "nemo-speech-nightly-windows-x86_64-cuda");
        assert_eq!(p.backend, "cuda");
        assert!(p.sha_url.as_deref().unwrap().ends_with(".sha256"));
        assert!(r[1].tested);
        assert_eq!(
            r[1].packages
                .iter()
                .map(|p| p.backend.as_str())
                .collect::<Vec<_>>(),
            ["vulkan", "cpu"]
        );
        let e = package_entry(&r[1].packages[0], "ab".repeat(32));
        assert_eq!((e.version.as_str(), e.tested), ("0.2.0", true));
        assert!(parse_asset_name("nemo-speech-0.2.0-windows-x86_64-cuda.zip.sha256").is_none());
    }
}
