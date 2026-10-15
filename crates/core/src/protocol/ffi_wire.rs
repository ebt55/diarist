//! Wire protocol between the app and an engine process (`docs/03-engine-protocol.md`).
//!
//! - **stdin (app to engine):** binary frames, `u8 type` + `u32 LE length` + payload.
//!   [`FrameType::Control`] payloads are one UTF-8 JSON [`EngineCommand`];
//!   [`FrameType::Audio`] payloads start with an [`AudioPayloadHeader`]
//!   (`u8 channel_id` + `u64 LE pts_us`) followed by int16 LE mono PCM at 16 kHz.
//! - **stdout (engine to app):** newline-delimited JSON, one [`EngineEvent`] per line.
//!
//! Only types and (de)serialization live here; the process supervisor and the
//! adapters arrive with the `engine` module in Phase 1.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::types::Channel;

// ---------------------------------------------------------------------------
// Control messages (app to engine)
// ---------------------------------------------------------------------------

/// A control message, sent as the JSON payload of a [`FrameType::Control`] frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum EngineCommand {
    /// Load models. Must be the first command; the engine answers [`EngineEvent::Ready`].
    Init {
        /// ASR model name, e.g. `parakeet-tdt-0.6b-v3`.
        asr: String,
        /// Diarizer model name; omitted for sessions that never diarize (mic).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diarizer: Option<String>,
        /// `cuda`, `cuda:0`, `cpu`, ...
        device: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_speakers: Option<u32>,
    },
    /// Transcribe (and optionally diarize) a whole file.
    TranscribeFile {
        job_id: String,
        path: String,
        diarize: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        language: Option<String>,
    },
    /// Open a live session; audio follows in [`FrameType::Audio`] frames.
    StartStream {
        job_id: String,
        channels: Vec<StreamChannel>,
    },
    /// Flush and finalize everything for a live session.
    StopStream { job_id: String },
    /// Abort a job without finalizing.
    Cancel { job_id: String },
    /// Exit the engine process.
    Shutdown,
}

/// One input channel of a live session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamChannel {
    /// `channel_id` used in audio frames and in events.
    pub id: u8,
    pub name: Channel,
    pub diarize: bool,
}

// ---------------------------------------------------------------------------
// Events (engine to app)
// ---------------------------------------------------------------------------

/// One line of engine stdout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum EngineEvent {
    /// Models loaded; the engine accepts jobs.
    Ready {
        /// Engine name and version, e.g. `meet-engine 0.1.0`.
        engine: String,
        /// Runtime name and version behind the engine.
        runtime: String,
        /// Loaded models by role; shape is runtime-defined.
        #[serde(default)]
        models: serde_json::Map<String, serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vram_mb: Option<u64>,
    },
    /// Job progress in media time.
    Progress {
        job_id: String,
        done_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_ms: Option<u64>,
    },
    /// Recognized words. With `final: false` they may be superseded by later
    /// words covering the same time range; the app keeps the latest.
    Words {
        job_id: String,
        channel: u8,
        #[serde(rename = "final")]
        is_final: bool,
        words: Vec<EngineWord>,
    },
    /// The diarizer re-mapped speaker labels within `[from_t, to_t)` seconds.
    Relabel {
        job_id: String,
        channel: u8,
        from_t: f64,
        to_t: f64,
        map: BTreeMap<String, String>,
    },
    /// Nothing before `up_to_t` seconds will change; safe to commit to the DB.
    Finalized {
        job_id: String,
        channel: u8,
        up_to_t: f64,
    },
    /// Job finished.
    Done {
        job_id: String,
        #[serde(default)]
        stats: JobStats,
    },
    /// Job- or engine-level failure. `job_id` is absent for engine-level errors.
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        job_id: Option<String>,
        /// Machine-readable code, e.g. `oom`.
        code: String,
        message: String,
    },
}

/// A word as emitted by the engine. Times are seconds from the job's t=0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineWord {
    pub t0: f64,
    pub t1: f64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conf: Option<f32>,
    /// Provisional speaker label (`spk_N`); absent when not diarized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spk: Option<String>,
}

/// Summary statistics in [`EngineEvent::Done`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JobStats {
    /// Real-time factor (processing time / media time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtf: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speakers: Option<u32>,
}

impl EngineEvent {
    /// Parses one stdout line (surrounding whitespace, including `\r\n`, is ignored).
    pub fn from_line(line: &str) -> Result<Self> {
        Ok(serde_json::from_str(line.trim())?)
    }
}

// ---------------------------------------------------------------------------
// Binary framing (stdin)
// ---------------------------------------------------------------------------

/// Kind of a stdin frame (first header byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FrameType {
    /// UTF-8 JSON [`EngineCommand`].
    Control = 0x01,
    /// [`AudioPayloadHeader`] + int16 LE mono PCM at 16 kHz.
    Audio = 0x02,
}

impl TryFrom<u8> for FrameType {
    type Error = CoreError;

    fn try_from(b: u8) -> Result<Self> {
        match b {
            0x01 => Ok(FrameType::Control),
            0x02 => Ok(FrameType::Audio),
            other => Err(CoreError::UnknownFrameType(other)),
        }
    }
}

/// Header in front of every stdin frame: `u8 type` + `u32 LE payload length`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub frame_type: FrameType,
    /// Payload length in bytes, not counting this header.
    pub len: u32,
}

impl FrameHeader {
    /// Encoded size in bytes.
    pub const LEN: usize = 5;

    /// Builds a header for a payload, failing if it does not fit in `u32`.
    pub fn for_payload(frame_type: FrameType, payload: &[u8]) -> Result<Self> {
        let len =
            u32::try_from(payload.len()).map_err(|_| CoreError::FrameTooLarge(payload.len()))?;
        Ok(Self { frame_type, len })
    }

    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0] = self.frame_type as u8;
        out[1..5].copy_from_slice(&self.len.to_le_bytes());
        out
    }

    /// Decodes the first [`FrameHeader::LEN`] bytes of `buf`; extra bytes are ignored.
    pub fn decode(buf: &[u8]) -> Result<Self> {
        let head = buf.get(..Self::LEN).ok_or(CoreError::ShortBuffer {
            what: "frame header",
            need: Self::LEN,
            got: buf.len(),
        })?;
        let frame_type = FrameType::try_from(head[0])?;
        let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]);
        Ok(Self { frame_type, len })
    }
}

/// Prefix of an [`FrameType::Audio`] payload: `u8 channel_id` + `u64 LE pts_us`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioPayloadHeader {
    /// Matches [`StreamChannel::id`].
    pub channel_id: u8,
    /// Presentation timestamp of the first sample, microseconds on the session timeline.
    pub pts_us: u64,
}

impl AudioPayloadHeader {
    /// Encoded size in bytes.
    pub const LEN: usize = 9;

    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0] = self.channel_id;
        out[1..9].copy_from_slice(&self.pts_us.to_le_bytes());
        out
    }

    /// Decodes the first [`AudioPayloadHeader::LEN`] bytes of `buf`; the PCM follows.
    pub fn decode(buf: &[u8]) -> Result<Self> {
        let head = buf.get(..Self::LEN).ok_or(CoreError::ShortBuffer {
            what: "audio payload header",
            need: Self::LEN,
            got: buf.len(),
        })?;
        let mut pts = [0u8; 8];
        pts.copy_from_slice(&head[1..9]);
        Ok(Self {
            channel_id: head[0],
            pts_us: u64::from_le_bytes(pts),
        })
    }
}

/// Serializes `cmd` into a complete control frame (header + JSON payload).
pub fn encode_control_frame(cmd: &EngineCommand) -> Result<Vec<u8>> {
    let payload = serde_json::to_vec(cmd)?;
    let header = FrameHeader::for_payload(FrameType::Control, &payload)?;
    let mut frame = Vec::with_capacity(FrameHeader::LEN + payload.len());
    frame.extend_from_slice(&header.encode());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// Control examples from docs/03-engine-protocol.md, verbatim.
    const COMMAND_EXAMPLES: &[&str] = &[
        r#"{"cmd":"init","asr":"parakeet-tdt-0.6b-v3","diarizer":"nemotron-3-diarization","device":"cuda","max_speakers":8}"#,
        r#"{"cmd":"transcribe_file","job_id":"...","path":"C:/...wav","diarize":true,"language":"en"}"#,
        r#"{"cmd":"start_stream","job_id":"...","channels":[{"id":0,"name":"mic","diarize":false},{"id":1,"name":"loopback","diarize":true}]}"#,
        r#"{"cmd":"stop_stream","job_id":"..."}"#,
        r#"{"cmd":"cancel","job_id":"..."}"#,
        r#"{"cmd":"shutdown"}"#,
    ];

    /// Event examples from docs/03-engine-protocol.md. The doc elides
    /// `"models":{...}` and `"runtime":"..."`; concrete values stand in here.
    const EVENT_EXAMPLES: &[&str] = &[
        r#"{"ev":"ready","engine":"meet-engine 0.1.0","runtime":"nemo-speech 0.2.0","models":{"asr":"parakeet-tdt-0.6b-v3","diarizer":"nemotron-3-diarization"},"vram_mb":1234}"#,
        r#"{"ev":"progress","job_id":"...","done_ms":120000,"total_ms":3600000}"#,
        r#"{"ev":"words","job_id":"...","channel":1,"final":false,
            "words":[{"t0":12.34,"t1":12.61,"text":"hello","conf":0.91,"spk":"spk_1"}]}"#,
        r#"{"ev":"relabel","job_id":"...","channel":1,"from_t":10.0,"to_t":20.0,"map":{"spk_1":"spk_0"}}"#,
        r#"{"ev":"finalized","job_id":"...","channel":1,"up_to_t":22.0}"#,
        r#"{"ev":"done","job_id":"...","stats":{"rtf":0.08,"speakers":3}}"#,
        r#"{"ev":"error","job_id":"...","code":"oom","message":"..."}"#,
    ];

    /// Parse into the typed enum, serialize back, and compare as JSON values,
    /// so field order and whitespace do not matter but every field must survive.
    fn round_trip<T>(src: &str) -> T
    where
        T: serde::de::DeserializeOwned + Serialize,
    {
        let typed: T = serde_json::from_str(src).unwrap_or_else(|e| panic!("parse {src}: {e}"));
        let again = serde_json::to_string(&typed).unwrap();
        let a: Value = serde_json::from_str(src).unwrap();
        let b: Value = serde_json::from_str(&again).unwrap();
        assert_eq!(a, b, "round trip changed {src}");
        typed
    }

    #[test]
    fn commands_round_trip() {
        for src in COMMAND_EXAMPLES {
            round_trip::<EngineCommand>(src);
        }
    }

    #[test]
    fn events_round_trip() {
        for src in EVENT_EXAMPLES {
            round_trip::<EngineEvent>(src);
        }
    }

    #[test]
    fn typed_fields_land_where_expected() {
        let cmd: EngineCommand = round_trip(COMMAND_EXAMPLES[2]);
        let EngineCommand::StartStream { channels, .. } = cmd else {
            panic!("expected start_stream");
        };
        assert_eq!(channels[1].name, Channel::Loopback);
        assert!(channels[1].diarize);

        let ev: EngineEvent = round_trip(EVENT_EXAMPLES[2]);
        let EngineEvent::Words {
            channel,
            is_final,
            words,
            ..
        } = ev
        else {
            panic!("expected words");
        };
        assert_eq!(channel, 1);
        assert!(!is_final);
        assert_eq!(words[0].spk.as_deref(), Some("spk_1"));
        assert_eq!(words[0].conf, Some(0.91));
    }

    #[test]
    fn optional_fields_may_be_absent() {
        let init: EngineCommand = serde_json::from_str(
            r#"{"cmd":"init","asr":"nemotron-speech-streaming-en-0.6b","device":"cpu"}"#,
        )
        .unwrap();
        assert_eq!(
            init,
            EngineCommand::Init {
                asr: "nemotron-speech-streaming-en-0.6b".into(),
                diarizer: None,
                device: "cpu".into(),
                max_speakers: None,
            }
        );
        let err = EngineEvent::from_line(
            "{\"ev\":\"error\",\"code\":\"model_missing\",\"message\":\"x\"}\r\n",
        )
        .unwrap();
        assert!(matches!(err, EngineEvent::Error { job_id: None, .. }));
    }

    #[test]
    fn unknown_tags_are_rejected() {
        assert!(serde_json::from_str::<EngineCommand>(r#"{"cmd":"reboot"}"#).is_err());
        assert!(EngineEvent::from_line(r#"{"ev":"party"}"#).is_err());
        assert!(EngineEvent::from_line("not json").is_err());
    }

    #[test]
    fn frame_header_encodes_little_endian() {
        let h = FrameHeader {
            frame_type: FrameType::Audio,
            len: 0x0102_0304,
        };
        assert_eq!(h.encode(), [0x02, 0x04, 0x03, 0x02, 0x01]);
        assert_eq!(FrameHeader::decode(&h.encode()).unwrap(), h);
    }

    #[test]
    fn frame_header_round_trips_extremes() {
        for ft in [FrameType::Control, FrameType::Audio] {
            for len in [0, 1, 3200, u32::MAX] {
                let h = FrameHeader {
                    frame_type: ft,
                    len,
                };
                assert_eq!(FrameHeader::decode(&h.encode()).unwrap(), h);
            }
        }
    }

    #[test]
    fn frame_header_decode_errors() {
        assert!(matches!(
            FrameHeader::decode(&[0x01, 0, 0]),
            Err(CoreError::ShortBuffer {
                need: 5,
                got: 3,
                ..
            })
        ));
        assert!(matches!(
            FrameHeader::decode(&[0x7f, 0, 0, 0, 0]),
            Err(CoreError::UnknownFrameType(0x7f))
        ));
    }

    #[test]
    fn audio_payload_header_round_trips() {
        let h = AudioPayloadHeader {
            channel_id: 1,
            pts_us: 0x0102_0304_0506_0708,
        };
        let bytes = h.encode();
        assert_eq!(bytes, [1, 8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(AudioPayloadHeader::decode(&bytes).unwrap(), h);
        assert!(AudioPayloadHeader::decode(&bytes[..8]).is_err());
    }

    #[test]
    fn control_frame_is_header_plus_json() {
        let frame = encode_control_frame(&EngineCommand::Shutdown).unwrap();
        let header = FrameHeader::decode(&frame).unwrap();
        assert_eq!(header.frame_type, FrameType::Control);
        assert_eq!(header.len as usize, frame.len() - FrameHeader::LEN);
        let body: Value = serde_json::from_slice(&frame[FrameHeader::LEN..]).unwrap();
        assert_eq!(body, serde_json::json!({"cmd": "shutdown"}));
    }
}
