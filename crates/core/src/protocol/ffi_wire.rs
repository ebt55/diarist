//! Wire format for an optional FFI sidecar process (`docs/03-engine-protocol.md`,
//! appendix). Compiled only with the `ffi-wire` feature: the shipped adapters
//! (CLI and WebSocket) do not use it.
//!
//! - **stdin (app to sidecar):** frames of `u8 type` + `u32 LE length` + payload.
//!   [`FrameType::Control`] payloads are one UTF-8 JSON [`EngineCommand`];
//!   [`FrameType::Audio`] payloads start with an [`AudioPayloadHeader`]
//!   (`u8 channel` + `u64 LE pts_us`) followed by int16 LE mono PCM at 16 kHz.
//! - **stdout (sidecar to app):** newline-delimited JSON
//!   [`EngineEvent`](super::EngineEvent)s (serde tag `ev`).
//! - **stderr:** free-form logs.

use serde::{Deserialize, Serialize};

use super::{DiarPreset, FileJob, LiveOpts};
use crate::error::{CoreError, Result};
use crate::types::{Channel, JobId};

/// A control message: the JSON payload of a [`FrameType::Control`] frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum EngineCommand {
    /// Load models; the sidecar answers `ready`.
    Init {
        asr: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diarizer: Option<String>,
        device: String,
        #[serde(default)]
        diar_preset: DiarPreset,
    },
    /// Start a file job (mirrors `Engine::transcribe_file`).
    TranscribeFile(FileJob),
    /// Open a live session for one channel (mirrors `LiveEngine::open`).
    /// Audio for it arrives in frames whose `channel_id` is `channel_id`.
    OpenSession {
        job_id: JobId,
        channel: Channel,
        channel_id: u8,
        #[serde(flatten)]
        opts: LiveOpts,
    },
    /// Flush and finalize a live session (mirrors `LiveSession::close`).
    CloseSession { job_id: JobId },
    /// Abort a job without finalizing.
    Cancel { job_id: JobId },
    /// Exit the sidecar.
    Shutdown,
}

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
    /// Matches `channel_id` of an [`EngineCommand::OpenSession`].
    pub channel_id: u8,
    /// Channel-sample time of the first sample, microseconds.
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

    const COMMANDS: &[&str] = &[
        r#"{"cmd":"init","asr":"nemotron-speech-streaming-en-0.6b","diarizer":"nemotron-3-diarization","device":"cuda:0","diar_preset":"streaming"}"#,
        r#"{"cmd":"transcribe_file","job_id":"j1","wav_path":"C:/a.wav","diarize":true,"language":"en","diar_preset":"streaming"}"#,
        r#"{"cmd":"open_session","job_id":"j2","channel":"loopback","channel_id":1,"diarize":true,"language":"en"}"#,
        r#"{"cmd":"close_session","job_id":"j2"}"#,
        r#"{"cmd":"cancel","job_id":"j1"}"#,
        r#"{"cmd":"shutdown"}"#,
    ];

    #[test]
    fn commands_round_trip() {
        for src in COMMANDS {
            let cmd: EngineCommand =
                serde_json::from_str(src).unwrap_or_else(|e| panic!("{src}: {e}"));
            let a: Value = serde_json::from_str(src).unwrap();
            let b: Value = serde_json::from_str(&serde_json::to_string(&cmd).unwrap()).unwrap();
            assert_eq!(a, b, "round trip changed {src}");
        }
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
