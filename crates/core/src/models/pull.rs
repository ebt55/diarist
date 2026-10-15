//! `nemo-speech --json pull <name>` into our models folder, then our own
//! size and SHA-256 check against `manifests/models.json` (research/08
//! section 3).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use super::download::verify_file;
use super::manifest::ModelEntry;
use crate::engine::process;
use crate::error::{CoreError, Result};

/// One file `pull` reported.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PullArtifact {
    #[serde(default)]
    pub cached: bool,
    pub path: PathBuf,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub role: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct PullOutput {
    artifacts: Vec<PullArtifact>,
}

/// Parses the `--json pull` result object.
pub fn parse_pull_output(stdout: &str) -> Result<Vec<PullArtifact>> {
    let start = stdout.find('{').unwrap_or(0);
    let end = stdout.rfind('}').map(|i| i + 1).unwrap_or(stdout.len());
    let out: PullOutput = serde_json::from_str(&stdout[start..end])
        .map_err(|e| CoreError::Models(format!("pull output: {e}")))?;
    Ok(out.artifacts)
}

/// A verified model file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pulled {
    pub path: PathBuf,
    /// The runtime already had it.
    pub cached: bool,
}

/// Pulls `entry` with the runtime in `engine_dir` into `models_dir`
/// (`NEMO_SPEECH_MODEL_DIR`) and verifies size and SHA-256 against the manifest.
pub fn pull(engine_dir: &Path, models_dir: &Path, entry: &ModelEntry) -> Result<Pulled> {
    pull_with_progress(engine_dir, models_dir, entry, &mut |_| {})
}

/// Where `nemo-speech pull` puts `entry`: `<models>\<repo>\<revision>\`
/// (`nvidia\Nemotron-3-Diarization\f667...\`).
pub fn pull_dir(models_dir: &Path, entry: &ModelEntry) -> PathBuf {
    entry.dest_dir(models_dir)
}

/// Bytes of the files under `dir` (partial downloads included).
fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => dir_bytes(&e.path()),
                    // Opened by path: a directory entry keeps the old size of a file
                    // that is still being written (curl's `.partial`).
                    Ok(t) if t.is_file() => {
                        std::fs::metadata(e.path()).map(|m| m.len()).unwrap_or(0)
                    }
                    _ => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

/// No new bytes for this long and the pull counts as stalled.
const PULL_STALL: Duration = Duration::from_secs(120);

/// [`pull`], reporting the bytes in the model's folder about four times a
/// second while the runtime downloads (it prints no progress itself; its
/// downloader, System32 `curl.exe`, resumes a partial file on the next
/// pull). A pull that brings no bytes for two minutes is stopped as stalled.
pub fn pull_with_progress(
    engine_dir: &Path,
    models_dir: &Path,
    entry: &ModelEntry,
    progress: &mut dyn FnMut(u64),
) -> Result<Pulled> {
    std::fs::create_dir_all(models_dir)?;
    let watch = pull_dir(models_dir, entry);
    let mut cmd = process::command(engine_dir, Some(models_dir));
    cmd.args(["--json", "pull", &entry.pull_name]);
    let mut last = dir_bytes(&watch);
    let mut last_change = std::time::Instant::now();
    progress(last.min(entry.size_bytes));
    let out = process::run_with_timeout_ticking(cmd, Duration::from_secs(3_600), &mut || {
        let now = dir_bytes(&watch);
        if now != last {
            last = now;
            last_change = std::time::Instant::now();
        } else if last_change.elapsed() >= PULL_STALL {
            return Some(format!(
                "nemo-speech pull {}: no data for {} s (stalled at {now} bytes); retrying resumes",
                entry.pull_name,
                PULL_STALL.as_secs()
            ));
        }
        progress(now.min(entry.size_bytes));
        None
    })
    .map_err(|e| match e {
        CoreError::Engine(m) => CoreError::Models(m),
        other => other,
    })?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Err(CoreError::Models(format!(
            "nemo-speech pull {} failed ({}): {}",
            entry.pull_name,
            out.status,
            process::tail(&stderr, 2_000)
        )));
    }
    let artifacts = parse_pull_output(&String::from_utf8_lossy(&out.stdout))?;
    let artifact = artifacts
        .iter()
        .find(|a| {
            a.path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&entry.file))
        })
        .ok_or_else(|| {
            CoreError::Models(format!(
                "pull {} did not report {}",
                entry.pull_name, entry.file
            ))
        })?;
    verify_file(&artifact.path, Some(entry.size_bytes), &entry.sha256)?;
    Ok(Pulled {
        path: artifact.path.clone(),
        cached: artifact.cached,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Manifest;

    #[test]
    fn parses_the_observed_output() {
        let json = r#"{"artifacts":[{"cached":true,"path":"C:\\m\\nvidia\\Nemotron-3-Diarization\\f667\\Nemotron-3-Diarization.q8_0.gguf","repo":"nvidia/Nemotron-3-Diarization","role":"diarization"}],"repo":"nemotron-3-diarization"}"#;
        let a = parse_pull_output(json).unwrap();
        assert_eq!(a.len(), 1);
        assert!(a[0].cached);
        assert_eq!(a[0].role, "diarization");
        assert!(parse_pull_output("garbage").is_err());
    }

    #[test]
    fn pull_dir_matches_the_runtime_layout() {
        let manifest = Manifest::builtin();
        let entry = manifest.model("nemotron-3-diarization").unwrap();
        let dir = pull_dir(Path::new("C:/m"), entry);
        assert_eq!(
            dir,
            Path::new("C:/m")
                .join("nvidia")
                .join("Nemotron-3-Diarization")
                .join("f667ed73aee57d40cc39428eb768b4fd87a0a29e")
        );
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("a/b")).unwrap();
        std::fs::write(tmp.path().join("a/b/x.part"), [0u8; 1234]).unwrap();
        std::fs::write(tmp.path().join("a/y"), [0u8; 10]).unwrap();
        assert_eq!(dir_bytes(tmp.path()), 1244);
        assert_eq!(dir_bytes(&tmp.path().join("missing")), 0);
    }

    /// Re-pulls the (already present) diarizer with the real runtime when
    /// `DIARIST_ENGINE_DIR` and `DIARIST_MODELS_DIR` are set: the
    /// runtime answers from its cache and we verify the hash.
    #[test]
    fn real_pull_of_a_cached_model_when_available() {
        let (Some(engine), Some(models)) = (
            std::env::var_os("DIARIST_ENGINE_DIR"),
            std::env::var_os("DIARIST_MODELS_DIR"),
        ) else {
            return;
        };
        let manifest = Manifest::builtin();
        let entry = manifest.model("nemotron-3-diarization").unwrap();
        let p = pull(Path::new(&engine), Path::new(&models), entry).unwrap();
        assert!(p.cached, "expected the runtime's cache to answer");
        assert!(p.path.ends_with(&entry.file));
    }
}
