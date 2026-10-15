//! Mock-only behaviour for Phase 2, written against the `Store` trait so it
//! keeps working when `SqliteStore` replaces `MemoryStore`. Each piece names
//! what replaces it:
//!
//! - [`run_import`]: `meet_core::pipeline::import_file` (Phase 1). It follows
//!   the same documented steps and events, with a timer instead of an engine.
//! - [`attach_demo_audio`]: real playback tracks written by import and capture.
//! - [`gpu`]: NVML in the `models` module.

use std::path::Path;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use meet_core::pipeline::{CancelToken, ImportRequest};
use meet_core::store::{
    NewAudioTrack, NewMeeting, NewSegment, NewSpeaker, NewTranscript, NewWord, Store,
};
use meet_core::types::{
    AudioFormat, Channel, MeetingId, MeetingStatus, RunKind, RunStatus, Source, TrackChannel,
};
use meet_core::views::{DeviceInfo, JobEvent, JobStage};
use tauri::AppHandle;

use crate::audio;
use crate::events::emit_job;

/// Relative to the data root, like every stored track.
pub const DEMO_TRACK: &str = "audio/demo/playback.wav";
const DEMO_SECONDS: u32 = 60;

/// Mock GPU: an RTX 3080 with a browser and a meeting app already on it.
pub fn gpu() -> (Option<u32>, Option<DeviceInfo>) {
    let free = 4_600;
    (
        Some(free),
        Some(DeviceInfo {
            name: "NVIDIA GeForce RTX 3080 (mock)".into(),
            vram_total_mb: Some(10_240),
            vram_free_mb: Some(free),
            driver_version: Some("610.88".into()),
        }),
    )
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Registers the demo WAV as the playback track of the seeded meetings that
/// have 2,000 segments, and writes the file once (in the background) if it
/// is not there yet.
pub fn attach_demo_audio(store: &dyn Store, data_root: &Path) {
    let file = data_root.join(DEMO_TRACK);
    let size = 44 + i64::from(DEMO_SECONDS) * 16_000 * 4;
    let large: Vec<MeetingId> = store
        .list_meetings()
        .unwrap_or_default()
        .into_iter()
        .filter(|m| {
            store
                .get_segments(m.id, 1_999, 1)
                .map(|s| !s.is_empty())
                .unwrap_or(false)
        })
        .map(|m| m.id)
        .collect();
    for meeting_id in &large {
        let track = NewAudioTrack {
            meeting_id: *meeting_id,
            channel: TrackChannel::Playback,
            path: DEMO_TRACK.into(),
            format: AudioFormat::Wav,
            sample_rate: 16_000,
            channels: 2,
            duration_ms: Some(i64::from(DEMO_SECONDS) * 1000),
            size_bytes: Some(size),
        };
        if let Err(e) = store.add_audio_track(track) {
            log::warn!("demo audio for meeting {meeting_id}: {e}");
        }
    }
    let fresh = std::fs::metadata(&file)
        .map(|m| m.len() == size as u64)
        .unwrap_or(false);
    if !fresh {
        thread::spawn(move || match audio::write_demo_wav(&file, DEMO_SECONDS) {
            Ok(_) => log::info!("wrote demo playback track {}", file.display()),
            Err(e) => log::error!("demo playback track {}: {e}", file.display()),
        });
    }
    log::info!("demo audio attached to {} meetings", large.len());
}

const SUPPORTED: &[&str] = &[
    "wav", "mp3", "m4a", "aac", "flac", "ogg", "opus", "wma", "mp4", "m4v", "mkv", "mov", "webm",
    "mka",
];
/// Message of the `job:error` event for a cancelled import (the UI shows it as info).
pub const CANCELLED: &str = "Import cancelled";
const TICKS: u64 = 20;
const TICK: Duration = Duration::from_millis(250);

struct Failure {
    meeting_id: Option<MeetingId>,
    message: String,
}

/// Mock of `pipeline::import_file`: creates the meeting (processing), reports
/// progress every 250 ms for about 5 s, then writes a generated transcript
/// and emits `job:done`. Honours cancellation; unsupported file types fail
/// during "decoding" like the real decoder chain would.
pub fn run_import(store: &dyn Store, app: &AppHandle, req: ImportRequest, token: &CancelToken) {
    let job_id = req.job_id.clone();
    let emit = |ev: JobEvent| emit_job(app, &ev);
    match import(store, &req, token, &emit) {
        Ok(meeting_id) => {
            log::info!(
                "import done: {} -> meeting {meeting_id}",
                req.path.display()
            );
            emit(JobEvent::Done { job_id, meeting_id });
        }
        Err(Failure {
            meeting_id,
            message,
        }) => {
            if meeting_id.is_none() && message == CANCELLED {
                log::info!("import cancelled: {}", req.path.display());
            } else {
                log::error!("import failed: {}: {message}", req.path.display());
            }
            emit(JobEvent::Error {
                job_id,
                meeting_id,
                message,
            });
        }
    }
}

fn import(
    store: &dyn Store,
    req: &ImportRequest,
    token: &CancelToken,
    emit: &dyn Fn(JobEvent),
) -> Result<MeetingId, Failure> {
    let fail = |meeting_id: Option<MeetingId>| {
        move |e: meet_core::store::StoreError| Failure {
            meeting_id,
            message: e.to_string(),
        }
    };
    let path = req.path.as_path();
    let meta = std::fs::metadata(path)
        .ok()
        .filter(|m| m.is_file())
        .ok_or_else(|| Failure {
            meeting_id: None,
            message: format!("File not found: {}", path.display()),
        })?;
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let total_ms = audio::wav_info(path)
        .map(|i| i.duration_ms())
        .unwrap_or_else(|_| {
            // About 128 kbit/s for compressed formats.
            (meta.len() / 16) as i64
        })
        .clamp(30_000, 3 * 3_600_000);
    let title = req.title.clone().unwrap_or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Imported meeting".into())
    });
    let started_at = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64 - total_ms)
        .unwrap_or_else(now_ms);

    let meeting = store
        .create_meeting(NewMeeting {
            title,
            source: Source::File,
            source_path: Some(path.display().to_string()),
            language: "en".into(),
            diarize_mic: false,
            started_at,
            status: MeetingStatus::Processing,
        })
        .map_err(fail(None))?;
    let id = meeting.id;
    store
        .add_audio_track(NewAudioTrack {
            meeting_id: id,
            channel: TrackChannel::Playback,
            path: path.display().to_string(),
            format: AudioFormat::Original,
            sample_rate: 48_000,
            channels: 2,
            duration_ms: Some(total_ms),
            size_bytes: Some(meta.len() as i64),
        })
        .map_err(fail(Some(id)))?;

    for tick in 0..TICKS {
        if token.is_cancelled() {
            let _ = store.delete_meeting(id);
            return Err(Failure {
                meeting_id: None,
                message: CANCELLED.into(),
            });
        }
        let stage = match tick {
            0..=3 => JobStage::Decoding,
            4..=13 => JobStage::Transcribing,
            14..=17 => JobStage::Diarizing,
            _ => JobStage::Saving,
        };
        emit(JobEvent::Progress {
            job_id: req.job_id.clone(),
            stage,
            done_ms: total_ms as u64 * tick / TICKS,
            total_ms: Some(total_ms as u64),
        });
        if tick == 3 && !SUPPORTED.contains(&ext.as_str()) {
            let _ = store.set_meeting_status(id, MeetingStatus::Failed);
            return Err(Failure {
                meeting_id: Some(id),
                message: format!(
                    "Cannot decode {}: unsupported file type \".{ext}\" (symphonia, Media Foundation and ffmpeg all declined)",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
            });
        }
        thread::sleep(TICK);
    }

    let saved = (|| {
        let run = store.create_run(
            id,
            RunKind::File,
            serde_json::json!({
                "runtime": "nemo-speech 0.2.0 (mock)",
                "asr": "nemotron-3.5-asr-streaming-0.6b.q8_0.gguf",
                "diarizer": "Nemotron-3-Diarization.q8_0.gguf",
                "mock": true
            }),
        )?;
        let seed = path
            .to_string_lossy()
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
            });
        store.insert_transcript(run.id, &mock_transcript(total_ms, seed))?;
        store.finish_run(run.id, RunStatus::Done)?;
        store.set_current_run(id, run.id)?;
        store.finish_meeting(id, started_at + total_ms, total_ms)?;
        store.set_meeting_status(id, MeetingStatus::Done)
    })();
    if let Err(e) = saved {
        let _ = store.set_meeting_status(id, MeetingStatus::Failed);
        return Err(fail(Some(id))(e));
    }
    emit(JobEvent::Progress {
        job_id: req.job_id.clone(),
        stage: JobStage::Saving,
        done_ms: total_ms as u64,
        total_ms: Some(total_ms as u64),
    });
    Ok(id)
}

const WORDS: &[&str] = &[
    "we",
    "should",
    "move",
    "the",
    "launch",
    "to",
    "next",
    "sprint",
    "because",
    "search",
    "latency",
    "is",
    "still",
    "high",
    "for",
    "large",
    "accounts",
    "I",
    "think",
    "customers",
    "asked",
    "about",
    "export",
    "and",
    "speaker",
    "names",
    "so",
    "let's",
    "review",
    "numbers",
    "on",
    "Friday",
    "okay",
    "that",
    "works",
    "budget",
    "plan",
    "risk",
    "owner",
    "follow",
    "up",
    "with",
    "design",
    "team",
    "agreed",
    "yeah",
    "right",
    "data",
    "model",
    "accuracy",
];

/// A plausible transcript covering `total_ms`: 2 to 4 speakers, words of
/// 250 to 450 ms, short pauses between turns.
fn mock_transcript(total_ms: i64, seed: u64) -> NewTranscript {
    let mut state = seed | 1;
    let mut rand = move |n: u64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state % n.max(1)
    };
    let n_speakers = 2 + rand(3) as usize;
    let speakers = (0..n_speakers)
        .map(|k| NewSpeaker {
            channel: Channel::Source,
            engine_label: format!("spk_{k}"),
            display_name: None,
            color: None,
            needs_review: false,
        })
        .collect();
    let mut segments = Vec::new();
    let mut words = Vec::new();
    let mut t = 400i64;
    let mut speaker = 0usize;
    while t < total_ms - 2_000 && segments.len() < 1_500 {
        if rand(100) < 60 {
            speaker = rand(n_speakers as u64) as usize;
        }
        let room = ((total_ms - t) / 450).max(1) as usize;
        let n = (4 + rand(20) as usize).min(room);
        let seg_index = segments.len();
        let start = t;
        let mut text = Vec::with_capacity(n);
        for i in 0..n {
            let mut w = WORDS[rand(WORDS.len() as u64) as usize].to_owned();
            if i == 0 {
                w = w[..1].to_uppercase() + &w[1..];
            }
            if i == n - 1 {
                w.push(if rand(100) < 15 { '?' } else { '.' });
            }
            let len = 250 + rand(200) as i64;
            words.push(NewWord {
                segment: seg_index,
                start_ms: t,
                end_ms: t + len - 30,
                text: w.clone(),
                confidence: None,
                raw_label: Some(format!("spk_{speaker}")),
                margin: None,
                overlap: false,
            });
            text.push(w);
            t += len;
        }
        segments.push(NewSegment {
            speaker,
            start_ms: start,
            end_ms: t,
            text: text.join(" "),
        });
        t += 200 + rand(1_000) as i64;
    }
    NewTranscript {
        speakers,
        segments,
        words,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_transcript_is_consistent() {
        let tr = mock_transcript(120_000, 42);
        assert!(tr.speakers.len() >= 2 && tr.speakers.len() <= 4);
        assert!(!tr.segments.is_empty());
        for (i, seg) in tr.segments.iter().enumerate() {
            let ws: Vec<_> = tr.words.iter().filter(|w| w.segment == i).collect();
            let joined: Vec<&str> = ws.iter().map(|w| w.text.as_str()).collect();
            assert_eq!(joined.join(" "), seg.text);
            assert!(seg.speaker < tr.speakers.len());
            assert!(seg.end_ms <= 120_000);
        }
    }
}
