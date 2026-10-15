//! Inference engine contract (`docs/03-engine-protocol.md` v0.2) and, from
//! Phase 1, its adapters.
//!
//! The rest of the app talks only to these traits. Adapters (`NemoCliEngine`
//! for file jobs, `NemoWsLiveEngine` for live sessions, later an FFI sidecar)
//! implement them over whatever the runtime offers and translate its output
//! into [`EngineEvent`]s; nothing runtime-specific leaks past this module.
//! Engines run as child processes so a runtime crash never takes the app down.

use std::sync::mpsc::Receiver;

use crate::error::Result;

pub mod live_ws;
pub mod nemo_cli;
pub mod process;

pub use crate::protocol::{
    DiarPreset, EngineEvent, EngineInfo, EngineWord, ErrorCode, FileJob, JobStats, LiveOpts,
};
use crate::types::{Channel, JobId};
pub use live_ws::{NemoWsLiveEngine, ServeConfig, ServeStatus, ServeSupervisor};
pub use nemo_cli::{DiarizeRunner, NemoCascadeEngine, NemoCliEngine};

/// Whole-file transcription (and diarization).
pub trait Engine: Send + Sync {
    /// Starts transcribing a 16 kHz mono PCM16 WAV. Events arrive on the
    /// receiver; the last one is [`EngineEvent::Done`] or [`EngineEvent::Error`].
    fn transcribe_file(&self, job: FileJob) -> Result<Receiver<EngineEvent>>;

    /// Requests cancellation; the job ends with an `Error` event. Unknown or
    /// finished jobs are ignored.
    fn cancel(&self, job_id: JobId);

    /// Runtime version, models and device.
    fn info(&self) -> EngineInfo;
}

/// Live transcription: one session per channel.
pub trait LiveEngine: Send + Sync {
    fn open(&self, job_id: JobId, channel: Channel, opts: LiveOpts)
        -> Result<Box<dyn LiveSession>>;
}

/// One open live session.
pub trait LiveSession: Send {
    /// Feeds 16 kHz mono PCM16 of any length; `pts_us` is the channel-sample
    /// time of the first sample in microseconds.
    fn push(&mut self, pcm: &[i16], pts_us: u64) -> Result<()>;

    /// Events produced so far and from now on.
    fn events(&mut self) -> &mut Receiver<EngineEvent>;

    /// Flushes, emits the final events and `Done`, and ends the session.
    fn close(self: Box<Self>) -> Result<()>;
}
