//! Domain types. One struct per table in `docs/02-data-model.md` (v0.3).
//!
//! Conventions, matching the schema:
//! - primary keys are SQLite INTEGER rowids wrapped in newtypes ([`MeetingId`],
//!   [`RunId`], ...); the only ULID is [`Meeting::uid`] (see [`new_id`]);
//! - wall-clock timestamps are unix epoch milliseconds (`i64`);
//! - media times (`*_ms`) are milliseconds from the meeting's t=0;
//! - SQLite `INTEGER` booleans are `bool`;
//! - enums serialize as the lowercase strings stored in the `CHECK` columns.
//!
//! Serde field names are the column names (snake_case), so the same JSON shape
//! is used by the store, the Tauri commands and the frontend. Types that cross
//! into the UI derive [`ts_rs::TS`] and are exported to
//! `apps/desktop/src/lib/generated/` by `cargo test` (see `.cargo/config.toml`).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::CoreError;
use crate::protocol::EngineWord;

/// Returns a fresh ULID as a 26-character Crockford base32 string.
pub fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// Declares an INTEGER-primary-key newtype (`number` in TypeScript).
macro_rules! int_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS,
        )]
        #[serde(transparent)]
        #[ts(export)]
        pub struct $name(pub i64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<i64> for $name {
            fn from(v: i64) -> Self {
                Self(v)
            }
        }

        impl From<$name> for i64 {
            fn from(v: $name) -> i64 {
                v.0
            }
        }
    };
}

int_id!(
    /// `meetings.id`.
    MeetingId
);
int_id!(
    /// `transcript_runs.id`.
    RunId
);
int_id!(
    /// `speakers.id`.
    SpeakerId
);
int_id!(
    /// `segments.id`.
    SegmentId
);
int_id!(
    /// `words.id`.
    WordId
);
int_id!(
    /// `audio_tracks.id`.
    TrackId
);
int_id!(
    /// `model_files.id`.
    ModelFileId
);

/// Identifies one engine or import job (a ULID string; not stored in the DB).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export)]
pub struct JobId(pub String);

impl JobId {
    /// A fresh, unique job id.
    pub fn new() -> Self {
        Self(new_id())
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Declares a fieldless enum stored as a lowercase string, with `as_str`,
/// `Display`, `FromStr`, serde and ts-rs all agreeing on the spelling.
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        $name:ident, $kind:literal {
            $( $(#[$vmeta:meta])* $variant:ident => $text:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS,
        )]
        #[ts(export)]
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
    /// Transcribed audio channel (`speakers.channel`, engine sessions, clock anchors).
    Channel, "channel" {
        /// The user's microphone ("me"); ASR only unless `diarize_mic` is set.
        Mic => "mic",
        /// System render loopback ("them"); ASR plus diarization.
        Loopback => "loopback",
        /// The single track of an imported file.
        Source => "source",
    }
}

string_enum! {
    /// Role of a stored audio file (`audio_tracks.channel`).
    TrackChannel, "track channel" {
        /// Per-channel 16 kHz WAV written while recording.
        Mic => "mic",
        /// Per-channel 16 kHz WAV written while recording.
        Loopback => "loopback",
        /// 16 kHz intermediate decoded from an import.
        Source => "source",
        /// What the UI plays: stereo FLAC (left mic, right loopback) or the original import.
        Playback => "playback",
    }
}

string_enum! {
    /// Container of a stored audio file (`audio_tracks.format`).
    AudioFormat, "audio format" {
        Wav => "wav",
        Flac => "flac",
        /// The user's original imported file, referenced by absolute path.
        Original => "original",
    }
}

string_enum! {
    /// Lifecycle of a meeting (`meetings.status`).
    MeetingStatus, "meeting status" {
        /// Live capture in progress.
        Recording => "recording",
        /// Capture finished (or file imported); transcription or post-pass running.
        Processing => "processing",
        /// The current run is final.
        Done => "done",
        /// Processing failed; see logs and the run's `engine_info`.
        Failed => "failed",
    }
}

string_enum! {
    /// What produced a transcript run (`transcript_runs.kind`).
    RunKind, "run kind" {
        /// Live preview while recording.
        Live => "live",
        /// File import.
        File => "file",
        /// Canonical pass over the recorded channels after a live meeting.
        PostPass => "post_pass",
        /// Re-diarization of an existing meeting.
        Rediarize => "rediarize",
        /// Re-fusion of words and speakers with a different algorithm.
        Refuse => "refuse",
    }
}

string_enum! {
    /// State of a transcript run (`transcript_runs.status`).
    RunStatus, "run status" {
        Running => "running",
        Done => "done",
        Failed => "failed",
    }
}

string_enum! {
    /// Kind of a time-ranged user edit (`edits.kind`).
    EditKind, "edit kind" {
        /// Words in the range belong to another speaker.
        Reassign => "reassign",
    }
}

string_enum! {
    /// What a file under `models\` or `engine\` is for (`model_files.role`).
    ModelRole, "model role" {
        /// Speech to text.
        Asr => "asr",
        /// Speaker diarization.
        Diarizer => "diarizer",
        /// Speaker embeddings (v3).
        Embedder => "embedder",
        Punctuation => "punctuation",
        Llm => "llm",
        /// A runtime folder (`engine/<name>/bin/nemo-speech.exe`).
        Engine => "engine",
        /// Not recognised by the classifier; shown greyed.
        Unknown => "unknown",
    }
}

string_enum! {
    /// How a model file got onto the disk (`model_files.source`).
    ModelSource, "model source" {
        /// Matches an entry of `manifests/models.json` ("recommended").
        Manifest => "manifest",
        /// Fetched with `nemo-speech pull`.
        Pull => "pull",
        /// Dropped in by the user.
        User => "user",
    }
}

/// What a model can do (`model_files.capabilities`, JSON).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ModelCapabilities {
    pub streaming: bool,
    pub diarize: bool,
    /// BCP-47 tags; empty when unknown.
    pub languages: Vec<String>,
    /// Measured on the reference GPU (from the manifest when the sha matches).
    pub vram_mb_measured: Option<u32>,
    /// Estimated from file size when nothing was measured.
    pub vram_mb_estimate: Option<u32>,
    /// Decoder lookahead for file jobs and the post-pass
    /// (`--asr.streaming.rnnt_right_context`, 80 ms frames), from the
    /// manifest entry or the `<file>.meet.json` sidecar. `None`: no flag,
    /// the runtime default (the low-latency setting live keeps).
    #[serde(default)]
    pub file_right_context: Option<u32>,
    /// Diarizer preset for file jobs and the post-pass (`--asr.diar.preset`,
    /// e.g. `v3-offline`), from the manifest entry or the sidecar. `None`: no
    /// flag, the runtime's streaming default (which live keeps).
    #[serde(default)]
    pub file_preset: Option<String>,
}

/// A file the model scanner found (`model_files`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ModelFile {
    pub id: ModelFileId,
    /// Relative to the data root.
    pub path: String,
    pub role: ModelRole,
    /// GGUF `general.architecture`, or the runtime version for engines.
    pub architecture: Option<String>,
    /// GGUF `general.name`, or the file stem.
    pub display_name: String,
    /// e.g. `Q8_0`.
    pub quant: Option<String>,
    pub size_bytes: i64,
    pub sha256: Option<String>,
    pub source: ModelSource,
    pub capabilities: ModelCapabilities,
    /// Set when the file matches a `manifests/models.json` entry.
    pub manifest_name: Option<String>,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    /// The file disappeared since the last scan.
    pub missing: bool,
}

/// A recorded or imported meeting (`meetings`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Meeting {
    pub id: MeetingId,
    /// ULID used for folder names and exports.
    pub uid: String,
    pub title: String,
    pub source: Source,
    /// Original file path when `source` is `file`.
    pub source_path: Option<String>,
    /// BCP-47 tag or `auto`.
    pub language: String,
    /// Live: diarize the mic channel too (several people in the room).
    pub diarize_mic: bool,
    /// Unix ms, wall clock.
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub status: MeetingStatus,
    /// The transcript run the UI shows.
    pub current_run_id: Option<RunId>,
    /// Pinned: exempt from audio retention.
    pub audio_keep: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One transcript of a meeting (`transcript_runs`). Runs are never overwritten.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TranscriptRun {
    pub id: RunId,
    pub meeting_id: MeetingId,
    pub kind: RunKind,
    /// Runtime version, model names and hashes, presets, fusion settings.
    pub engine_info: serde_json::Value,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub status: RunStatus,
}

/// A stored audio file of a meeting (`audio_tracks`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AudioTrack {
    pub id: TrackId,
    pub meeting_id: MeetingId,
    pub channel: TrackChannel,
    /// Relative to the data root, or absolute for `original`.
    pub path: String,
    pub format: AudioFormat,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_ms: Option<i64>,
    pub size_bytes: Option<i64>,
}

/// Live alignment anchor: one per channel per second (`clock_anchors`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockAnchor {
    pub meeting_id: MeetingId,
    pub channel: Channel,
    /// Position in the channel's 16 kHz stream.
    pub sample_index: i64,
    /// QueryPerformanceCounter time in 100 ns units.
    pub qpc_100ns: i64,
}

/// A speaker label within one run (`speakers`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Speaker {
    pub id: SpeakerId,
    pub run_id: RunId,
    pub meeting_id: MeetingId,
    pub channel: Channel,
    /// `spk_0`.., `me`, `spk_unknown` or `user_N`.
    pub engine_label: String,
    /// User-set name; `None` shows the default ("Speaker N", "Me").
    pub display_name: Option<String>,
    /// `#rrggbb`; `None` uses the default palette.
    pub color: Option<String>,
    /// Cross-meeting identity (v3).
    pub global_id: Option<i64>,
    /// Root speaker this label was merged into (chains are always one level deep).
    pub merged_into: Option<SpeakerId>,
    /// Carry-over from a previous run was ambiguous; the UI shows a badge.
    pub needs_review: bool,
    /// Time range of the segment the user renamed from, used by carry-over.
    pub name_anchor_start_ms: Option<i64>,
    pub name_anchor_end_ms: Option<i64>,
}

/// Contiguous same-speaker run of words (`segments`). The segment owns the speaker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Segment {
    pub id: SegmentId,
    pub run_id: RunId,
    pub meeting_id: MeetingId,
    pub speaker_id: SpeakerId,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    /// Display order within the run.
    pub seq: i64,
}

/// The atomic transcript unit (`words`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Word {
    pub id: WordId,
    pub segment_id: SegmentId,
    pub meeting_id: MeetingId,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub confidence: Option<f32>,
    /// Engine speaker label before smoothing or edits.
    pub raw_label: Option<String>,
    /// v1.5 fusion: top speaker score minus runner-up.
    pub margin: Option<f32>,
    /// v1.5 fusion: another speaker was active during the word.
    pub overlap: bool,
}

/// A mic word the echo gate removed (`transcript_runs.echo_dropped`, plan
/// 15): far-end speech the microphone picked up from the speakers. Meeting
/// time; `coefficient` is the word's correlation with the loopback (0 when
/// it was not scored), `reason` the layer that removed it (absent on runs
/// from before plan 15 section 9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EchoDroppedWord {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub coefficient: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<EchoReason>,
}

/// Which layer of the echo gate removed a mic word (plan 15 section 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum EchoReason {
    /// It repeats the far end's words, in a delay-consistent run.
    Text,
    /// The canceller removed most of its energy (post-pass).
    Attenuation,
    /// Its level sits at the mic's noise floor while the far end talks.
    Level,
    /// It correlates with the loopback.
    Correlation,
    /// A fragment inside an utterance the other layers found mostly echo.
    Utterance,
    /// After the post-pass canceller: the uncancelled mic's word at the same
    /// time was echo (plan 15 section 9.2).
    Transfer,
}

impl EchoReason {
    pub const ALL: [Self; 6] = [
        Self::Text,
        Self::Attenuation,
        Self::Level,
        Self::Correlation,
        Self::Utterance,
        Self::Transfer,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Attenuation => "attenuation",
            Self::Level => "level",
            Self::Correlation => "correlation",
            Self::Utterance => "utterance",
            Self::Transfer => "transfer",
        }
    }
}

/// One revision of a live utterance (`live_utterances`), append-only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveUtterance {
    pub id: i64,
    pub run_id: RunId,
    pub channel: Channel,
    pub utt: u32,
    pub rev: u32,
    pub start_ms: i64,
    pub end_ms: i64,
    pub raw_label: Option<String>,
    /// Stored as `words_json`.
    pub words: Vec<EngineWord>,
}

/// Target of a reassign edit (`edits.payload`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReassignTarget {
    pub to_display_name: Option<String>,
    pub to_engine_label: Option<String>,
}

/// A time-ranged user edit that is replayed onto new runs (`edits`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    pub id: i64,
    pub meeting_id: MeetingId,
    pub kind: EditKind,
    pub channel: Channel,
    pub start_ms: i64,
    pub end_ms: i64,
    pub payload: ReassignTarget,
    pub created_at: i64,
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
        assert_ne!(JobId::new(), JobId::new());
    }

    fn strings<T: fmt::Display>(all: &[T]) -> Vec<String> {
        all.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn enums_match_schema_check_constraints() {
        assert_eq!(strings(Source::ALL), ["live", "file"]);
        assert_eq!(strings(Channel::ALL), ["mic", "loopback", "source"]);
        assert_eq!(
            strings(TrackChannel::ALL),
            ["mic", "loopback", "source", "playback"]
        );
        assert_eq!(strings(AudioFormat::ALL), ["wav", "flac", "original"]);
        assert_eq!(
            strings(MeetingStatus::ALL),
            ["recording", "processing", "done", "failed"]
        );
        assert_eq!(
            strings(RunKind::ALL),
            ["live", "file", "post_pass", "rediarize", "refuse"]
        );
        assert_eq!(strings(RunStatus::ALL), ["running", "done", "failed"]);
        assert_eq!(strings(EditKind::ALL), ["reassign"]);
        assert_eq!(
            strings(ModelRole::ALL),
            [
                "asr",
                "diarizer",
                "embedder",
                "punctuation",
                "llm",
                "engine",
                "unknown"
            ]
        );
        assert_eq!(strings(ModelSource::ALL), ["manifest", "pull", "user"]);
    }

    #[test]
    fn enums_round_trip_through_str_and_serde() {
        for &k in RunKind::ALL {
            assert_eq!(k.as_str().parse::<RunKind>().unwrap(), k);
            let j = serde_json::to_string(&k).unwrap();
            assert_eq!(j, format!("\"{}\"", k.as_str()));
            assert_eq!(serde_json::from_str::<RunKind>(&j).unwrap(), k);
        }
        for &c in TrackChannel::ALL {
            assert_eq!(c.as_str().parse::<TrackChannel>().unwrap(), c);
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
        assert!("playback".parse::<Channel>().is_err());
        assert!("Done".parse::<MeetingStatus>().is_err());
    }

    #[test]
    fn ids_serialize_as_plain_numbers() {
        assert_eq!(serde_json::to_string(&MeetingId(42)).unwrap(), "42");
        assert_eq!(
            serde_json::from_str::<SpeakerId>("7").unwrap(),
            SpeakerId(7)
        );
        assert_eq!(serde_json::to_string(&JobId("x".into())).unwrap(), "\"x\"");
        assert_eq!(MeetingId(3).to_string(), "3");
    }

    #[test]
    fn meeting_serializes_with_column_names() {
        let m = Meeting {
            id: MeetingId(1),
            uid: new_id(),
            title: "Weekly sync".into(),
            source: Source::File,
            source_path: Some("C:/rec/sync.m4a".into()),
            language: "en".into(),
            diarize_mic: false,
            started_at: 1_760_000_000_000,
            ended_at: Some(1_760_000_600_000),
            duration_ms: Some(600_000),
            status: MeetingStatus::Done,
            current_run_id: Some(RunId(9)),
            audio_keep: true,
            created_at: 1_760_000_000_000,
            updated_at: 1_760_000_700_000,
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["id"], 1);
        assert_eq!(v["source"], "file");
        assert_eq!(v["status"], "done");
        assert_eq!(v["current_run_id"], 9);
        assert_eq!(v["audio_keep"], true);
        let back: Meeting = serde_json::from_value(v).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn run_and_word_round_trip() {
        let run = TranscriptRun {
            id: RunId(2),
            meeting_id: MeetingId(1),
            kind: RunKind::PostPass,
            engine_info: json!({"runtime": "nemo-speech 0.2.0"}),
            started_at: 1,
            finished_at: None,
            status: RunStatus::Running,
        };
        let w = Word {
            id: WordId(42),
            segment_id: SegmentId(5),
            meeting_id: MeetingId(1),
            start_ms: 12_340,
            end_ms: 12_610,
            text: "hello".into(),
            confidence: Some(0.91),
            raw_label: Some("spk_1".into()),
            margin: None,
            overlap: false,
        };
        let run2: TranscriptRun =
            serde_json::from_str(&serde_json::to_string(&run).unwrap()).unwrap();
        let w2: Word = serde_json::from_str(&serde_json::to_string(&w).unwrap()).unwrap();
        assert_eq!(run2, run);
        assert_eq!(w2, w);
        assert_eq!(serde_json::to_value(&run).unwrap()["kind"], "post_pass");
    }
}
