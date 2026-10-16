//! Engine contract data types (`docs/03-engine-protocol.md` v0.2).
//!
//! The traits that use these types ([`crate::engine::Engine`],
//! [`crate::engine::LiveEngine`], [`crate::engine::LiveSession`]) live in
//! `engine`, which re-exports everything here. Adapters translate runtime
//! output into [`EngineEvent`]s; nothing runtime-specific appears in these types.
//!
//! All engine timestamps are channel-sample time in microseconds (position in
//! that channel's 16 kHz stream). The app maps them to meeting time through
//! clock anchors when interleaving channels.
//!
//! The binary stdin framing for an FFI sidecar is in [`ffi_wire`], behind the
//! `ffi-wire` cargo feature; it is not on the main path.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::types::{Channel, JobId};

#[cfg(feature = "ffi-wire")]
pub mod ffi_wire;

/// Diarizer preset. The streaming preset is used everywhere (ADR: offline
/// scored worse on the spike); `Offline` exists for re-testing new runtimes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiarPreset {
    #[default]
    Streaming,
    Offline,
}

/// A whole-file transcription job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileJob {
    pub job_id: JobId,
    /// 16 kHz mono PCM16 WAV.
    pub wav_path: PathBuf,
    pub diarize: bool,
    /// BCP-47 tag or `auto`.
    pub language: String,
    #[serde(default)]
    pub diar_preset: DiarPreset,
}

/// Options for one live session (one channel).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveOpts {
    pub diarize: bool,
    pub language: String,
}

/// What an engine is running: reported by [`EngineEvent::Ready`] and `Engine::info`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineInfo {
    /// Runtime name and version, e.g. `nemo-speech 0.2.0`.
    pub runtime: String,
    /// Model role (`asr`, `diarizer`, ...) to model name or file.
    #[serde(default)]
    pub models: BTreeMap<String, String>,
    /// `cuda:0`, `cpu`, ...
    pub device: String,
    /// Decoder lookahead a file job passes (`--asr.streaming.rnnt_right_context`,
    /// 80 ms frames; `-1`, the model file's value, only through an extra
    /// flag). `None`: no flag, the runtime default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_context: Option<i64>,
    /// Diarizer preset a diarized file job passes (`--asr.diar.preset`, e.g.
    /// `v3-offline`). `None`: no flag, the runtime's streaming default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diar_preset: Option<String>,
}

/// What runs and Copy diagnostics say for a job that passes no lookahead or
/// preset flag.
pub const LOOKAHEAD_RUNTIME_DEFAULT: &str = "runtime default";

/// A lookahead as a run's `engine_info` stores it (`right_context`): the
/// frames passed, or [`LOOKAHEAD_RUNTIME_DEFAULT`].
pub fn lookahead_json(right_context: Option<i64>) -> serde_json::Value {
    right_context.map_or_else(|| LOOKAHEAD_RUNTIME_DEFAULT.into(), Into::into)
}

/// The same as text: `13`, or `runtime default`.
pub fn lookahead_text(right_context: Option<i64>) -> String {
    right_context.map_or_else(|| LOOKAHEAD_RUNTIME_DEFAULT.to_owned(), |n| n.to_string())
}

/// A diarizer preset as runs (`diar_preset`) and Copy diagnostics show it:
/// its name, or [`LOOKAHEAD_RUNTIME_DEFAULT`].
pub fn diar_preset_text(preset: Option<&str>) -> String {
    preset.unwrap_or(LOOKAHEAD_RUNTIME_DEFAULT).to_owned()
}

/// An event from an engine job or live session.
///
/// Serialized with tag `ev` (snake_case variant names) for logs, fixtures and
/// the optional FFI sidecar's stdout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum EngineEvent {
    /// Models loaded; the engine accepts work.
    Ready { info: EngineInfo },
    /// Job progress in media time.
    Progress {
        job_id: JobId,
        done_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_ms: Option<u64>,
    },
    /// Whole-utterance replacement: a later `rev` of the same `utt` replaces
    /// the earlier one entirely.
    Utterance {
        job_id: JobId,
        channel: Channel,
        utt: u32,
        rev: u32,
        words: Vec<EngineWord>,
    },
    /// Text before `t_us` will not change (no new rev touches utterances ending before it).
    TextFinalUntil {
        job_id: JobId,
        channel: Channel,
        t_us: u64,
    },
    /// Speaker labels before `t_us` will not change. Live adapters may never
    /// send this; the post-pass sets it to the end of the audio.
    SpeakersFinalUntil {
        job_id: JobId,
        channel: Channel,
        t_us: u64,
    },
    /// Reserved: relabel a window. Not produced by any current adapter.
    Relabel {
        job_id: JobId,
        channel: Channel,
        from_us: u64,
        to_us: u64,
        map: BTreeMap<String, String>,
    },
    /// Job finished; always the last event of a successful job.
    Done {
        job_id: JobId,
        #[serde(default)]
        stats: JobStats,
    },
    /// Job failed; always the last event of a failed job. `message` carries
    /// the tail (about 2 kB) of runtime stderr.
    Error {
        job_id: JobId,
        code: ErrorCode,
        message: String,
    },
}

impl EngineEvent {
    /// The job this event belongs to (`None` for [`EngineEvent::Ready`]).
    pub fn job_id(&self) -> Option<&JobId> {
        match self {
            EngineEvent::Ready { .. } => None,
            EngineEvent::Progress { job_id, .. }
            | EngineEvent::Utterance { job_id, .. }
            | EngineEvent::TextFinalUntil { job_id, .. }
            | EngineEvent::SpeakersFinalUntil { job_id, .. }
            | EngineEvent::Relabel { job_id, .. }
            | EngineEvent::Done { job_id, .. }
            | EngineEvent::Error { job_id, .. } => Some(job_id),
        }
    }

    /// `Done` or `Error`: nothing follows for this job.
    pub fn is_terminal(&self) -> bool {
        matches!(self, EngineEvent::Done { .. } | EngineEvent::Error { .. })
    }

    /// Parses one JSON line (surrounding whitespace, including `\r\n`, is ignored).
    pub fn from_json_line(line: &str) -> Result<Self> {
        Ok(serde_json::from_str(line.trim())?)
    }
}

/// A recognized word. Times are channel-sample time in microseconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineWord {
    pub t0_us: u64,
    pub t1_us: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conf: Option<f32>,
    /// Adapter-normalized label (`spk_0`..); `None` when the runtime gave no
    /// speaker. The app assigns `me` for an undiarized mic channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spk: Option<String>,
}

/// Why a job failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Oom,
    Crash,
    BadInput,
    ModelMissing,
    Timeout,
    /// The job was cancelled on request (`Engine::cancel`).
    Cancelled,
    Other,
}

/// Summary statistics in [`EngineEvent::Done`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JobStats {
    /// Real-time factor (processing time / media time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtf: Option<f64>,
    /// Distinct speakers found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speakers: Option<u32>,
    /// Media duration processed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_ms: Option<u64>,
    /// Wall-clock processing time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// One line per variant, in the JSON shape of the docs/03 appendix.
    const EVENT_LINES: &[&str] = &[
        r#"{"ev":"ready","info":{"runtime":"nemo-speech 0.2.0","models":{"asr":"nemotron-speech-streaming-en-0.6b.q8_0.gguf","diarizer":"Nemotron-3-Diarization.q8_0.gguf"},"device":"cuda:0"}}"#,
        r#"{"ev":"progress","job_id":"01J0","done_ms":120000,"total_ms":3600000}"#,
        r#"{"ev":"utterance","job_id":"01J0","channel":"loopback","utt":3,"rev":1,"words":[{"t0_us":12340000,"t1_us":12610000,"text":"hello","conf":0.91,"spk":"spk_1"},{"t0_us":12700000,"t1_us":12900000,"text":"there"}]}"#,
        r#"{"ev":"text_final_until","job_id":"01J0","channel":"mic","t_us":22000000}"#,
        r#"{"ev":"speakers_final_until","job_id":"01J0","channel":"source","t_us":1260000000}"#,
        r#"{"ev":"relabel","job_id":"01J0","channel":"loopback","from_us":10000000,"to_us":20000000,"map":{"spk_1":"spk_0"}}"#,
        r#"{"ev":"done","job_id":"01J0","stats":{"rtf":0.037,"speakers":3,"audio_ms":1273000,"wall_ms":47100}}"#,
        r#"{"ev":"error","job_id":"01J0","code":"model_missing","message":"diarizer model not found"}"#,
    ];

    #[test]
    fn lookahead_reads_as_frames_or_the_runtime_default() {
        assert_eq!(lookahead_json(Some(13)), serde_json::json!(13));
        assert_eq!(lookahead_json(None), serde_json::json!("runtime default"));
        assert_eq!(lookahead_text(Some(-1)), "-1");
        assert_eq!(lookahead_text(None), "runtime default");
        // Absent from `Ready` when no flag is passed (the docs/03 line above).
        let info = EngineInfo {
            right_context: Some(13),
            ..Default::default()
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains(r#""right_context":13"#), "{json}");
        assert_eq!(serde_json::from_str::<EngineInfo>(&json).unwrap(), info);
        let none = serde_json::to_string(&EngineInfo::default()).unwrap();
        assert!(!none.contains("right_context"), "{none}");
    }

    #[test]
    fn diar_preset_reads_as_a_name_or_the_runtime_default() {
        assert_eq!(diar_preset_text(Some("v3-offline")), "v3-offline");
        assert_eq!(diar_preset_text(None), "runtime default");
        let info = EngineInfo {
            diar_preset: Some("v3-offline".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains(r#""diar_preset":"v3-offline""#), "{json}");
        assert_eq!(serde_json::from_str::<EngineInfo>(&json).unwrap(), info);
        let none = serde_json::to_string(&EngineInfo::default()).unwrap();
        assert!(!none.contains("diar_preset"), "{none}");
    }

    #[test]
    fn events_round_trip() {
        for line in EVENT_LINES {
            let ev = EngineEvent::from_json_line(line).unwrap_or_else(|e| panic!("{line}: {e}"));
            let again = serde_json::to_string(&ev).unwrap();
            let a: Value = serde_json::from_str(line).unwrap();
            let b: Value = serde_json::from_str(&again).unwrap();
            assert_eq!(a, b, "round trip changed {line}");
        }
    }

    #[test]
    fn utterance_fields_are_typed() {
        let ev = EngineEvent::from_json_line(EVENT_LINES[2]).unwrap();
        let EngineEvent::Utterance {
            channel,
            utt,
            rev,
            words,
            ..
        } = &ev
        else {
            panic!("expected utterance");
        };
        assert_eq!(*channel, Channel::Loopback);
        assert_eq!((*utt, *rev), (3, 1));
        assert_eq!(words[0].spk.as_deref(), Some("spk_1"));
        assert_eq!(words[1].spk, None);
        assert_eq!(ev.job_id(), Some(&JobId("01J0".into())));
        assert!(!ev.is_terminal());
    }

    #[test]
    fn terminal_events() {
        let done = EngineEvent::from_json_line(EVENT_LINES[6]).unwrap();
        let err = EngineEvent::from_json_line(&format!("{}\r\n", EVENT_LINES[7])).unwrap();
        assert!(done.is_terminal() && err.is_terminal());
        assert!(matches!(
            err,
            EngineEvent::Error {
                code: ErrorCode::ModelMissing,
                ..
            }
        ));
        let ready = EngineEvent::from_json_line(EVENT_LINES[0]).unwrap();
        assert_eq!(ready.job_id(), None);
    }

    #[test]
    fn rejects_unknown_or_malformed() {
        assert!(EngineEvent::from_json_line(r#"{"ev":"party"}"#).is_err());
        assert!(EngineEvent::from_json_line(
            r#"{"ev":"utterance","job_id":"x","channel":"playback","utt":0,"rev":0,"words":[]}"#
        )
        .is_err());
        assert!(EngineEvent::from_json_line("not json").is_err());
    }

    #[test]
    fn file_job_defaults_to_streaming_preset() {
        let job: FileJob = serde_json::from_str(
            r#"{"job_id":"j","wav_path":"C:/a.wav","diarize":true,"language":"en"}"#,
        )
        .unwrap();
        assert_eq!(job.diar_preset, DiarPreset::Streaming);
    }
}
