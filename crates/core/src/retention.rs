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
//!
//! ## Warnings before deletion (0.1.0-beta.7, issue 22)
//!
//! `audio_retention_days` defaults to 0 (keep forever). With a limit set,
//! nothing is deleted automatically ([`run_auto`]) unless the user could
//! have seen it coming:
//!
//! - **First deletion ever:** held until the user answers the hold dialog.
//!   "Delete as set" sets [`RetentionState::first_ack`] and deletes the
//!   meetings it listed. "Keep everything" sets the days to 0 (the UI
//!   writes the setting). "Review and pin" never acknowledges.
//! - **After that:** a meeting is deleted automatically only when the
//!   library heads-up listed it while the window was in front, inside the
//!   [`HEADS_UP_DAYS`] before its due time
//!   (`due - HEADS_UP_DAYS <= warned_at < due`, [`RetentionState::warned`]).
//!   Everything else that is due (the app was closed during the window, the
//!   limit was lowered, a beta.6 upgrade) is held and asked about.
//! - **Approved:** "Free up now" and "Delete now" ([`RetentionAction::Delete`])
//!   delete the meetings the user saw listed, without the gate.
//!
//! The bookkeeping is one JSON value in the settings table
//! ([`settings::RETENTION_STATE`]), so the schema does not change.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::Result;
use crate::settings;
use crate::store::{RunStore, Store};
use crate::types::{AudioFormat, MeetingId, MeetingStatus, TrackId};

const DAY_MS: i64 = 86_400_000;

/// The library heads-up lists a meeting this many days before its audio
/// is deleted.
pub const HEADS_UP_DAYS: u32 = 3;

/// Recordings past this many bytes (10 GB, decimal) bring the one-time
/// storage prompt. It never deletes anything.
pub const STORAGE_PROMPT_BYTES: u64 = 10_000_000_000;

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

/// The store's `audio_retention_days` (0 when unset: keep forever).
pub fn days_setting(store: &dyn Store) -> Result<u32> {
    let s = settings::effective(store)?;
    Ok(settings::get_u64(&s, settings::AUDIO_RETENTION_DAYS).min(u64::from(u32::MAX)) as u32)
}

/// [`plan`] with the store's `audio_retention_days`.
pub fn plan_from_settings(
    store: &dyn Store,
    data_root: &Path,
    now_ms: i64,
) -> Result<RetentionPlan> {
    plan(store, data_root, days_setting(store)?, now_ms)
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

// ---- Warnings before deletion -----------------------------------------------------------

/// Retention bookkeeping, stored as JSON under [`settings::RETENTION_STATE`].
/// Missing or unreadable reads as the default: nothing answered, nobody
/// warned, which only ever holds more.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionState {
    /// The user chose "Delete as set" at the first-deletion hold.
    #[serde(default)]
    pub first_ack: bool,
    /// Meeting id to the unix ms the library heads-up listing it was first
    /// on screen with the window in front. Valid for a due time only when
    /// inside its heads-up window ([`warned_in_time`]); pruned to the
    /// meetings due within [`HEADS_UP_DAYS`].
    #[serde(default)]
    pub warned: BTreeMap<MeetingId, i64>,
    /// The one-time storage prompt was on screen.
    #[serde(default)]
    pub storage_prompted: bool,
}

/// Serialises read-modify-write of [`RetentionState`] (the daily job and
/// the commands run on different threads).
static STATE_LOCK: Mutex<()> = Mutex::new(());

fn state_lock() -> std::sync::MutexGuard<'static, ()> {
    STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

impl RetentionState {
    /// The stored state (the default when missing or unreadable).
    pub fn load(store: &dyn Store) -> Result<Self> {
        Ok(store
            .get_settings()?
            .0
            .get(settings::RETENTION_STATE)
            .and_then(|v| serde_json::from_str(v).ok())
            .unwrap_or_default())
    }

    fn save(&self, store: &dyn Store) -> Result<()> {
        store.set_setting(settings::RETENTION_STATE, &serde_json::to_string(self)?)?;
        Ok(())
    }
}

/// When a meeting reaches the limit (unix ms).
fn due_at(ended_at: i64, days: u32) -> i64 {
    ended_at + i64::from(days) * DAY_MS
}

/// A heads-up seen at `warned_at` covers a deletion due at `due`: it was
/// on screen inside the heads-up window, before the due time. A warning
/// for another limit (the days changed since) does not count.
pub fn warned_in_time(warned_at: i64, due: i64) -> bool {
    warned_at >= due - i64::from(HEADS_UP_DAYS) * DAY_MS && warned_at < due
}

/// What the automatic job may do with the meetings that are due.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Decision {
    /// Due and warned in time, after the first deletion was acknowledged.
    pub delete: RetentionPlan,
    /// Due but not covered: kept until the user decides.
    pub held: Vec<RetentionMeeting>,
}

/// Splits `due` (a [`plan`] as of now) into what may go and what is held.
/// Before the first deletion was acknowledged everything is held.
pub fn decide(due: &RetentionPlan, state: &RetentionState) -> Decision {
    let mut out = Decision {
        delete: RetentionPlan {
            days: due.days,
            cutoff_ms: due.cutoff_ms,
            pinned_skipped: due.pinned_skipped,
            ..Default::default()
        },
        held: Vec::new(),
    };
    for m in &due.meetings {
        let ok = state.first_ack
            && state
                .warned
                .get(&m.meeting_id)
                .is_some_and(|&w| warned_in_time(w, due_at(m.ended_at, due.days)));
        if ok {
            out.delete.files += m.files.len() as u32;
            out.delete.bytes += m.files.iter().map(|f| f.bytes).sum::<u64>();
            out.delete.meetings.push(m.clone());
        } else {
            out.held.push(m.clone());
        }
    }
    out
}

/// One meeting in a retention notice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct RetentionNotice {
    pub meeting_id: MeetingId,
    pub title: String,
    /// Unix ms the meeting ended.
    pub ended_at: i64,
    /// Unix ms the limit is reached (the audio goes at the next check after
    /// this, if the heads-up was seen in time).
    pub due_at: i64,
    pub bytes: u64,
}

impl RetentionNotice {
    fn of(m: &RetentionMeeting, days: u32) -> Self {
        Self {
            meeting_id: m.meeting_id,
            title: m.title.clone(),
            ended_at: m.ended_at,
            due_at: due_at(m.ended_at, days),
            bytes: m.files.iter().map(|f| f.bytes).sum(),
        }
    }
}

/// What Settings and the library notices show ([`status`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct RetentionStatus {
    /// `audio_retention_days` (0: keep forever).
    pub days: u32,
    /// [`HEADS_UP_DAYS`].
    pub heads_up_days: u32,
    /// Every file under `audio\` (pinned meetings included; an import's
    /// original lives elsewhere and is not counted).
    pub recordings_bytes: u64,
    /// Everything past the limit now: what "Free up now" deletes.
    pub due: RetentionPlan,
    /// The first automatic deletion waits for the user (`due` lists it).
    pub first_hold: bool,
    /// Past the limit but not warned in time (after the first deletion was
    /// acknowledged): kept until the user decides.
    pub held: Vec<RetentionNotice>,
    /// The heads-up: audio that goes within [`HEADS_UP_DAYS`] (after the
    /// first deletion was acknowledged), soonest first.
    pub upcoming: Vec<RetentionNotice>,
    /// Recordings passed [`STORAGE_PROMPT_BYTES`] and the prompt was never
    /// shown.
    pub storage_prompt: bool,
}

/// Bytes of every file under `<data_root>\audio`.
pub fn recordings_bytes(data_root: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![data_root.join("audio")];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(t) if t.is_file() => total += e.metadata().map(|m| m.len()).unwrap_or(0),
                _ => {}
            }
        }
    }
    total
}

/// The storage prompt shows once, when recordings pass
/// [`STORAGE_PROMPT_BYTES`].
pub fn storage_prompt_due(recordings_bytes: u64, state: &RetentionState) -> bool {
    !state.storage_prompted && recordings_bytes >= STORAGE_PROMPT_BYTES
}

/// The heads-up window's plan: everything due by `now + HEADS_UP_DAYS`.
fn plan_soon(store: &dyn Store, data_root: &Path, days: u32, now_ms: i64) -> Result<RetentionPlan> {
    plan(
        store,
        data_root,
        days,
        now_ms + i64::from(HEADS_UP_DAYS) * DAY_MS,
    )
}

/// What retention would do and is waiting for, as of `now_ms`. Read only.
pub fn status(store: &dyn Store, data_root: &Path, now_ms: i64) -> Result<RetentionStatus> {
    let days = days_setting(store)?;
    let state = RetentionState::load(store)?;
    let recordings_bytes = recordings_bytes(data_root);
    let mut out = RetentionStatus {
        days,
        heads_up_days: HEADS_UP_DAYS,
        recordings_bytes,
        storage_prompt: storage_prompt_due(recordings_bytes, &state),
        ..Default::default()
    };
    out.due = plan(store, data_root, days, now_ms)?;
    if days == 0 {
        return Ok(out);
    }
    if !state.first_ack {
        out.first_hold = !out.due.meetings.is_empty();
        return Ok(out);
    }
    let d = decide(&out.due, &state);
    let held: HashSet<MeetingId> = d.held.iter().map(|m| m.meeting_id).collect();
    out.held = d
        .held
        .iter()
        .map(|m| RetentionNotice::of(m, days))
        .collect();
    out.upcoming = plan_soon(store, data_root, days, now_ms)?
        .meetings
        .iter()
        .filter(|m| !held.contains(&m.meeting_id))
        .map(|m| RetentionNotice::of(m, days))
        .collect();
    out.upcoming.sort_by_key(|n| (n.due_at, n.meeting_id));
    Ok(out)
}

/// What the automatic job did ([`run_auto`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutoOutcome {
    pub days: u32,
    pub report: RetentionReport,
    /// Due meetings kept because nobody was warned (or the first deletion
    /// was not acknowledged).
    pub held: u32,
    /// The first deletion is waiting for the user.
    pub first_hold: bool,
    pub pinned_skipped: u32,
}

/// The launch and daily job: prunes the warnings, then deletes only what
/// [`decide`] allows. Never deletes before the first acknowledgement.
pub fn run_auto(store: &dyn RunStore, data_root: &Path, now_ms: i64) -> Result<AutoOutcome> {
    let _g = state_lock();
    let days = days_setting(store)?;
    let mut state = RetentionState::load(store)?;
    let mut out = AutoOutcome {
        days,
        ..Default::default()
    };
    // Prune: only meetings in the heads-up window keep their warning.
    let soon: HashSet<MeetingId> = if days == 0 {
        HashSet::new()
    } else {
        plan_soon(store, data_root, days, now_ms)?
            .meetings
            .iter()
            .map(|m| m.meeting_id)
            .collect()
    };
    let before = state.warned.len();
    state.warned.retain(|id, _| soon.contains(id));
    if state.warned.len() != before {
        state.save(store)?;
    }
    if days == 0 {
        return Ok(out);
    }
    let due = plan(store, data_root, days, now_ms)?;
    out.pinned_skipped = due.pinned_skipped;
    let d = decide(&due, &state);
    out.held = d.held.len() as u32;
    out.first_hold = !state.first_ack && !due.meetings.is_empty();
    if !d.delete.meetings.is_empty() {
        out.report = apply(store, &d.delete);
    }
    Ok(out)
}

/// What the user did in a retention notice or in Settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum RetentionAction {
    /// "Free up now", or "Delete now" on held meetings: deletes the audio
    /// of these meetings (the ones the user saw listed) if still due. An
    /// explicit approval, no warning needed.
    Delete { ids: Vec<MeetingId> },
    /// "Delete as set" at the first-deletion hold: remembers the answer,
    /// then deletes these meetings' audio as [`RetentionAction::Delete`].
    DeleteAsSet { ids: Vec<MeetingId> },
    /// The heads-up listing these meetings was on screen with the window
    /// in front.
    Seen { ids: Vec<MeetingId> },
    /// The storage prompt was on screen.
    StoragePromptSeen,
}

/// Carries out `action` as of `now_ms` (empty report for the bookkeeping
/// ones).
pub fn act(
    store: &dyn RunStore,
    data_root: &Path,
    action: &RetentionAction,
    now_ms: i64,
) -> Result<RetentionReport> {
    match action {
        RetentionAction::Delete { ids } => delete_approved(store, data_root, ids, now_ms),
        RetentionAction::DeleteAsSet { ids } => {
            {
                let _g = state_lock();
                let mut state = RetentionState::load(store)?;
                if !state.first_ack {
                    state.first_ack = true;
                    state.save(store)?;
                }
            }
            delete_approved(store, data_root, ids, now_ms)
        }
        RetentionAction::Seen { ids } => {
            mark_seen(store, data_root, ids, now_ms)?;
            Ok(RetentionReport::default())
        }
        RetentionAction::StoragePromptSeen => {
            let _g = state_lock();
            let mut state = RetentionState::load(store)?;
            if !state.storage_prompted {
                state.storage_prompted = true;
                state.save(store)?;
            }
            Ok(RetentionReport::default())
        }
    }
}

/// Deletes the audio of `ids` that is past the limit now (pinned meetings
/// and meetings no longer due are left alone).
fn delete_approved(
    store: &dyn RunStore,
    data_root: &Path,
    ids: &[MeetingId],
    now_ms: i64,
) -> Result<RetentionReport> {
    let wanted: HashSet<MeetingId> = ids.iter().copied().collect();
    let mut p = plan_from_settings(store, data_root, now_ms)?;
    p.meetings.retain(|m| wanted.contains(&m.meeting_id));
    if p.meetings.is_empty() {
        return Ok(RetentionReport::default());
    }
    Ok(apply(store, &p))
}

/// Records that the heads-up listed `ids`: each meeting in the heads-up
/// window, not yet due, whose warning is missing or was for another due
/// time, is marked warned now.
fn mark_seen(store: &dyn Store, data_root: &Path, ids: &[MeetingId], now_ms: i64) -> Result<()> {
    let _g = state_lock();
    let days = days_setting(store)?;
    if days == 0 || ids.is_empty() {
        return Ok(());
    }
    let wanted: HashSet<MeetingId> = ids.iter().copied().collect();
    let mut state = RetentionState::load(store)?;
    let mut changed = false;
    for m in plan_soon(store, data_root, days, now_ms)?.meetings {
        let due = due_at(m.ended_at, days);
        if !wanted.contains(&m.meeting_id) || due <= now_ms {
            continue;
        }
        let covered = state
            .warned
            .get(&m.meeting_id)
            .is_some_and(|&w| warned_in_time(w, due));
        if !covered {
            state.warned.insert(m.meeting_id, now_ms);
            changed = true;
        }
    }
    if changed {
        state.save(store)?;
    }
    Ok(())
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

    // ---- warnings before deletion ----

    const HOUR: i64 = 3_600_000;

    /// A finished meeting with a 1000-byte FLAC that ended `days_ago` days
    /// before [`NOW`].
    fn with_audio(store: &SqliteStore, root: &Path, title: &str, days_ago: i64) -> Meeting {
        let m = meeting(store, title, days_ago, MeetingStatus::Done);
        track(store, root, &m, "playback.flac", AudioFormat::Flac, 1000);
        m
    }

    fn has_audio(root: &Path, m: &Meeting) -> bool {
        root.join("audio")
            .join(&m.uid)
            .join("playback.flac")
            .is_file()
    }

    fn ids(list: &[RetentionNotice]) -> Vec<MeetingId> {
        list.iter().map(|n| n.meeting_id).collect()
    }

    fn ack(store: &SqliteStore) {
        let mut s = RetentionState::load(store).unwrap();
        s.first_ack = true;
        s.save(store).unwrap();
    }

    #[test]
    fn unset_days_keep_everything() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let store = SqliteStore::open(root.join("diarist.db")).unwrap();
        let old = with_audio(&store, root, "old", 400);
        let out = run_auto(&store, root, NOW).unwrap();
        assert_eq!((out.days, out.report.files, out.held), (0, 0, 0));
        assert!(has_audio(root, &old));
        let st = status(&store, root, NOW).unwrap();
        assert_eq!(st.days, 0);
        assert!(st.due.meetings.is_empty() && !st.first_hold && st.upcoming.is_empty());
        assert_eq!(st.recordings_bytes, 1000);
    }

    /// A beta.6 user who set 30 days and whose audio is already overdue:
    /// nothing goes until the first-deletion hold is answered, even with
    /// a heads-up seen.
    #[test]
    fn first_deletion_is_held_until_acknowledged() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let store = SqliteStore::open(root.join("diarist.db")).unwrap();
        store
            .set_setting(settings::AUDIO_RETENTION_DAYS, "30")
            .unwrap();
        let old = with_audio(&store, root, "old", 40);
        let older = with_audio(&store, root, "older", 50);
        let soon = with_audio(&store, root, "soon", 28);

        act(
            &store,
            root,
            &RetentionAction::Seen { ids: vec![soon.id] },
            NOW,
        )
        .unwrap();
        for day in 0..5 {
            let out = run_auto(&store, root, NOW + day * DAY_MS).unwrap();
            assert_eq!(out.report.files, 0, "day {day}: {out:?}");
            assert!(out.first_hold);
        }
        assert!(has_audio(root, &old) && has_audio(root, &older) && has_audio(root, &soon));

        let st = status(&store, root, NOW).unwrap();
        assert!(st.first_hold);
        assert_eq!(st.due.meetings.len(), 2);
        assert!(
            st.held.is_empty() && st.upcoming.is_empty(),
            "the hold dialog lists them"
        );
        assert_eq!(st.due.bytes, 2000);
        assert_eq!(
            store
                .get_settings()
                .unwrap()
                .0
                .get(settings::AUDIO_RETENTION_DAYS)
                .map(String::as_str),
            Some("30"),
            "the stored limit is never rewritten"
        );
    }

    /// "Delete as set" deletes what the hold listed (not what became due
    /// since) and is remembered across a restart (a new connection).
    #[test]
    fn delete_as_set_deletes_the_listed_meetings_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let db = root.join("diarist.db");
        let store = SqliteStore::open(&db).unwrap();
        store
            .set_setting(settings::AUDIO_RETENTION_DAYS, "30")
            .unwrap();
        let listed = with_audio(&store, root, "listed", 40);
        let unlisted = with_audio(&store, root, "unlisted", 45);
        let pinned = with_audio(&store, root, "pinned", 40);
        store.set_audio_keep(pinned.id, true).unwrap();

        let r = act(
            &store,
            root,
            &RetentionAction::DeleteAsSet {
                ids: vec![listed.id, pinned.id],
            },
            NOW,
        )
        .unwrap();
        assert_eq!((r.meetings, r.files), (1, 1), "{r:?}");
        assert!(!has_audio(root, &listed));
        assert!(has_audio(root, &unlisted) && has_audio(root, &pinned));
        drop(store);

        let store = SqliteStore::open(&db).unwrap();
        assert!(RetentionState::load(&store).unwrap().first_ack);
        let st = status(&store, root, NOW).unwrap();
        assert!(!st.first_hold);
        // The unlisted one was never warned: held and asked about.
        assert_eq!(ids(&st.held), vec![unlisted.id]);
        assert_eq!(run_auto(&store, root, NOW).unwrap().report.files, 0);
        assert!(has_audio(root, &unlisted));
    }

    /// After the acknowledgement, audio goes only when the heads-up listed
    /// it in its window before the due time.
    #[test]
    fn heads_up_gates_each_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let store = SqliteStore::open(root.join("diarist.db")).unwrap();
        store
            .set_setting(settings::AUDIO_RETENTION_DAYS, "30")
            .unwrap();
        ack(&store);
        let seen = with_audio(&store, root, "seen", 28); // due in 2 days
        let unseen = with_audio(&store, root, "unseen", 28);
        let later = with_audio(&store, root, "later", 20); // due in 10 days

        let st = status(&store, root, NOW).unwrap();
        assert_eq!(ids(&st.upcoming), vec![seen.id, unseen.id]);
        assert_eq!(st.upcoming[0].due_at, NOW + 2 * DAY_MS);
        assert!(st.held.is_empty());

        // The UI reports what was on screen; "later" is outside the window.
        act(
            &store,
            root,
            &RetentionAction::Seen {
                ids: vec![seen.id, later.id],
            },
            NOW,
        )
        .unwrap();
        let warned = RetentionState::load(&store).unwrap().warned;
        assert_eq!(warned.keys().copied().collect::<Vec<_>>(), vec![seen.id]);
        // Seen again later: the first sighting stays.
        act(
            &store,
            root,
            &RetentionAction::Seen { ids: vec![seen.id] },
            NOW + HOUR,
        )
        .unwrap();
        assert_eq!(RetentionState::load(&store).unwrap().warned[&seen.id], NOW);

        // Not yet due: nothing goes.
        assert_eq!(
            run_auto(&store, root, NOW + DAY_MS).unwrap().report.files,
            0
        );
        // Due: the warned one goes, the other is held.
        let out = run_auto(&store, root, NOW + 2 * DAY_MS + HOUR).unwrap();
        assert_eq!((out.report.meetings, out.held), (1, 1), "{out:?}");
        assert!(!has_audio(root, &seen) && has_audio(root, &unseen) && has_audio(root, &later));

        // A warning for another limit does not count: the days went up, so
        // the old sighting is outside the new window and is pruned.
        store
            .set_setting(settings::AUDIO_RETENTION_DAYS, "60")
            .unwrap();
        let mut s = RetentionState::load(&store).unwrap();
        s.warned.insert(later.id, NOW);
        s.save(&store).unwrap();
        assert!(!warned_in_time(NOW, due_at(NOW - 20 * DAY_MS, 60)));
        run_auto(&store, root, NOW + 3 * DAY_MS).unwrap();
        assert!(RetentionState::load(&store).unwrap().warned.is_empty());
    }

    /// The app was closed through the heads-up window: what is overdue at
    /// launch is held and asked about, and a sighting after the due time
    /// does not release it. "Delete now" does.
    #[test]
    fn overdue_at_launch_is_held_not_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let store = SqliteStore::open(root.join("diarist.db")).unwrap();
        store
            .set_setting(settings::AUDIO_RETENTION_DAYS, "7")
            .unwrap();
        ack(&store);
        let overdue = with_audio(&store, root, "overdue", 9);

        let out = run_auto(&store, root, NOW).unwrap();
        assert_eq!((out.report.files, out.held), (0, 1));
        let st = status(&store, root, NOW).unwrap();
        assert_eq!(ids(&st.held), vec![overdue.id]);
        assert!(st.upcoming.is_empty());

        act(
            &store,
            root,
            &RetentionAction::Seen {
                ids: vec![overdue.id],
            },
            NOW,
        )
        .unwrap();
        assert!(RetentionState::load(&store).unwrap().warned.is_empty());
        for day in 1..10 {
            assert_eq!(
                run_auto(&store, root, NOW + day * DAY_MS)
                    .unwrap()
                    .report
                    .files,
                0
            );
        }
        assert!(has_audio(root, &overdue));

        let r = act(
            &store,
            root,
            &RetentionAction::Delete {
                ids: vec![overdue.id],
            },
            NOW,
        )
        .unwrap();
        assert_eq!(r.meetings, 1);
        assert!(!has_audio(root, &overdue));
    }

    /// "Free up now" is an explicit approval: it deletes what it listed,
    /// with no acknowledgement and no warning.
    #[test]
    fn free_up_now_still_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let store = SqliteStore::open(root.join("diarist.db")).unwrap();
        store
            .set_setting(settings::AUDIO_RETENTION_DAYS, "30")
            .unwrap();
        let a = with_audio(&store, root, "a", 40);
        let b = with_audio(&store, root, "b", 41);
        let recent = with_audio(&store, root, "recent", 3);
        let listed: Vec<MeetingId> = status(&store, root, NOW)
            .unwrap()
            .due
            .meetings
            .iter()
            .map(|m| m.meeting_id)
            .collect();
        assert_eq!(listed.len(), 2);
        let r = act(
            &store,
            root,
            &RetentionAction::Delete {
                ids: [listed, vec![recent.id]].concat(),
            },
            NOW,
        )
        .unwrap();
        assert_eq!((r.meetings, r.files, r.bytes), (2, 2, 2000), "{r:?}");
        assert!(!has_audio(root, &a) && !has_audio(root, &b));
        assert!(has_audio(root, &recent), "not due, so not deleted");
        assert!(
            !RetentionState::load(&store).unwrap().first_ack,
            "an approval, not the acknowledgement"
        );
    }

    #[test]
    fn state_reads_garbage_as_nothing_answered_and_storage_prompt_is_once() {
        let store = crate::store::MemoryStore::new();
        store
            .set_setting(settings::RETENTION_STATE, "{not json")
            .unwrap();
        assert_eq!(
            RetentionState::load(&store).unwrap(),
            RetentionState::default()
        );
        let mut s = RetentionState::default();
        s.warned.insert(MeetingId(7), NOW);
        s.save(&store).unwrap();
        assert_eq!(RetentionState::load(&store).unwrap(), s, "round trip");

        assert!(!storage_prompt_due(STORAGE_PROMPT_BYTES - 1, &s));
        assert!(storage_prompt_due(STORAGE_PROMPT_BYTES, &s));
        let dir = tempfile::tempdir().unwrap();
        act(&store, dir.path(), &RetentionAction::StoragePromptSeen, NOW).unwrap();
        let s = RetentionState::load(&store).unwrap();
        assert!(s.storage_prompted && s.warned.contains_key(&MeetingId(7)));
        assert!(!storage_prompt_due(STORAGE_PROMPT_BYTES * 2, &s));
    }

    #[test]
    fn actions_parse_from_the_ui() {
        let a: RetentionAction =
            serde_json::from_str(r#"{"kind":"delete_as_set","ids":[1,2]}"#).unwrap();
        assert_eq!(
            a,
            RetentionAction::DeleteAsSet {
                ids: vec![MeetingId(1), MeetingId(2)]
            }
        );
        let a: RetentionAction = serde_json::from_str(r#"{"kind":"storage_prompt_seen"}"#).unwrap();
        assert_eq!(a, RetentionAction::StoragePromptSeen);
    }
}
