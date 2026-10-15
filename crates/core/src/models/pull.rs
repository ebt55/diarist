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
    std::fs::create_dir_all(models_dir)?;
    let mut cmd = process::command(engine_dir, Some(models_dir));
    cmd.args(["--json", "pull", &entry.pull_name]);
    let out = process::run_with_timeout(cmd, Duration::from_secs(3_600))?;
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

    /// Re-pulls the (already present) diarizer with the real runtime when
    /// `MEET_ASSIST_ENGINE_DIR` and `MEET_ASSIST_MODELS_DIR` are set: the
    /// runtime answers from its cache and we verify the hash.
    #[test]
    fn real_pull_of_a_cached_model_when_available() {
        let (Some(engine), Some(models)) = (
            std::env::var_os("MEET_ASSIST_ENGINE_DIR"),
            std::env::var_os("MEET_ASSIST_MODELS_DIR"),
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
