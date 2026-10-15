//! What a model file is for, from its GGUF header (`docs/01`, "Model
//! management"): a small architecture table plus an optional
//! `<file>.meet.json` sidecar that overrides the guess.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::gguf::{GgufHeader, GgufValue};
use crate::error::{CoreError, Result};
use crate::types::{ModelCapabilities, ModelRole};

/// The result of classifying one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    pub role: ModelRole,
    /// Model family from the table (`nemotron-streaming-asr`, `parakeet-tdt`,
    /// `nemotron-3-diarization`, ...), `None` when unrecognised.
    pub family: Option<&'static str>,
    pub architecture: Option<String>,
    pub display_name: Option<String>,
    pub quant: Option<String>,
    pub capabilities: ModelCapabilities,
}

/// Classifies a GGUF from its header and file size. VRAM is an estimate
/// from the size; the scanner replaces it with a measured value when the
/// file matches the manifest.
pub fn classify(header: &GgufHeader, size_bytes: u64) -> Classification {
    let arch = header.architecture().map(str::to_owned);
    let mut caps = ModelCapabilities::default();
    let (role, family) = match arch.as_deref() {
        Some("sortformer") => {
            caps.diarize = true;
            caps.streaming = true;
            let v3 = header.get_str("sortformer.version") == Some("v3");
            let family = if v3 {
                "nemotron-3-diarization"
            } else {
                "sortformer"
            };
            (ModelRole::Diarizer, Some(family))
        }
        Some("asr") => {
            let head = header.get_str("asr.head_type").unwrap_or("");
            let cache = header
                .get("asr.encoder.cache_supported")
                .and_then(GgufValue::as_bool)
                .unwrap_or(false);
            caps.languages = languages(header);
            let family = match head {
                "rnnt" if cache => {
                    caps.streaming = true;
                    "nemotron-streaming-asr"
                }
                "tdt" => "parakeet-tdt",
                "ctc" => "parakeet-ctc",
                _ => "nemo-asr",
            };
            (ModelRole::Asr, Some(family))
        }
        Some("titanet" | "ecapa_tdnn" | "speakernet") => {
            (ModelRole::Embedder, Some("speaker-embedding"))
        }
        Some(
            "llama" | "qwen2" | "qwen3" | "qwen3moe" | "gemma" | "gemma2" | "gemma3" | "mistral"
            | "phi3" | "granite",
        ) => (ModelRole::Llm, Some("llm")),
        _ => (ModelRole::Unknown, None),
    };
    if caps.languages.is_empty() && role == ModelRole::Asr {
        let name = header.name().unwrap_or_default().to_ascii_lowercase();
        if name.contains("-en-") || name.contains("_en_") || name.ends_with("-en") {
            caps.languages = vec!["en".into()];
        }
    }
    caps.vram_mb_estimate = (role != ModelRole::Unknown).then(|| estimate_vram_mb(size_bytes));
    Classification {
        role,
        family,
        architecture: arch,
        display_name: header.name(),
        quant: header.quant(),
        capabilities: caps,
    }
}

/// Locales from Nemotron 3.5's prompt dictionary (`en-US:0`, ...), deduplicated.
fn languages(header: &GgufHeader) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(items) = header
        .get("asr.rnnt.prompt_dictionary")
        .and_then(GgufValue::as_array)
    {
        for v in items {
            if let Some(tag) = v.as_str().and_then(|s| s.split(':').next()) {
                let tag = tag.trim();
                if !tag.is_empty() && !out.iter().any(|t| t == tag) {
                    out.push(tag.to_owned());
                }
            }
        }
    }
    out
}

/// Rough VRAM for running the file: weights plus activations and caches.
/// Roughly matches the measured 1.8 GB for the 742 MB Nemotron 3.5 file.
pub fn estimate_vram_mb(size_bytes: u64) -> u32 {
    let mb = size_bytes / (1 << 20);
    let est = mb * 2 + 400;
    (est.div_ceil(50) * 50).min(u64::from(u32::MAX)) as u32
}

/// `<file>.meet.json`: manual overrides for a model file. Every field is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Sidecar {
    pub role: Option<ModelRole>,
    pub display_name: Option<String>,
    pub streaming: Option<bool>,
    pub diarize: Option<bool>,
    pub languages: Option<Vec<String>>,
    pub vram_mb: Option<u32>,
}

impl Sidecar {
    /// Path of the sidecar for `model`.
    pub fn path_for(model: &Path) -> PathBuf {
        let mut s = model.as_os_str().to_owned();
        s.push(".meet.json");
        PathBuf::from(s)
    }

    /// Reads the sidecar of `model`, if any.
    pub fn read(model: &Path) -> Result<Option<Self>> {
        let path = Self::path_for(model);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map(Some)
                .map_err(|e| CoreError::Models(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(CoreError::Models(format!("{}: {e}", path.display()))),
        }
    }

    /// Applies the overrides.
    pub fn apply(&self, c: &mut Classification) {
        if let Some(role) = self.role {
            c.role = role;
        }
        if let Some(name) = &self.display_name {
            c.display_name = Some(name.clone());
        }
        if let Some(v) = self.streaming {
            c.capabilities.streaming = v;
        }
        if let Some(v) = self.diarize {
            c.capabilities.diarize = v;
        }
        if let Some(v) = &self.languages {
            c.capabilities.languages = v.clone();
        }
        if let Some(v) = self.vram_mb {
            c.capabilities.vram_mb_measured = Some(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::gguf::build;
    use super::super::gguf::parse_header;
    use super::*;

    fn s(v: &str) -> GgufValue {
        GgufValue::Str(v.into())
    }

    fn header(kv: &[(&str, GgufValue)]) -> GgufHeader {
        parse_header(&build::header(3, 1, kv)[..]).unwrap()
    }

    #[test]
    fn architecture_table() {
        let nemotron35 = header(&[
            ("general.architecture", s("asr")),
            (
                "general.name",
                s(".nemotron-3.5-asr-streaming-0.6b.q8_0.gguf"),
            ),
            ("general.file_type", GgufValue::U32(7)),
            ("asr.head_type", s("rnnt")),
            ("asr.encoder.cache_supported", GgufValue::Bool(true)),
            (
                "asr.rnnt.prompt_dictionary",
                GgufValue::Array {
                    elem_type: 8,
                    len: 3,
                    items: vec![s("en-US:0"), s("en:0"), s("de-DE:2")],
                },
            ),
        ]);
        let c = classify(&nemotron35, 741_548_352);
        assert_eq!(c.role, ModelRole::Asr);
        assert_eq!(c.family, Some("nemotron-streaming-asr"));
        assert!(c.capabilities.streaming && !c.capabilities.diarize);
        assert_eq!(c.capabilities.languages, ["en-US", "en", "de-DE"]);
        assert_eq!(c.quant.as_deref(), Some("Q8_0"));
        let est = c.capabilities.vram_mb_estimate.unwrap();
        assert!((1_500..2_200).contains(&est), "{est}");

        let en = header(&[
            ("general.architecture", s("asr")),
            (
                "general.name",
                s(".nemotron-speech-streaming-en-0.6b.q8_0.gguf"),
            ),
            ("asr.head_type", s("rnnt")),
            ("asr.encoder.cache_supported", GgufValue::Bool(true)),
        ]);
        assert_eq!(classify(&en, 1).capabilities.languages, ["en"]);

        let parakeet = header(&[
            ("general.architecture", s("asr")),
            ("asr.head_type", s("tdt")),
            ("asr.encoder.cache_supported", GgufValue::Bool(false)),
        ]);
        let c = classify(&parakeet, 713_975_456);
        assert_eq!((c.role, c.family), (ModelRole::Asr, Some("parakeet-tdt")));
        assert!(!c.capabilities.streaming);

        let diar = header(&[
            ("general.architecture", s("sortformer")),
            ("sortformer.version", s("v3")),
        ]);
        let c = classify(&diar, 107_012_128);
        assert_eq!(
            (c.role, c.family),
            (ModelRole::Diarizer, Some("nemotron-3-diarization"))
        );
        assert!(c.capabilities.diarize);

        let other = header(&[
            ("general.architecture", s("bert")),
            ("general.name", s("x")),
        ]);
        let c = classify(&other, 10);
        assert_eq!((c.role, c.family), (ModelRole::Unknown, None));
        assert_eq!(c.capabilities.vram_mb_estimate, None);
        assert_eq!(c.display_name.as_deref(), Some("x"));
    }

    #[test]
    fn sidecar_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("odd.gguf");
        assert_eq!(Sidecar::read(&model).unwrap(), None);
        std::fs::write(
            Sidecar::path_for(&model),
            r#"{"role":"asr","streaming":true,"languages":["fr"],"vram_mb":900}"#,
        )
        .unwrap();
        let sc = Sidecar::read(&model).unwrap().unwrap();
        let mut c = classify(&header(&[("general.architecture", s("mystery"))]), 1);
        sc.apply(&mut c);
        assert_eq!(c.role, ModelRole::Asr);
        assert!(c.capabilities.streaming);
        assert_eq!(c.capabilities.languages, ["fr"]);
        assert_eq!(c.capabilities.vram_mb_measured, Some(900));
        std::fs::write(Sidecar::path_for(&model), "{not json").unwrap();
        assert!(Sidecar::read(&model).is_err());
    }
}
