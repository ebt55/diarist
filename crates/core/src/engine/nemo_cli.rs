//! `NemoCliEngine`: the v1 file adapter (`docs/03` v0.2, research/08 section 1).
//!
//! One `nemo-speech.exe` child per job:
//!
//! `transcribe <wav> --model <asr.gguf> [--language <lang>] --stream --endpointing
//!  --diarize --diar-model <diar.gguf> --device cuda:0 --json --output <words.json> --force`
//!
//! with the reduced `PATH` and `NEMO_SPEECH_MODEL_DIR` from
//! [`super::process`]. The runtime prints no progress, so `Progress` is
//! extrapolated from elapsed time at the measured 26x real time. Its JSON
//! (`words[{word,start,end,confidence,speaker?}]`, speaker 1-based and absent
//! when untagged) becomes one `Utterance` per contiguous same-speaker
//! stretch, then `TextFinalUntil`, `SpeakersFinalUntil` and `Done`. Exit
//! codes 1 to 4, a crash, a missing output file, or the one-line JSON error
//! on stderr become `Error` with the stderr tail. `cancel` kills the child.
//!
//! The JSON (`<wav stem>.words.json`) and the runtime's stderr
//! (`engine.log`) stay in the WAV's folder, i.e. the meeting folder.
//!
//! Also here: [`DiarizeRunner`] for the optional standalone `diarize --format
//! rttm` pass, and the experimental [`NemoCascadeEngine`] (Parakeet without
//! diarization, then `diarize`, then [`crate::fusion::assign_words`]).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::process;
use super::{
    DiarPreset, Engine, EngineEvent, EngineInfo, EngineWord, ErrorCode, FileJob, JobStats,
};
use crate::error::Result;
use crate::fusion::{assign_words, AsrWord, DiarSegment};
use crate::transcript::LabelMap;
use crate::types::{Channel, JobId};

/// Measured speed of the streaming file pass on the RTX 3080 (research/08 section 7).
pub const EXPECTED_REALTIME_FACTOR: f64 = 26.0;
/// Bytes of stderr kept for error messages.
const STDERR_TAIL: usize = 2_048;

// ---- output parsing ---------------------------------------------------------------

/// `transcribe --json` output.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TranscribeOutput {
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub text: String,
    /// Audio seconds.
    #[serde(default)]
    pub duration: f64,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub words: Vec<RawWord>,
}

/// One word as the runtime prints it (seconds, 80 ms grid).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RawWord {
    pub word: String,
    pub start: f64,
    pub end: f64,
    #[serde(default)]
    pub confidence: Option<f64>,
    /// 1-based; absent when untagged or when diarization is off.
    #[serde(default)]
    pub speaker: Option<u32>,
}

/// Parses `transcribe --json` output.
pub fn parse_transcribe_json(json: &str) -> std::result::Result<TranscribeOutput, String> {
    serde_json::from_str(json).map_err(|e| format!("transcribe output: {e}"))
}

fn secs_to_us(s: f64) -> u64 {
    (s.max(0.0) * 1_000_000.0).round() as u64
}

/// Words with speakers normalized to `spk_N` by first appearance. Confidence
/// is always 1.0 in v0.2.0 (upstream #55), so it is dropped.
pub fn engine_words(out: &TranscribeOutput) -> Vec<EngineWord> {
    let mut labels = LabelMap::new();
    out.words
        .iter()
        .map(|w| EngineWord {
            t0_us: secs_to_us(w.start),
            t1_us: secs_to_us(w.end.max(w.start)),
            text: w.word.clone(),
            conf: None,
            spk: w.speaker.map(|s| labels.label(Some(s))),
        })
        .collect()
}

/// The event sequence of a finished file job: one `Utterance` (rev 0) per
/// contiguous same-speaker stretch, then `TextFinalUntil` and
/// `SpeakersFinalUntil` at the end of the audio, then `Done`.
pub fn file_events(
    job_id: &JobId,
    channel: Channel,
    words: Vec<EngineWord>,
    audio_us: u64,
    wall: Duration,
) -> Vec<EngineEvent> {
    let speakers = words
        .iter()
        .filter_map(|w| w.spk.as_deref())
        .collect::<std::collections::HashSet<_>>()
        .len() as u32;
    let end_us = words
        .iter()
        .map(|w| w.t1_us)
        .max()
        .unwrap_or(0)
        .max(audio_us);
    let mut events = Vec::new();
    let mut utt = 0u32;
    let mut current: Vec<EngineWord> = Vec::new();
    for w in words {
        if current.last().is_some_and(|p| p.spk != w.spk) {
            events.push(EngineEvent::Utterance {
                job_id: job_id.clone(),
                channel,
                utt,
                rev: 0,
                words: std::mem::take(&mut current),
            });
            utt += 1;
        }
        current.push(w);
    }
    if !current.is_empty() {
        events.push(EngineEvent::Utterance {
            job_id: job_id.clone(),
            channel,
            utt,
            rev: 0,
            words: current,
        });
    }
    events.push(EngineEvent::TextFinalUntil {
        job_id: job_id.clone(),
        channel,
        t_us: end_us,
    });
    events.push(EngineEvent::SpeakersFinalUntil {
        job_id: job_id.clone(),
        channel,
        t_us: end_us,
    });
    let audio_ms = end_us / 1_000;
    let wall_ms = wall.as_millis() as u64;
    events.push(EngineEvent::Done {
        job_id: job_id.clone(),
        stats: JobStats {
            rtf: (audio_ms > 0).then(|| wall_ms as f64 / audio_ms as f64),
            speakers: Some(speakers),
            audio_ms: Some(audio_ms),
            wall_ms: Some(wall_ms),
        },
    });
    events
}

/// The runtime's one-line JSON error on stderr.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RuntimeError {
    pub message: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub exit_code: Option<i32>,
}

/// Finds `{"error":{...}}` in stderr.
pub fn find_runtime_error(stderr: &str) -> Option<RuntimeError> {
    #[derive(Deserialize)]
    struct Wrapper {
        error: RuntimeError,
    }
    stderr.lines().rev().find_map(|line| {
        let line = line.trim();
        line.starts_with("{\"error\"")
            .then(|| serde_json::from_str::<Wrapper>(line).ok())
            .flatten()
            .map(|w| w.error)
    })
}

/// Maps a failed run to an error code and message (JSON error message,
/// then the stderr tail).
pub fn map_failure(exit_code: Option<i32>, stderr: &str) -> (ErrorCode, String) {
    let json = find_runtime_error(stderr);
    let lower = stderr.to_ascii_lowercase();
    let oom = [
        "out of memory",
        "cudamalloc",
        "cudaerrormemoryallocation",
        "oom",
    ]
    .iter()
    .any(|p| lower.contains(p));
    let code = match exit_code {
        _ if oom => ErrorCode::Oom,
        Some(2) => ErrorCode::BadInput,
        Some(3) => ErrorCode::ModelMissing,
        Some(1) => {
            if ["wav", "riff", "not a file", "unsupported audio"]
                .iter()
                .any(|p| lower.contains(p))
            {
                ErrorCode::BadInput
            } else {
                ErrorCode::Other
            }
        }
        Some(4) => ErrorCode::Other,
        _ => ErrorCode::Crash,
    };
    let head = match (&json, exit_code) {
        (Some(e), _) => format!("{} ({})", e.message, e.kind),
        (None, Some(c)) => format!("nemo-speech exited with code {c}"),
        (None, None) => "nemo-speech was terminated".to_owned(),
    };
    let tail = process::tail(stderr, STDERR_TAIL);
    let message = if tail.is_empty() {
        head
    } else {
        format!("{head}\n{tail}")
    };
    (code, message)
}

/// Parses RTTM lines (`SPEAKER <rec> 1 <start> <dur> <NA> <NA> speaker_<N> <NA> <NA>`).
pub fn parse_rttm(text: &str) -> Vec<DiarSegment> {
    let mut labels: HashMap<String, u32> = HashMap::new();
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 8 || f[0] != "SPEAKER" {
                return None;
            }
            let start: f64 = f[3].parse().ok()?;
            let dur: f64 = f[4].parse().ok()?;
            let label = f[7];
            let next = labels.len() as u32 + 1;
            let speaker = label
                .rsplit('_')
                .next()
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or_else(|| *labels.entry(label.to_owned()).or_insert(next));
            Some(DiarSegment {
                start_ms: (start * 1_000.0).round() as i64,
                end_ms: ((start + dur) * 1_000.0).round() as i64,
                speaker,
            })
        })
        .collect()
}

/// Parses `diarize --format json` output.
pub fn parse_diarize_json(json: &str) -> std::result::Result<Vec<DiarSegment>, String> {
    #[derive(Deserialize)]
    struct Seg {
        start: f64,
        end: f64,
        speaker: u32,
    }
    #[derive(Deserialize)]
    struct Out {
        segments: Vec<Seg>,
    }
    let out: Out = serde_json::from_str(json).map_err(|e| format!("diarize output: {e}"))?;
    Ok(out
        .segments
        .into_iter()
        .map(|s| DiarSegment {
            start_ms: (s.start * 1_000.0).round() as i64,
            end_ms: (s.end * 1_000.0).round() as i64,
            speaker: s.speaker,
        })
        .collect())
}

/// Duration of a WAV in microseconds (`None` if unreadable).
fn wav_duration_us(path: &Path) -> Option<u64> {
    let r = hound::WavReader::open(path).ok()?;
    let rate = u64::from(r.spec().sample_rate).max(1);
    Some(u64::from(r.duration()) * 1_000_000 / rate)
}

// ---- running a child ------------------------------------------------------------------

/// A running child that `cancel` can reach.
struct RunningJob {
    child: Mutex<Option<Child>>,
    cancelled: AtomicBool,
}

type Running = Arc<Mutex<HashMap<JobId, Arc<RunningJob>>>>;

/// Why a child run did not succeed.
enum RunFailure {
    Cancelled,
    Failed(ErrorCode, String),
}

/// Spawns `cmd`, tees stderr into `log`, emits `Progress` from elapsed time
/// (scaled into `[progress_from, progress_to]` of `total_us`), and waits.
#[allow(clippy::too_many_arguments)]
fn run_child(
    mut cmd: Command,
    job_id: &JobId,
    job: &Arc<RunningJob>,
    tx: &Sender<EngineEvent>,
    log: &Path,
    total_us: Option<u64>,
    progress: (f64, f64),
    realtime_factor: f64,
) -> std::result::Result<(), RunFailure> {
    if job.cancelled.load(Ordering::SeqCst) {
        return Err(RunFailure::Cancelled);
    }
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut log_file = OpenOptions::new().create(true).append(true).open(log).ok();
    if let Some(f) = log_file.as_mut() {
        let _ = writeln!(f, "\n$ {cmd:?}");
    }
    let mut child = cmd.spawn().map_err(|e| {
        RunFailure::Failed(
            ErrorCode::Crash,
            format!("cannot start {:?}: {e}", cmd.get_program()),
        )
    })?;
    let stderr = child.stderr.take();
    let tail: Arc<Mutex<VecDeque<String>>> = Arc::default();
    let tail_w = Arc::clone(&tail);
    let reader = std::thread::spawn(move || {
        let Some(stderr) = stderr else { return };
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if let Some(f) = log_file.as_mut() {
                let _ = writeln!(f, "{line}");
            }
            if line.trim().is_empty() {
                continue;
            }
            let mut t = tail_w.lock().unwrap_or_else(|e| e.into_inner());
            t.push_back(line);
            while t.len() > 64 {
                t.pop_front();
            }
        }
    });
    *job.child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);

    let start = Instant::now();
    let mut last_progress = Instant::now() - Duration::from_secs(10);
    let status = loop {
        {
            let mut guard = job.child.lock().unwrap_or_else(|e| e.into_inner());
            match guard.as_mut().map(Child::try_wait) {
                Some(Ok(Some(status))) => break Ok(status),
                Some(Ok(None)) => {}
                Some(Err(e)) => break Err(e),
                None => break Err(std::io::Error::other("child vanished")),
            }
        }
        if last_progress.elapsed() >= Duration::from_secs(1) {
            last_progress = Instant::now();
            if let Some(total) = total_us {
                let span = progress.1 - progress.0;
                let est_us = start.elapsed().as_secs_f64() * realtime_factor * 1e6;
                let frac = (est_us / total as f64).min(0.99);
                let done = ((progress.0 + span * frac) * total as f64) as u64;
                let _ = tx.send(EngineEvent::Progress {
                    job_id: job_id.clone(),
                    done_ms: done / 1_000,
                    total_ms: Some(total / 1_000),
                });
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    job.child.lock().unwrap_or_else(|e| e.into_inner()).take();
    let _ = reader.join();
    let stderr_text = {
        let t = tail.lock().unwrap_or_else(|e| e.into_inner());
        t.iter().cloned().collect::<Vec<_>>().join("\n")
    };
    if job.cancelled.load(Ordering::SeqCst) {
        return Err(RunFailure::Cancelled);
    }
    let status = status.map_err(|e| RunFailure::Failed(ErrorCode::Crash, e.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        let (code, msg) = map_failure(
            status.code().filter(|c| (0..=255).contains(c)),
            &stderr_text,
        );
        Err(RunFailure::Failed(code, msg))
    }
}

fn send_failure(tx: &Sender<EngineEvent>, job_id: &JobId, f: RunFailure) {
    let (code, message) = match f {
        RunFailure::Cancelled => (ErrorCode::Cancelled, "cancelled".to_owned()),
        RunFailure::Failed(c, m) => (c, m),
    };
    let _ = tx.send(EngineEvent::Error {
        job_id: job_id.clone(),
        code,
        message,
    });
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn missing(tx: &Sender<EngineEvent>, job_id: &JobId, what: &str, path: &Path) -> bool {
    if path.is_file() {
        return false;
    }
    let code = if what == "audio" {
        ErrorCode::BadInput
    } else {
        ErrorCode::ModelMissing
    };
    let _ = tx.send(EngineEvent::Error {
        job_id: job_id.clone(),
        code,
        message: format!("{what} file not found: {}", path.display()),
    });
    true
}

// ---- NemoCliEngine ------------------------------------------------------------------

/// The v1 file adapter. See the module docs.
pub struct NemoCliEngine {
    /// Runtime folder (holds `bin\nemo-speech.exe`).
    pub engine_dir: PathBuf,
    /// `NEMO_SPEECH_MODEL_DIR`.
    pub models_dir: PathBuf,
    /// `cuda:0`, `cpu`, ...
    pub device: String,
    /// Nemotron 3.5 (default) or the EN fallback; always an absolute path.
    pub asr_model: PathBuf,
    /// Nemotron 3 Diarization.
    pub diar_model: PathBuf,
    /// Runtime version for `info()` (e.g. from `doctor`).
    pub version: Option<String>,
    /// Speed assumed for `Progress`.
    pub realtime_factor: f64,
    running: Running,
}

impl NemoCliEngine {
    pub fn new(
        engine_dir: impl Into<PathBuf>,
        models_dir: impl Into<PathBuf>,
        asr_model: impl Into<PathBuf>,
        diar_model: impl Into<PathBuf>,
    ) -> Self {
        Self {
            engine_dir: engine_dir.into(),
            models_dir: models_dir.into(),
            device: "cuda:0".into(),
            asr_model: asr_model.into(),
            diar_model: diar_model.into(),
            version: None,
            realtime_factor: EXPECTED_REALTIME_FACTOR,
            running: Running::default(),
        }
    }

    /// The exact argument list of a job (after the executable).
    pub fn args(&self, job: &FileJob, output: &Path) -> Vec<std::ffi::OsString> {
        let mut a: Vec<std::ffi::OsString> =
            vec!["transcribe".into(), process::path_arg(&job.wav_path)];
        a.push("--model".into());
        a.push(process::path_arg(&self.asr_model));
        let lang = job.language.trim();
        if !lang.is_empty() && !lang.eq_ignore_ascii_case("auto") {
            a.push("--language".into());
            a.push(lang.into());
        }
        a.push("--stream".into());
        a.push("--endpointing".into());
        if job.diarize {
            a.push("--diarize".into());
            a.push("--diar-model".into());
            a.push(process::path_arg(&self.diar_model));
        }
        for s in ["--device", &self.device, "--json", "--output"] {
            a.push(s.into());
        }
        a.push(process::path_arg(output));
        a.push("--force".into());
        a
    }

    /// Where the words JSON of `wav` goes: `<wav stem>.words.json` next to it.
    pub fn output_path(wav: &Path) -> PathBuf {
        wav.with_extension("words.json")
    }

    fn register(&self, job_id: &JobId) -> Arc<RunningJob> {
        let job = Arc::new(RunningJob {
            child: Mutex::new(None),
            cancelled: AtomicBool::new(false),
        });
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(job_id.clone(), Arc::clone(&job));
        job
    }
}

impl Engine for NemoCliEngine {
    fn transcribe_file(&self, job: FileJob) -> Result<Receiver<EngineEvent>> {
        let (tx, rx) = channel();
        let running_job = self.register(&job.job_id);
        let running = Arc::clone(&self.running);
        let output = Self::output_path(&job.wav_path);
        let log = job
            .wav_path
            .parent()
            .map(|d| d.join("engine.log"))
            .unwrap_or_else(|| PathBuf::from("engine.log"));
        let mut cmd = process::command(&self.engine_dir, Some(&self.models_dir));
        cmd.args(self.args(&job, &output));
        let checks = [
            ("engine", process::exe_path(&self.engine_dir)),
            ("ASR model", self.asr_model.clone()),
            ("audio", job.wav_path.clone()),
        ];
        let diar = job.diarize.then(|| self.diar_model.clone());
        let rtf = self.realtime_factor;
        std::thread::Builder::new()
            .name(format!("nemo-cli-{}", job.job_id))
            .spawn(move || {
                let id = job.job_id.clone();
                let ok = checks.iter().all(|(what, p)| !missing(&tx, &id, what, p))
                    && diar
                        .as_ref()
                        .is_none_or(|d| !missing(&tx, &id, "diarizer model", d));
                if ok {
                    let _ = std::fs::remove_file(&output);
                    let total = wav_duration_us(&job.wav_path);
                    let started = Instant::now();
                    match run_child(cmd, &id, &running_job, &tx, &log, total, (0.0, 1.0), rtf) {
                        Ok(()) => match std::fs::read_to_string(&output)
                            .map_err(|e| format!("no output {}: {e}", output.display()))
                            .and_then(|s| parse_transcribe_json(&s))
                        {
                            Ok(out) => {
                                let audio_us = total.unwrap_or_else(|| secs_to_us(out.duration));
                                for ev in file_events(
                                    &id,
                                    Channel::Source,
                                    engine_words(&out),
                                    audio_us,
                                    started.elapsed(),
                                ) {
                                    let _ = tx.send(ev);
                                }
                            }
                            Err(message) => {
                                let _ = tx.send(EngineEvent::Error {
                                    job_id: id.clone(),
                                    code: ErrorCode::Crash,
                                    message,
                                });
                            }
                        },
                        Err(f) => send_failure(&tx, &id, f),
                    }
                }
                running
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
            })
            .map_err(|e| crate::error::CoreError::Engine(format!("spawn job thread: {e}")))?;
        Ok(rx)
    }

    fn cancel(&self, job_id: JobId) {
        let job = self
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&job_id)
            .cloned();
        if let Some(job) = job {
            job.cancelled.store(true, Ordering::SeqCst);
            if let Some(child) = job.child.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                let _ = child.kill();
            }
        }
    }

    fn info(&self) -> EngineInfo {
        let mut models = BTreeMap::new();
        models.insert("asr".to_owned(), file_name(&self.asr_model));
        models.insert("diarizer".to_owned(), file_name(&self.diar_model));
        EngineInfo {
            runtime: format!(
                "nemo-speech {}",
                self.version.as_deref().unwrap_or("unknown")
            ),
            models,
            device: self.device.clone(),
        }
    }
}

// ---- DiarizeRunner -------------------------------------------------------------------------

/// The optional standalone diarization pass (`diarize --format rttm`),
/// kept next to the transcript for the eval harness and re-fusion.
#[derive(Debug, Clone)]
pub struct DiarizeRunner {
    pub engine_dir: PathBuf,
    pub models_dir: PathBuf,
    pub device: String,
    pub diar_model: PathBuf,
    pub preset: DiarPreset,
}

impl DiarizeRunner {
    pub fn new(
        engine_dir: impl Into<PathBuf>,
        models_dir: impl Into<PathBuf>,
        diar_model: impl Into<PathBuf>,
    ) -> Self {
        Self {
            engine_dir: engine_dir.into(),
            models_dir: models_dir.into(),
            device: "cuda:0".into(),
            diar_model: diar_model.into(),
            preset: DiarPreset::Streaming,
        }
    }

    /// The argument list for one run.
    pub fn args(&self, wav: &Path, rttm: &Path) -> Vec<std::ffi::OsString> {
        let mut a: Vec<std::ffi::OsString> = vec![
            "diarize".into(),
            process::path_arg(wav),
            "--model".into(),
            process::path_arg(&self.diar_model),
            "--device".into(),
            self.device.clone().into(),
        ];
        if self.preset == DiarPreset::Offline {
            a.push("--offline".into());
        }
        for s in ["--format", "rttm", "--output"] {
            a.push(s.into());
        }
        a.push(process::path_arg(rttm));
        a.push("--force".into());
        a
    }

    /// Runs the pass (blocking), writes `rttm` (and `json`, converted from
    /// the RTTM, when given) and returns the segments.
    pub fn run(&self, wav: &Path, rttm: &Path, json: Option<&Path>) -> Result<Vec<DiarSegment>> {
        let job = Arc::new(RunningJob {
            child: Mutex::new(None),
            cancelled: AtomicBool::new(false),
        });
        let (tx, _rx) = channel();
        self.run_job(&JobId::new(), &job, &tx, wav, rttm, json, None, (0.0, 1.0))
    }

    #[allow(clippy::too_many_arguments)]
    fn run_job(
        &self,
        job_id: &JobId,
        job: &Arc<RunningJob>,
        tx: &Sender<EngineEvent>,
        wav: &Path,
        rttm: &Path,
        json: Option<&Path>,
        total_us: Option<u64>,
        progress: (f64, f64),
    ) -> Result<Vec<DiarSegment>> {
        let mut cmd = process::command(&self.engine_dir, Some(&self.models_dir));
        cmd.args(self.args(wav, rttm));
        let log = wav
            .parent()
            .map(|d| d.join("engine.log"))
            .unwrap_or_else(|| PathBuf::from("engine.log"));
        let _ = std::fs::remove_file(rttm);
        // Diarization alone runs far faster than the fused pass.
        run_child(cmd, job_id, job, tx, &log, total_us, progress, 120.0).map_err(|f| {
            crate::error::CoreError::Engine(match f {
                RunFailure::Cancelled => "cancelled".into(),
                RunFailure::Failed(code, msg) => format!("diarize failed ({code:?}): {msg}"),
            })
        })?;
        let text = std::fs::read_to_string(rttm).map_err(|e| {
            crate::error::CoreError::Engine(format!("no RTTM {}: {e}", rttm.display()))
        })?;
        let segs = parse_rttm(&text);
        if let Some(json) = json {
            let value = serde_json::json!({
                "file": wav.display().to_string(),
                "segments": segs.iter().map(|s| serde_json::json!({
                    "start": s.start_ms as f64 / 1_000.0,
                    "end": s.end_ms as f64 / 1_000.0,
                    "speaker": s.speaker,
                })).collect::<Vec<_>>(),
            });
            let mut f = File::create(json)?;
            serde_json::to_writer_pretty(&mut f, &value)?;
        }
        Ok(segs)
    }
}

// ---- NemoCascadeEngine (experimental) ----------------------------------------------------

/// Experimental cascade: Parakeet TDT `transcribe` without diarization, then
/// [`DiarizeRunner`], then [`assign_words`]. Parakeet silently drops spans
/// on some audio (research/08 section 8), so this is never the default.
pub struct NemoCascadeEngine {
    pub engine_dir: PathBuf,
    pub models_dir: PathBuf,
    pub device: String,
    /// Parakeet TDT (offline; rejects `--stream`).
    pub asr_model: PathBuf,
    pub diar_model: PathBuf,
    pub version: Option<String>,
    running: Running,
}

impl NemoCascadeEngine {
    pub fn new(
        engine_dir: impl Into<PathBuf>,
        models_dir: impl Into<PathBuf>,
        asr_model: impl Into<PathBuf>,
        diar_model: impl Into<PathBuf>,
    ) -> Self {
        Self {
            engine_dir: engine_dir.into(),
            models_dir: models_dir.into(),
            device: "cuda:0".into(),
            asr_model: asr_model.into(),
            diar_model: diar_model.into(),
            version: None,
            running: Running::default(),
        }
    }
}

impl Engine for NemoCascadeEngine {
    fn transcribe_file(&self, job: FileJob) -> Result<Receiver<EngineEvent>> {
        let (tx, rx) = channel();
        let running_job = Arc::new(RunningJob {
            child: Mutex::new(None),
            cancelled: AtomicBool::new(false),
        });
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(job.job_id.clone(), Arc::clone(&running_job));
        let running = Arc::clone(&self.running);
        let output = NemoCliEngine::output_path(&job.wav_path);
        let dir = job
            .wav_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let log = dir.join("engine.log");
        let mut cmd = process::command(&self.engine_dir, Some(&self.models_dir));
        cmd.arg("transcribe")
            .arg(process::path_arg(&job.wav_path))
            .arg("--model")
            .arg(process::path_arg(&self.asr_model))
            .args(["--device", &self.device, "--json", "--output"])
            .arg(process::path_arg(&output))
            .arg("--force");
        let diarizer = DiarizeRunner {
            engine_dir: self.engine_dir.clone(),
            models_dir: self.models_dir.clone(),
            device: self.device.clone(),
            diar_model: self.diar_model.clone(),
            preset: job.diar_preset,
        };
        let asr_model = self.asr_model.clone();
        std::thread::spawn(move || {
            let id = job.job_id.clone();
            let ok = !missing(&tx, &id, "ASR model", &asr_model)
                && !missing(&tx, &id, "audio", &job.wav_path)
                && (!job.diarize || !missing(&tx, &id, "diarizer model", &diarizer.diar_model));
            if ok {
                let total = wav_duration_us(&job.wav_path);
                let started = Instant::now();
                let result = run_child(cmd, &id, &running_job, &tx, &log, total, (0.0, 0.6), 85.0)
                    .map_err(|f| match f {
                        RunFailure::Cancelled => (ErrorCode::Cancelled, "cancelled".to_owned()),
                        RunFailure::Failed(c, m) => (c, m),
                    })
                    .and_then(|()| {
                        std::fs::read_to_string(&output)
                            .map_err(|e| format!("no output {}: {e}", output.display()))
                            .and_then(|s| parse_transcribe_json(&s))
                            .map_err(|m| (ErrorCode::Crash, m))
                    })
                    .and_then(|out| {
                        let words: Vec<AsrWord> = out
                            .words
                            .iter()
                            .map(|w| AsrWord {
                                start_ms: (w.start * 1_000.0).round() as i64,
                                end_ms: (w.end.max(w.start) * 1_000.0).round() as i64,
                                text: w.word.clone(),
                                raw_label: None,
                            })
                            .collect();
                        if !job.diarize {
                            return Ok((words, Vec::new(), out.duration));
                        }
                        let rttm = dir.join("diarization.rttm");
                        diarizer
                            .run_job(
                                &id,
                                &running_job,
                                &tx,
                                &job.wav_path,
                                &rttm,
                                None,
                                total,
                                (0.6, 1.0),
                            )
                            .map(|segs| (words, segs, out.duration))
                            .map_err(|e| (ErrorCode::Other, e.to_string()))
                    });
                match result {
                    Ok((words, segs, duration)) => {
                        let engine_words: Vec<EngineWord> = if segs.is_empty() {
                            words
                                .iter()
                                .map(|w| EngineWord {
                                    t0_us: w.start_ms as u64 * 1_000,
                                    t1_us: w.end_ms as u64 * 1_000,
                                    text: w.text.clone(),
                                    conf: None,
                                    spk: None,
                                })
                                .collect()
                        } else {
                            assign_words(&words, &segs)
                                .into_iter()
                                .map(|t| EngineWord {
                                    t0_us: t.start_ms.max(0) as u64 * 1_000,
                                    t1_us: t.end_ms.max(0) as u64 * 1_000,
                                    text: t.text,
                                    conf: None,
                                    spk: t.diar_speaker.map(|_| t.speaker),
                                })
                                .collect()
                        };
                        let audio_us = total.unwrap_or_else(|| secs_to_us(duration));
                        for ev in file_events(
                            &id,
                            Channel::Source,
                            engine_words,
                            audio_us,
                            started.elapsed(),
                        ) {
                            let _ = tx.send(ev);
                        }
                    }
                    Err((code, message)) => {
                        let _ = tx.send(EngineEvent::Error {
                            job_id: id.clone(),
                            code,
                            message,
                        });
                    }
                }
            }
            running
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
        });
        Ok(rx)
    }

    fn cancel(&self, job_id: JobId) {
        let job = self
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&job_id)
            .cloned();
        if let Some(job) = job {
            job.cancelled.store(true, Ordering::SeqCst);
            if let Some(child) = job.child.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                let _ = child.kill();
            }
        }
    }

    fn info(&self) -> EngineInfo {
        let mut models = BTreeMap::new();
        models.insert("asr".to_owned(), file_name(&self.asr_model));
        models.insert("diarizer".to_owned(), file_name(&self.diar_model));
        models.insert("fusion".to_owned(), "start-anchored-v1".to_owned());
        EngineInfo {
            runtime: format!(
                "nemo-speech {} (cascade, experimental)",
                self.version.as_deref().unwrap_or("unknown")
            ),
            models,
            device: self.device.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_nemotron35_fixture_into_utterances() {
        let out =
            parse_transcribe_json(&fixture("nemotron35_transcribe_diarize_5min.json")).unwrap();
        assert_eq!(out.words.len(), 464);
        assert!((out.duration - 300.032).abs() < 1e-6);
        let words = engine_words(&out);
        assert!(words.iter().all(|w| w.conf.is_none()));
        assert_eq!(words[0].spk.as_deref(), Some("spk_0"));
        let labels: std::collections::BTreeSet<&str> =
            words.iter().filter_map(|w| w.spk.as_deref()).collect();
        assert_eq!(labels.len(), 4);
        // spk_N numbering follows first appearance of the runtime's 1-based ids.
        let first_raw = out.words[0].speaker.unwrap();
        assert!(out
            .words
            .iter()
            .zip(&words)
            .all(|(r, w)| (r.speaker == Some(first_raw)) == (w.spk.as_deref() == Some("spk_0"))));
        let job = JobId("j".into());
        let events = file_events(
            &job,
            Channel::Source,
            words.clone(),
            300_032_000,
            Duration::from_secs(12),
        );
        let utterances: Vec<&Vec<EngineWord>> = events
            .iter()
            .filter_map(|e| match e {
                EngineEvent::Utterance { words, rev: 0, .. } => Some(words),
                _ => None,
            })
            .collect();
        // Contiguous same-speaker stretches, covering every word in order.
        assert_eq!(utterances.iter().map(|u| u.len()).sum::<usize>(), 464);
        assert!(utterances
            .iter()
            .all(|u| u.iter().all(|w| w.spk == u[0].spk)));
        assert!(utterances.windows(2).all(|p| p[0][0].spk != p[1][0].spk));
        let n = events.len();
        // The last word ends at 300.32 s, past the 300.032 s of audio (80 ms grid).
        assert!(matches!(
            events[n - 3],
            EngineEvent::TextFinalUntil {
                t_us: 300_320_000,
                ..
            }
        ));
        assert!(matches!(
            events[n - 2],
            EngineEvent::SpeakersFinalUntil { .. }
        ));
        let EngineEvent::Done { stats, .. } = &events[n - 1] else {
            panic!("last event is not Done")
        };
        assert_eq!(stats.speakers, Some(4));
        assert_eq!(stats.audio_ms, Some(300_320));
        assert!((stats.rtf.unwrap() - 0.04).abs() < 0.001);
    }

    #[test]
    fn untagged_words_have_no_speaker() {
        let out = parse_transcribe_json(&fixture("parakeet_transcribe_5min.json")).unwrap();
        let words = engine_words(&out);
        assert!(words.iter().all(|w| w.spk.is_none()));
        let events = file_events(
            &JobId("p".into()),
            Channel::Source,
            words,
            0,
            Duration::ZERO,
        );
        // One utterance: no speaker changes.
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, EngineEvent::Utterance { .. }))
                .count(),
            1
        );
        let mixed = parse_transcribe_json(
            r#"{"words":[{"word":"a","start":0.0,"end":0.08,"speaker":2},
                         {"word":"b","start":0.08,"end":0.16},
                         {"word":"c","start":0.16,"end":0.24,"speaker":2}]}"#,
        )
        .unwrap();
        let w = engine_words(&mixed);
        assert_eq!(
            w.iter().map(|w| w.spk.clone()).collect::<Vec<_>>(),
            [Some("spk_0".into()), None, Some("spk_0".into())]
        );
        assert_eq!(w[1].t0_us, 80_000);
    }

    #[test]
    fn maps_exit_codes_and_json_errors() {
        let stderr = "[nemo-speech] transcribe session started\n\n{\"error\":{\"message\":\"nosuch.wav is not a file or directory (run 'nemo-speech help transcribe')\",\"type\":\"invalid_argument\",\"command\":\"transcribe\",\"exit_code\":2}}\n";
        let e = find_runtime_error(stderr).unwrap();
        assert_eq!(e.exit_code, Some(2));
        let (code, msg) = map_failure(Some(2), stderr);
        assert_eq!(code, ErrorCode::BadInput);
        assert!(msg.starts_with("nosuch.wav is not a file"));
        assert_eq!(
            map_failure(Some(3), "model not found").0,
            ErrorCode::ModelMissing
        );
        assert_eq!(map_failure(Some(4), "").0, ErrorCode::Other);
        assert_eq!(
            map_failure(Some(1), "bad RIFF header").0,
            ErrorCode::BadInput
        );
        assert_eq!(
            map_failure(Some(1), "CUDA error: out of memory").0,
            ErrorCode::Oom
        );
        assert_eq!(map_failure(None, "").0, ErrorCode::Crash);
        let (_, long) = map_failure(Some(1), &"x".repeat(10_000));
        assert!(long.len() < 2_200);
    }

    #[test]
    fn exact_command_line() {
        let e = NemoCliEngine::new(
            "E:/engine",
            "E:/models",
            "E:/models/asr.gguf",
            "E:/models/diar.gguf",
        );
        let job = FileJob {
            job_id: JobId("j".into()),
            wav_path: "E:/audio/u/source.wav".into(),
            diarize: true,
            language: "en".into(),
            diar_preset: DiarPreset::Streaming,
        };
        let out = NemoCliEngine::output_path(&job.wav_path);
        assert_eq!(out, Path::new("E:/audio/u/source.words.json"));
        let args: Vec<String> = e
            .args(&job, &out)
            .iter()
            .map(|a| a.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(
            args,
            [
                "transcribe",
                "E:/audio/u/source.wav",
                "--model",
                "E:/models/asr.gguf",
                "--language",
                "en",
                "--stream",
                "--endpointing",
                "--diarize",
                "--diar-model",
                "E:/models/diar.gguf",
                "--device",
                "cuda:0",
                "--json",
                "--output",
                "E:/audio/u/source.words.json",
                "--force"
            ]
        );
        let auto = FileJob {
            language: "auto".into(),
            diarize: false,
            ..job
        };
        let args = e.args(&auto, &out);
        assert!(!args.iter().any(|a| a == "--language" || a == "--diarize"));
        let info = e.info();
        assert_eq!(info.models["asr"], "asr.gguf");
        assert_eq!(info.device, "cuda:0");
    }

    #[test]
    fn diarize_outputs_parse() {
        let rttm = parse_rttm(&fixture("diarize_5min.rttm"));
        let json = parse_diarize_json(&fixture("diarize_5min.json")).unwrap();
        assert_eq!(rttm.len(), 88);
        assert_eq!(rttm.len(), json.len());
        for (a, b) in rttm.iter().zip(&json) {
            assert_eq!(a.speaker, b.speaker);
            assert!((a.start_ms - b.start_ms).abs() <= 1);
            assert!((a.end_ms - b.end_ms).abs() <= 1);
        }
        assert_eq!(
            rttm[0],
            DiarSegment {
                start_ms: 251,
                end_ms: 1_079,
                speaker: 1
            }
        );
        let runner = DiarizeRunner::new("E:/e", "E:/m", "E:/m/d.gguf");
        let args: Vec<String> = runner
            .args(Path::new("a.wav"), Path::new("a.rttm"))
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.windows(2).any(|w| w == ["--format", "rttm"]));
        assert!(!args.iter().any(|a| a == "--offline"));
    }

    #[test]
    fn missing_files_become_error_events() {
        let dir = tempfile::tempdir().unwrap();
        let e = NemoCliEngine::new(
            dir.path(),
            dir.path(),
            dir.path().join("a.gguf"),
            dir.path().join("d.gguf"),
        );
        let rx = e
            .transcribe_file(FileJob {
                job_id: JobId("m".into()),
                wav_path: dir.path().join("x.wav"),
                diarize: true,
                language: "en".into(),
                diar_preset: DiarPreset::Streaming,
            })
            .unwrap();
        let ev = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            matches!(
                ev,
                EngineEvent::Error {
                    code: ErrorCode::ModelMissing,
                    ..
                }
            ),
            "{ev:?}"
        );
        e.cancel(JobId("unknown".into()));
    }
}
