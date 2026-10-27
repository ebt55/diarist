//! Audio retention (`docs/01`, "Recording storage"): a meeting's stored
//! audio is deleted `audio_retention_days` after it ended, unless the
//! meeting is pinned (`audio_keep`). Transcripts are never touched.
//!
//! What goes: the `playback.flac` and the WAVs under `audio\<uid>\` (the
//! registered tracks of format `flac` or `wav`, plus unregistered `*.wav`
//! and `*.flac` a crash left behind), their waveform caches, and the AGC
//! gain tracks beside them (`*.gain`, `crate::audio::gain_track`: kept as
//! long as the audio they describe). What stays:
//! the database (meetings, runs, words, speakers, edits), the engine JSON
//! and RTTM files in the meeting folder, and an import's original file,
//! which is the user's and lives outside the data root. Meetings still
//! recording or processing are skipped.
//!
//! Re-running the post-pass, re-diarization or a fusion experiment needs
//! the audio, so those only reach meetings whose audio is still there.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use ts_rs::TS;

use crate::error::Result;
use crate::settings;
use crate::store::{RunStore, Store};
use crate::types::{AudioFormat, MeetingId, MeetingStatus, TrackId};

const DAY_MS: i64 = 86_400_000;

/// One file retention would delete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct RetentionFile {
    /// Absolute path.
    pub path: String,
    pub bytes: u64,
    /// The `audio_tracks` row removed with it (`None`: a leftover file).
    pub track_id: Option<TrackId>,
}

/// One meeting whose audio is past retention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct RetentionMeeting {
    pub meeting_id: MeetingId,
    pub title: String,
    /// Unix ms the meeting ended (start plus duration).
    pub ended_at: i64,
    pub files: Vec<RetentionFile>,
}

/// What [`plan`] found (Settings shows the totals as reclaimable space).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct RetentionPlan {
    /// The setting the plan was made with (0: keep forever, nothing planned).
    pub days: u32,
    /// Audio of meetings that ended before this (unix ms) is deleted.
    pub cutoff_ms: i64,
    pub meetings: Vec<RetentionMeeting>,
    pub files: u32,
    pub bytes: u64,
    /// Pinned meetings that would otherwise be due (kept).
    pub pinned_skipped: u32,
}

/// What [`apply`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct RetentionReport {
    pub meetings: u32,
    pub files: u32,
    pub bytes: u64,
    pub errors: Vec<String>,
}

/// Audio retention deletes: the WAVs and FLACs, and the gain tracks.
fn is_audio_file(p: &Path) -> bool {
    matches!(
        p.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("wav" | "flac" | "gain")
    )
}

/// The audio of unpinned, finished meetings that ended more than `days`
/// ago, as of `now_ms`. `days == 0` keeps everything.
pub fn plan(store: &dyn Store, data_root: &Path, days: u32, now_ms: i64) -> Result<RetentionPlan> {
    let mut out = RetentionPlan {
        days,
        cutoff_ms: now_ms - i64::from(days) * DAY_MS,
        ..Default::default()
    };
    if days == 0 {
        out.cutoff_ms = 0;
        return Ok(out);
    }
    let audio_root = data_root.join("audio");
    for m in store.list_meetings()? {
        if matches!(
            m.status,
            MeetingStatus::Recording | MeetingStatus::Processing
        ) {
            continue;
        }
        let ended_at = m.started_at + m.duration_ms.unwrap_or(0);
        if ended_at > out.cutoff_ms {
            continue;
        }
        let folder = audio_root.join(&m.uid);
        let mut files = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        for t in store.list_audio_tracks(m.id)? {
            if t.format == AudioFormat::Original || Path::new(&t.path).is_absolute() {
                continue; // the user's own file, never ours to delete
            }
            let p = t
                .path
                .split('/')
                .fold(data_root.to_path_buf(), |acc, part| acc.join(part));
            if !p.starts_with(&audio_root) {
                continue;
            }
            seen.insert(p.clone());
            files.push(RetentionFile {
                bytes: std::fs::metadata(&p).map(|md| md.len()).unwrap_or(0),
                path: p.display().to_string(),
                track_id: Some(t.id),
            });
        }
        if let Ok(rd) = std::fs::read_dir(&folder) {
            let mut leftovers: Vec<PathBuf> = rd
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| e.path())
                .filter(|p| is_audio_file(p) && !seen.contains(p))
                .collect();
            leftovers.sort();
            for p in leftovers {
                files.push(RetentionFile {
                    bytes: std::fs::metadata(&p).map(|md| md.len()).unwrap_or(0),
                    path: p.display().to_string(),
                    track_id: None,
                });
            }
        }
        if files.is_empty() {
            continue;
        }
        if m.audio_keep {
            out.pinned_skipped += 1;
            continue;
        }
        out.files += files.len() as u32;
        out.bytes += files.iter().map(|f| f.bytes).sum::<u64>();
        out.meetings.push(RetentionMeeting {
            meeting_id: m.id,
            title: m.title,
            ended_at,
            files,
        });
    }
    Ok(out)
}

/// [`plan`] with the store's `audio_retention_days`.
pub fn plan_from_settings(
    store: &dyn Store,
    data_root: &Path,
    now_ms: i64,
) -> Result<RetentionPlan> {
    let s = settings::effective(store)?;
    let days =
        settings::get_u64(&s, settings::AUDIO_RETENTION_DAYS).min(u64::from(u32::MAX)) as u32;
    plan(store, data_root, days, now_ms)
}

/// Deletes the planned files and their track rows (a file that cannot be
/// deleted keeps its row). A meeting pinned since the plan was made is
/// skipped.
pub fn apply(store: &dyn RunStore, plan: &RetentionPlan) -> RetentionReport {
    let mut report = RetentionReport::default();
    let pinned: HashSet<MeetingId> = store
        .list_meetings()
        .map(|ms| {
            ms.into_iter()
                .filter(|m| m.audio_keep)
                .map(|m| m.id)
                .collect()
        })
        .unwrap_or_default();
    for m in &plan.meetings {
        if pinned.contains(&m.meeting_id) {
            continue;
        }
        let mut any = false;
        for f in &m.files {
            let p = Path::new(&f.path);
            let removed = match std::fs::remove_file(p) {
                Ok(()) => true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
                Err(e) => {
                    report.errors.push(format!("{}: {e}", p.display()));
                    false
                }
            };
            if !removed {
                continue;
            }
            let mut cache = p.as_os_str().to_owned();
            cache.push(".peaks.json");
            let _ = std::fs::remove_file(PathBuf::from(cache));
            if let Some(t) = f.track_id {
                if let Err(e) = store.delete_audio_track(t) {
                    report.errors.push(format!("track {t}: {e}"));
                }
            }
            report.files += 1;
            report.bytes += f.bytes;
            any = true;
        }
        if any {
            report.meetings += 1;
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NewAudioTrack, NewMeeting, SqliteStore};
    use crate::types::{Meeting, Source, TrackChannel};

    const NOW: i64 = 1_800_000_000_000;

    fn meeting(store: &SqliteStore, title: &str, days_ago: i64, status: MeetingStatus) -> Meeting {
        let m = store
            .create_meeting(NewMeeting {
                title: title.into(),
                source: Source::Live,
                source_path: None,
                language: "en".into(),
                diarize_mic: false,
                started_at: NOW - days_ago * DAY_MS - 3_600_000,
                status,
            })
            .unwrap();
        store
            .finish_meeting(m.id, NOW - days_ago * DAY_MS, 3_600_000)
            .unwrap();
        store.set_meeting_status(m.id, status).unwrap();
        m
    }

    fn track(
        store: &SqliteStore,
        root: &Path,
        m: &Meeting,
        name: &str,
        format: AudioFormat,
        bytes: usize,
    ) {
        let rel = format!("audio/{}/{name}", m.uid);
        let p = root.join("audio").join(&m.uid).join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, vec![1u8; bytes]).unwrap();
        store
            .add_audio_track(NewAudioTrack {
                meeting_id: m.id,
                channel: if format == AudioFormat::Flac {
                    TrackChannel::Playback
                } else {
                    TrackChannel::Mic
                },
                path: rel,
                format,
                sample_rate: 16_000,
                channels: if format == AudioFormat::Flac { 2 } else { 1 },
                duration_ms: Some(1_000),
                size_bytes: Some(bytes as i64),
                channel_frames: None,
            })
            .unwrap();
    }

    #[test]
    fn deletes_old_unpinned_audio_and_never_transcripts_or_originals() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let store = SqliteStore::open(root.join("diarist.db")).unwrap();

        let old = meeting(&store, "old", 40, MeetingStatus::Done);
        track(&store, root, &old, "playback.flac", AudioFormat::Flac, 900);
        std::fs::write(
            root.join("audio").join(&old.uid).join("loopback.wav"),
            [0u8; 100],
        )
        .unwrap();
        std::fs::write(root.join("audio").join(&old.uid).join("words.json"), b"{}").unwrap();
        std::fs::write(
            root.join("audio").join(&old.uid).join("loopback.gain"),
            [0u8; 16],
        )
        .unwrap();
        std::fs::write(
            root.join("audio")
                .join(&old.uid)
                .join("playback.flac.peaks.json"),
            b"{}",
        )
        .unwrap();

        let pinned = meeting(&store, "pinned", 40, MeetingStatus::Done);
        track(
            &store,
            root,
            &pinned,
            "playback.flac",
            AudioFormat::Flac,
            500,
        );
        store.set_audio_keep(pinned.id, true).unwrap();

        let recent = meeting(&store, "recent", 5, MeetingStatus::Done);
        track(
            &store,
            root,
            &recent,
            "playback.flac",
            AudioFormat::Flac,
            700,
        );

        let busy = meeting(&store, "busy", 40, MeetingStatus::Processing);
        track(&store, root, &busy, "mic.wav", AudioFormat::Wav, 300);

        // An import: its original lives outside the data root.
        let import = meeting(&store, "import", 40, MeetingStatus::Done);
        let original = dir.path().join("elsewhere.m4a");
        std::fs::write(&original, b"user file").unwrap();
        store
            .add_audio_track(NewAudioTrack {
                meeting_id: import.id,
                channel: TrackChannel::Playback,
                path: original.display().to_string(),
                format: AudioFormat::Original,
                sample_rate: 48_000,
                channels: 2,
                duration_ms: None,
                size_bytes: None,
                channel_frames: None,
            })
            .unwrap();

        let p = plan(&store, root, 30, NOW).unwrap();
        assert_eq!(p.meetings.len(), 1, "{p:?}");
        assert_eq!(p.meetings[0].title, "old");
        assert_eq!((p.files, p.bytes), (3, 1016));
        assert_eq!(p.pinned_skipped, 1);
        assert!(plan(&store, root, 0, NOW).unwrap().meetings.is_empty());

        let r = apply(&store, &p);
        assert_eq!((r.meetings, r.files, r.bytes), (1, 3, 1016), "{r:?}");
        let folder = root.join("audio").join(&old.uid);
        assert!(!folder.join("playback.flac").exists() && !folder.join("loopback.wav").exists());
        assert!(
            !folder.join("loopback.gain").exists(),
            "the gain track goes with the FLAC"
        );
        assert!(!folder.join("playback.flac.peaks.json").exists());
        assert!(folder.join("words.json").is_file(), "engine output stays");
        assert!(store.list_audio_tracks(old.id).unwrap().is_empty());
        assert!(store.get_meeting(old.id).is_ok(), "the meeting stays");
        assert!(original.is_file());
        assert!(root
            .join("audio")
            .join(&pinned.uid)
            .join("playback.flac")
            .is_file());
        assert!(root
            .join("audio")
            .join(&recent.uid)
            .join("playback.flac")
            .is_file());
        assert!(root.join("audio").join(&busy.uid).join("mic.wav").is_file());
        // Nothing left to do.
        assert!(plan(&store, root, 30, NOW).unwrap().meetings.is_empty());
    }
}
