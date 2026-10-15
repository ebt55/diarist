//! Domain types. One struct per table in `docs/02-data-model.md`.
//!
//! Conventions, matching the schema:
//! - ids are ULID strings (see [`new_id`]), except [`Word::id`], which is the
//!   SQLite integer rowid;
//! - wall-clock timestamps are unix epoch milliseconds (`i64`);
//! - media times (`*_ms`) are milliseconds from the meeting's t=0;
//! - enums serialize as the lowercase strings stored in the `CHECK` columns.
//!
//! Serde field names are the column names (snake_case), so the same JSON shape
//! is used by the DB layer, the Tauri commands and the frontend.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// Returns a fresh ULID as a 26-character Crockford base32 string.
pub fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// Declares a fieldless enum stored as a lowercase string, with `as_str`,
/// `Display`, `FromStr` and serde all agreeing on the spelling.
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        $name:ident, $kind:literal {
            $( $(#[$vmeta:meta])* $variant:ident => $text:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $( $(#[$vmeta])* #[serde(rename = $text)] $variant ),+
        }

        impl $name {
            /// All variants, in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// The string stored in the database and sent over JSON.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = CoreError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($text => Ok($name::$variant),)+
                    _ => Err(CoreError::InvalidEnum {
                        kind: $kind,
                        value: s.to_owned(),
                    }),
                }
            }
        }
    };
}

string_enum! {
    /// Where a meeting's audio came from (`meetings.source`).
    Source, "source" {
        /// Recorded live from mic and loopback.
        Live => "live",
        /// Imported from an existing audio or video file.
        File => "file",
    }
}

string_enum! {
    /// Audio channel (`audio_tracks.channel`, `speakers.channel`).
    Channel, "channel" {
        /// The user's microphone ("me"); ASR only, never diarized.
        Mic => "mic",
        /// System render loopback ("them"); ASR plus diarization.
        Loopback => "loopback",
        /// The single track of an imported file.
        Source => "source",
    }
}

string_enum! {
    /// Lifecycle of a meeting (`meetings.status`).
    MeetingStatus, "meeting status" {
        /// Live capture in progress.
        Recording => "recording",
        /// Capture finished (or file imported); transcription or post-pass running.
        Processing => "processing",
        /// Transcript is final.
        Done => "done",
        /// Processing failed; see logs and `engine_info`.
        Failed => "failed",
    }
}

/// A recorded or imported meeting (`meetings` table).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meeting {
    pub id: String,
    pub title: String,
    pub source: Source,
    /// Original file path when `source` is [`Source::File`].
    pub source_path: Option<String>,
    /// Unix ms, wall clock.
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub status: MeetingStatus,
    /// Model names, versions and runtime version used to produce the transcript.
    pub engine_info: Option<serde_json::Value>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One recorded channel of a meeting (`audio_tracks` table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioTrack {
    pub id: String,
    pub meeting_id: String,
    pub channel: Channel,
    /// Relative to the audio root (`%LOCALAPPDATA%\meet-assist\audio`).
    pub path: String,
    pub sample_rate: u32,
    pub duration_ms: Option<i64>,
    /// Alignment offset of this track versus meeting t=0.
    pub offset_ms: i64,
}

/// A per-meeting speaker label (`speakers` table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Speaker {
    pub id: String,
    pub meeting_id: String,
    /// `spk_0`..`spk_7` from the diarizer, or `me` for the mic channel.
    pub engine_label: String,
    pub channel: Channel,
    /// User-set name; `None` means the UI shows "Speaker N".
    pub display_name: Option<String>,
    /// Hex colour, assigned deterministically, user-overridable.
    pub color: Option<String>,
    /// Cross-meeting identity (v3).
    pub global_id: Option<String>,
    /// Set when the user merged this label into another speaker.
    pub merged_into: Option<String>,
}

/// Contiguous same-speaker run of words (`segments` table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub id: String,
    pub meeting_id: String,
    pub speaker_id: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    /// Display order within the meeting.
    pub seq: i64,
}

/// The atomic transcript unit (`words` table).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    /// SQLite integer rowid (not a ULID).
    pub id: i64,
    pub meeting_id: String,
    pub segment_id: String,
    pub speaker_id: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub confidence: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn new_id_is_a_ulid() {
        let id = new_id();
        assert_eq!(id.len(), 26);
        assert!(ulid::Ulid::from_string(&id).is_ok());
        assert_ne!(id, new_id());
    }

    #[test]
    fn enums_match_schema_check_constraints() {
        let strs = |all: &[&str]| all.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            Source::ALL
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>(),
            strs(&["live", "file"])
        );
        assert_eq!(
            Channel::ALL
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>(),
            strs(&["mic", "loopback", "source"])
        );
        assert_eq!(
            MeetingStatus::ALL
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>(),
            strs(&["recording", "processing", "done", "failed"])
        );
    }

    #[test]
    fn enums_round_trip_through_str_and_serde() {
        for &c in Channel::ALL {
            assert_eq!(c.as_str().parse::<Channel>().unwrap(), c);
            let j = serde_json::to_string(&c).unwrap();
            assert_eq!(j, format!("\"{}\"", c.as_str()));
            assert_eq!(serde_json::from_str::<Channel>(&j).unwrap(), c);
        }
        for &s in MeetingStatus::ALL {
            assert_eq!(s.as_str().parse::<MeetingStatus>().unwrap(), s);
        }
        for &s in Source::ALL {
            assert_eq!(s.as_str().parse::<Source>().unwrap(), s);
        }
    }

    #[test]
    fn enum_parse_rejects_unknown() {
        let err = "speaker".parse::<Channel>().unwrap_err();
        assert!(matches!(
            err,
            CoreError::InvalidEnum {
                kind: "channel",
                ..
            }
        ));
        assert!("Done".parse::<MeetingStatus>().is_err());
    }

    #[test]
    fn meeting_serializes_with_column_names() {
        let m = Meeting {
            id: "01JABCDEFGHJKMNPQRSTVWXYZ0".into(),
            title: "Weekly sync".into(),
            source: Source::File,
            source_path: Some("C:/rec/sync.m4a".into()),
            started_at: 1_760_000_000_000,
            ended_at: Some(1_760_000_600_000),
            duration_ms: Some(600_000),
            status: MeetingStatus::Done,
            engine_info: Some(json!({"asr": "parakeet-tdt-0.6b-v3"})),
            created_at: 1_760_000_000_000,
            updated_at: 1_760_000_700_000,
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["source"], "file");
        assert_eq!(v["status"], "done");
        assert_eq!(v["duration_ms"], 600_000);
        assert_eq!(v["engine_info"]["asr"], "parakeet-tdt-0.6b-v3");
        let back: Meeting = serde_json::from_value(v).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn word_and_speaker_round_trip() {
        let sp = Speaker {
            id: new_id(),
            meeting_id: new_id(),
            engine_label: "spk_1".into(),
            channel: Channel::Loopback,
            display_name: None,
            color: Some("#4f7cff".into()),
            global_id: None,
            merged_into: None,
        };
        let w = Word {
            id: 42,
            meeting_id: sp.meeting_id.clone(),
            segment_id: new_id(),
            speaker_id: sp.id.clone(),
            start_ms: 12_340,
            end_ms: 12_610,
            text: "hello".into(),
            confidence: Some(0.91),
        };
        let sp2: Speaker = serde_json::from_str(&serde_json::to_string(&sp).unwrap()).unwrap();
        let w2: Word = serde_json::from_str(&serde_json::to_string(&w).unwrap()).unwrap();
        assert_eq!(sp2, sp);
        assert_eq!(w2, w);
    }
}
