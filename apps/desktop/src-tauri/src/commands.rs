//! Tauri commands: thin wrappers that run one `Store` call (or a few) on the
//! blocking pool and map errors to strings. Argument names are snake_case on
//! the wire, matching `docs/plans/03-phase2-desktop-ui.md`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use meet_core::pipeline::{CancelOutcome, ImportRequest};
use meet_core::store::{ScannedModelFile, Store, StoreError, StoreResult};
use meet_core::types::{
    AudioFormat, AudioTrack, JobId, MeetingId, ModelFile, ModelFileId, ModelRole, SegmentId,
    SpeakerId, TrackChannel, Word, WordId,
};
use meet_core::views::{
    selection_key, EngineStatus, JobEvent, MeetingDetail, MeetingSummary, SearchHit, SegmentView,
    Settings,
};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::audio::{self, Waveform};
use crate::events::emit_job;
use crate::export::{self, ExportFormat};
use crate::{mock, AppPaths, AppStore, Jobs};

/// Engine versions with recorded fixtures; others get an "untested" badge.
const TESTED_ENGINE_VERSIONS: &[&str] = &["0.2.0"];

/// Settings the UI reads, with their defaults (written at startup when absent).
pub const DEFAULT_SETTINGS: &[(&str, &str)] = &[
    ("audio_retention_days", "30"),
    ("theme", "system"),
    ("default_language", "en"),
    ("post_pass_mode", "always"),
];

type CmdResult<T> = Result<T, String>;

/// Runs a store call on the blocking pool.
pub(crate) async fn with_store<T, F>(store: &State<'_, AppStore>, f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce(&dyn Store) -> StoreResult<T> + Send + 'static,
{
    let store = Arc::clone(&store.0);
    tauri::async_runtime::spawn_blocking(move || f(store.as_ref()))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Runs any blocking closure on the blocking pool.
async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> CmdResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

pub fn apply_default_settings(store: &dyn Store) {
    let current = store.get_settings().unwrap_or_default();
    for (key, value) in DEFAULT_SETTINGS {
        if !current.0.contains_key(*key) {
            if let Err(e) = store.set_setting(key, value) {
                log::warn!("default setting {key}: {e}");
            }
        }
    }
}

// ---- Library and meeting view ----------------------------------------------

#[tauri::command]
pub async fn list_meetings(store: State<'_, AppStore>) -> CmdResult<Vec<MeetingSummary>> {
    with_store(&store, |s| s.list_meetings()).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_meeting(store: State<'_, AppStore>, id: MeetingId) -> CmdResult<MeetingDetail> {
    with_store(&store, move |s| s.get_meeting(id)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_segments(
    store: State<'_, AppStore>,
    id: MeetingId,
    offset: u32,
    limit: u32,
) -> CmdResult<Vec<SegmentView>> {
    with_store(&store, move |s| s.get_segments(id, offset, limit)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_words(
    store: State<'_, AppStore>,
    id: MeetingId,
    from_ms: i64,
    to_ms: i64,
) -> CmdResult<Vec<Word>> {
    with_store(&store, move |s| s.get_words(id, from_ms, to_ms)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn search(
    store: State<'_, AppStore>,
    query: String,
    limit: u32,
) -> CmdResult<Vec<SearchHit>> {
    with_store(&store, move |s| s.search(&query, limit)).await
}

// ---- Import jobs -------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
pub async fn import_file(
    app: AppHandle,
    store: State<'_, AppStore>,
    jobs: State<'_, Jobs>,
    path: String,
    title: Option<String>,
) -> CmdResult<JobId> {
    let settings = with_store(&store, |s| s.get_settings()).await?;
    let language = settings
        .0
        .get("default_language")
        .cloned()
        .unwrap_or_else(|| "en".into());
    let job_id = JobId::new();
    let req = ImportRequest {
        job_id: job_id.clone(),
        path: PathBuf::from(&path),
        title: title.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()),
        language,
        diarize: true,
    };
    log::info!("import queued: {path} (job {job_id})");
    let store = Arc::clone(&store.0);
    // Phase 1: `pipeline::import_file(store, engine, decoder, req, &|ev| emit_job(&app, &ev))`.
    jobs.0.submit(job_id.clone(), move |token| {
        mock::run_import(store.as_ref(), &app, req, token)
    });
    Ok(job_id)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cancel_job(app: AppHandle, jobs: State<'_, Jobs>, job_id: JobId) -> CmdResult<()> {
    match jobs.0.cancel(&job_id) {
        // The running job notices its token and emits job:error itself.
        CancelOutcome::Signalled => Ok(()),
        CancelOutcome::Dequeued => {
            emit_job(
                &app,
                &JobEvent::Error {
                    job_id,
                    meeting_id: None,
                    message: mock::CANCELLED.into(),
                },
            );
            Ok(())
        }
        CancelOutcome::NotFound => Err(format!("job {job_id} is not queued or running")),
    }
}

// ---- Speaker and segment edits -------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
pub async fn rename_speaker(
    store: State<'_, AppStore>,
    speaker_id: SpeakerId,
    name: String,
    anchor_segment_id: Option<SegmentId>,
) -> CmdResult<()> {
    with_store(&store, move |s| {
        s.rename_speaker(speaker_id, &name, anchor_segment_id)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_speaker_color(
    store: State<'_, AppStore>,
    speaker_id: SpeakerId,
    color: String,
) -> CmdResult<()> {
    with_store(&store, move |s| s.set_speaker_color(speaker_id, &color)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn merge_speakers(
    store: State<'_, AppStore>,
    from: SpeakerId,
    into: SpeakerId,
) -> CmdResult<()> {
    with_store(&store, move |s| s.merge_speakers(from, into)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn unmerge_speaker(store: State<'_, AppStore>, speaker_id: SpeakerId) -> CmdResult<()> {
    with_store(&store, move |s| s.unmerge_speaker(speaker_id)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn reassign_segment(
    store: State<'_, AppStore>,
    segment_id: SegmentId,
    speaker_id: SpeakerId,
) -> CmdResult<()> {
    with_store(&store, move |s| s.reassign_segment(segment_id, speaker_id)).await
}

/// Splits before `word_id`; returns the new (second) segment.
#[tauri::command(rename_all = "snake_case")]
pub async fn split_segment(
    store: State<'_, AppStore>,
    segment_id: SegmentId,
    word_id: WordId,
) -> CmdResult<SegmentId> {
    with_store(&store, move |s| s.split_segment(segment_id, word_id)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_meeting_title(
    store: State<'_, AppStore>,
    id: MeetingId,
    title: String,
) -> CmdResult<()> {
    with_store(&store, move |s| s.set_meeting_title(id, &title)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_audio_keep(
    store: State<'_, AppStore>,
    id: MeetingId,
    keep: bool,
) -> CmdResult<()> {
    with_store(&store, move |s| s.set_audio_keep(id, keep)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn delete_meeting(store: State<'_, AppStore>, id: MeetingId) -> CmdResult<()> {
    with_store(&store, move |s| s.delete_meeting(id)).await?;
    log::info!("deleted meeting {id}");
    Ok(())
}

/// Asks for a location with the save dialog and writes the transcript there.
/// Returns the written path, or `None` when the user cancelled.
#[tauri::command(rename_all = "snake_case")]
pub async fn export_meeting(
    app: AppHandle,
    store: State<'_, AppStore>,
    id: MeetingId,
    format: ExportFormat,
) -> CmdResult<Option<String>> {
    let (detail, segments) = with_store(&store, move |s| {
        Ok((s.get_meeting(id)?, s.get_segments(id, 0, u32::MAX)?))
    })
    .await?;
    let name = export::file_name(&detail.meeting.title, format);
    blocking(move || {
        let Some(target) = app
            .dialog()
            .file()
            .set_title("Export transcript")
            .set_file_name(name)
            .add_filter(format.filter_name(), &[format.extension()])
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = target.into_path().map_err(|e| e.to_string())?;
        std::fs::write(&path, export::render(format, &detail, &segments))
            .map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
        log::info!(
            "exported meeting {id} as {:?} to {}",
            format,
            path.display()
        );
        Ok(Some(path.display().to_string()))
    })
    .await
}

// ---- Audio --------------------------------------------------------------------

/// The newest track of `channel` whose file exists, resolved against the data root.
async fn playable_track(
    store: &State<'_, AppStore>,
    data_root: &Path,
    id: MeetingId,
    channel: TrackChannel,
) -> CmdResult<(AudioTrack, PathBuf)> {
    let mut tracks = with_store(store, move |s| s.list_audio_tracks(id)).await?;
    tracks.retain(|t| t.channel == channel);
    tracks.sort_by_key(|t| std::cmp::Reverse(t.id));
    tracks
        .into_iter()
        .map(|t| {
            let p = Path::new(&t.path);
            let path = if t.format == AudioFormat::Original || p.is_absolute() {
                p.to_path_buf()
            } else {
                data_root.join(p)
            };
            (t, path)
        })
        .find(|(_, p)| p.is_file())
        .ok_or_else(|| "No audio file for this meeting".to_owned())
}

/// Asset-protocol URL of the meeting's audio (`playback` by default). The
/// file is added to the asset scope on demand (imports live outside the data root).
#[tauri::command(rename_all = "snake_case")]
pub async fn get_audio_url(
    app: AppHandle,
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    id: MeetingId,
    channel: Option<TrackChannel>,
) -> CmdResult<String> {
    let channel = channel.unwrap_or(TrackChannel::Playback);
    let (_, path) = playable_track(&store, &paths.data_root, id, channel).await?;
    app.asset_protocol_scope()
        .allow_file(&path)
        .map_err(|e| e.to_string())?;
    Ok(audio::asset_url(&path))
}

/// Peak envelope of the playback track, computed in Rust.
#[tauri::command(rename_all = "snake_case")]
pub async fn get_waveform(
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    id: MeetingId,
    buckets: u32,
) -> CmdResult<Waveform> {
    let (_, path) = playable_track(&store, &paths.data_root, id, TrackChannel::Playback).await?;
    blocking(move || audio::wav_peaks(&path, buckets as usize).map_err(|e| e.to_string())).await
}

// ---- Settings, models, engine -------------------------------------------------

#[tauri::command]
pub async fn get_settings(store: State<'_, AppStore>) -> CmdResult<Settings> {
    with_store(&store, |s| s.get_settings()).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_setting(store: State<'_, AppStore>, key: String, value: String) -> CmdResult<()> {
    with_store(&store, move |s| s.set_setting(&key, &value)).await
}

#[tauri::command]
pub async fn engine_status(
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
) -> CmdResult<EngineStatus> {
    let data_root = paths.data_root.display().to_string();
    let (files, settings) =
        with_store(&store, |s| Ok((s.list_model_files()?, s.get_settings()?))).await?;
    let (free_vram_mb, gpu) = mock::gpu();
    Ok(EngineStatus::from_parts(
        data_root,
        files,
        &settings,
        TESTED_ENGINE_VERSIONS,
        free_vram_mb,
        gpu,
    ))
}

#[tauri::command]
pub async fn list_model_files(store: State<'_, AppStore>) -> CmdResult<Vec<ModelFile>> {
    with_store(&store, |s| s.list_model_files()).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn select_model(
    store: State<'_, AppStore>,
    role: ModelRole,
    id: ModelFileId,
) -> CmdResult<()> {
    with_store(&store, move |s| {
        if role == ModelRole::Unknown {
            return Err(StoreError::Invalid(
                "unrecognised files cannot be selected".into(),
            ));
        }
        let files = s.list_model_files()?;
        let file = files
            .iter()
            .find(|f| f.id == id)
            .ok_or_else(|| StoreError::not_found("model file", id))?;
        if file.role != role {
            return Err(StoreError::Invalid(format!(
                "{} is not a {} file",
                file.display_name, role
            )));
        }
        if file.missing {
            return Err(StoreError::Invalid(format!(
                "{} is missing from disk",
                file.display_name
            )));
        }
        s.set_setting(&selection_key(role), &id.to_string())
    })
    .await
}

/// Mock rescan: re-syncs the known files. Phase 1's `models::scan` walks
/// `models\` and `engine\` and passes what it finds to `sync_model_files`.
#[tauri::command]
pub async fn rescan_models(store: State<'_, AppStore>) -> CmdResult<Vec<ModelFile>> {
    with_store(&store, |s| {
        let found: Vec<ScannedModelFile> = s
            .list_model_files()?
            .into_iter()
            .filter(|f| !f.missing)
            .map(|f| ScannedModelFile {
                path: f.path,
                role: f.role,
                architecture: f.architecture,
                display_name: f.display_name,
                quant: f.quant,
                size_bytes: f.size_bytes,
                sha256: f.sha256,
                source: f.source,
                capabilities: f.capabilities,
                manifest_name: f.manifest_name,
            })
            .collect();
        s.sync_model_files(&found, mock::now_ms())
    })
    .await
}

fn open_folder(path: &Path) -> CmdResult<()> {
    std::fs::create_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Cannot open {}: {e}", path.display()))
}

#[tauri::command]
pub async fn open_models_folder(paths: State<'_, AppPaths>) -> CmdResult<()> {
    open_folder(&paths.data_root.join("models"))
}

#[tauri::command]
pub async fn open_data_folder(paths: State<'_, AppPaths>) -> CmdResult<()> {
    open_folder(&paths.data_root)
}

// ---- Diagnostics ----------------------------------------------------------------

/// The last `max_lines` lines of the newest app log file.
#[tauri::command(rename_all = "snake_case")]
pub async fn read_log_tail(app: AppHandle, max_lines: u32) -> CmdResult<String> {
    let dir = app.path().app_log_dir().map_err(|e| e.to_string())?;
    blocking(move || {
        let newest = std::fs::read_dir(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
            .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
            .ok_or_else(|| format!("No log file in {}", dir.display()))?;
        let bytes = std::fs::read(newest.path()).map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text.lines().collect();
        let start = lines.len().saturating_sub(max_lines as usize);
        Ok(format!(
            "{}\n{}",
            newest.path().display(),
            lines[start..].join("\n")
        ))
    })
    .await
}

/// Stderr of the last engine job. No engine runs in the mock.
#[tauri::command]
pub async fn last_engine_stderr() -> CmdResult<Option<String>> {
    Ok(None)
}

/// Lets the UI write to the app log (startup timing, unexpected errors).
#[tauri::command(rename_all = "snake_case")]
pub async fn ui_log(level: String, message: String) -> CmdResult<()> {
    match level.as_str() {
        "error" => log::error!(target: "ui", "{message}"),
        "warn" => log::warn!(target: "ui", "{message}"),
        _ => log::info!(target: "ui", "{message}"),
    }
    Ok(())
}
