# Engine contract

Status: v0.2, 15 Oct 2026. The contract is a pair of Rust traits plus an event enum in `crates/core/src/engine/mod.rs`. Adapters implement it over whatever the runtime offers: a spawned CLI for files, a WebSocket server or an FFI sidecar for live. A wire format is defined only if an FFI sidecar is built (appendix).

## Traits

```rust
pub trait Engine: Send + Sync {
    /// Transcribe a 16 kHz mono PCM16 WAV. Events arrive on the receiver; the last one is Done or Error.
    fn transcribe_file(&self, job: FileJob) -> Result<Receiver<EngineEvent>>;
    fn cancel(&self, job_id: JobId);
    fn info(&self) -> EngineInfo;            // runtime version, models, device
}

pub trait LiveEngine: Send + Sync {
    fn open(&self, job_id: JobId, channel: Channel, opts: LiveOpts) -> Result<Box<dyn LiveSession>>;
}

pub trait LiveSession: Send {
    /// 16 kHz mono PCM16, any length; pts is channel-sample time in microseconds.
    fn push(&mut self, pcm: &[i16], pts_us: u64) -> Result<()>;
    fn events(&mut self) -> &mut Receiver<EngineEvent>;
    fn close(self: Box<Self>) -> Result<()>;   // flush, then Done
}

pub struct FileJob { pub job_id: JobId, pub wav_path: PathBuf, pub diarize: bool, pub language: String, pub diar_preset: DiarPreset }
pub struct LiveOpts { pub diarize: bool, pub language: String }
```

## Events

```rust
pub enum EngineEvent {
    Ready    { info: EngineInfo },
    Progress { job_id, done_ms: u64, total_ms: Option<u64> },
    /// Whole-utterance replacement: a later rev of the same utt replaces the earlier one entirely.
    Utterance { job_id, channel: Channel, utt: u32, rev: u32, words: Vec<EngineWord> },
    /// Text before t will not change (no new rev will touch utterances ending before t).
    TextFinalUntil { job_id, channel: Channel, t_us: u64 },
    /// Speaker labels before t will not change. Live adapters may never send this; the post-pass sets it to the end.
    SpeakersFinalUntil { job_id, channel: Channel, t_us: u64 },
    /// Reserved: relabel a window. Not produced by any current adapter.
    Relabel  { job_id, channel: Channel, from_us: u64, to_us: u64, map: HashMap<String, String> },
    Done     { job_id, stats: JobStats },
    Error    { job_id, code: ErrorCode, message: String },
}

pub struct EngineWord { pub t0_us: u64, pub t1_us: u64, pub text: String, pub conf: Option<f32>, pub spk: Option<String> }
pub enum ErrorCode { Oom, Crash, BadInput, ModelMissing, Timeout, Other }
```

Rules:
- Timestamps are channel-sample time (position in that channel's 16 kHz stream). The app maps them to meeting time through `clock_anchors` when interleaving channels.
- `spk` is the adapter-normalized label: `spk_0`.. for diarized channels, `None` when the runtime gave no speaker. The app assigns `me` for an undiarized mic channel.
- For file jobs, adapters emit one or more `Utterance` events with `rev = 0`, then `TextFinalUntil` and `SpeakersFinalUntil` at the end of the audio, then `Done`.
- For live jobs, the UI treats everything after `TextFinalUntil` as tentative text and every speaker label as provisional until the post-pass. If a live adapter never sends `TextFinalUntil`, the orchestrator treats utterances older than 10 s as text-final.
- Any adapter failure becomes `Error` with the last 2 kB of runtime stderr in `message`. The app never panics on engine output.

## v1 adapter: `NemoCliEngine`

Spawns `nemo-speech.exe transcribe <wav> --diarize --json --output <tmp.json> --device cuda:0` plus the model and preset flags recorded in `docs/research/03-runtime-spike.md`. Parses `words[{word, start, end, confidence, speaker?}]` (seconds, speaker 1-based, absent when none) into one `Utterance` per contiguous same-speaker stretch. Confidence is always 1.0 in runtime v0.2.0 and is stored as `None`.

Long files: if the runtime rejects or degrades on inputs over its positional limit, the adapter falls back to one diarization pass over the whole file (`diarize` subcommand, `v3-offline` preset) plus ASR over silence-cut chunks of at most 10 minutes, fused by the start-anchored window rule from research/04. This fallback is a Phase 1 acceptance test on the 77-minute CIMC recording.

## Performance targets

- File mode: real-time factor under 0.1 for ASR plus diarization on the RTX 3080 (one hour in under six minutes), measured on the eval set.
- Live: first `Utterance` within 1.5 s of speech; post-pass canonical transcript within 60 s of Stop for a one-hour meeting.
- VRAM: ASR plus diarizer resident under 4 GB; the orchestrator checks free VRAM via NVML before opening live sessions and refuses with a clear message when under 2 GB free.

## Appendix: wire format for an FFI sidecar (only if built)

stdin: `u8 type` + `u32 LE length` + payload; type 1 = JSON control, type 2 = `u8 channel` + `u64 LE pts_us` + PCM16. stdout: newline-delimited JSON mirroring `EngineEvent` with `serde` tag `ev`. stderr: logs.
