//! Tauri commands: thin wrappers that run one `meet_core` call (or a few) on
//! the blocking pool and map errors to strings. Argument names are
//! snake_case on the wire, matching `docs/plans/03-phase2-desktop-ui.md`.

use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use meet_core::audio::{self, ChainDecoder};
use meet_core::export::{self, ExportDoc, ExportFormat};
use meet_core::models::{self, DataPaths, Manifest};
use meet_core::pipeline::{self, CancelOutcome, CancelToken, ImportOptions, ImportRequest};
use meet_core::settings;
use meet_core::store::RunStore;
use meet_core::types::{
    AudioTrack, JobId, MeetingId, ModelFile, ModelFileId, ModelRole, SegmentId, SpeakerId,
    TrackChannel, Word, WordId,
};
use meet_core::views::{
    EngineStatus, JobEvent, MeetingDetail, MeetingSummary, SearchHit, SegmentView, Settings,
    Waveform,
};
use meet_core::CoreError;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::asset::asset_url;
use crate::events::{emit_download, emit_job, DownloadProgress};
use crate::{demo, AppPaths, AppStore, GpuCache, Jobs};

/// Where imports run.
const DEVICE: &str = "cuda:0";
/// How long a `nemo-speech doctor` report (GPU, free VRAM) is reused.
const DOCTOR_TTL: Duration = Duration::from_secs(20);
/// Bytes of the last engine log shown in Diagnostics.
const ENGINE_LOG_TAIL: usize = 8_192;

type CmdResult<T> = Result<T, String>;

/// Runs a store call on the blocking pool.
pub(crate) async fn with_store<T, E, F>(store: &State<'_, AppStore>, f: F) -> CmdResult<T>
where
    T: Send + 'static,
    E: Display,
    F: FnOnce(&dyn RunStore) -> Result<T, E> + Send + 'static,
{
    let store = Arc::clone(&store.0);
    tauri::async_runtime::spawn_blocking(move || f(store.as_ref()).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
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

/// Queues an import on the process-wide [`Jobs`] queue. Progress, `job:done`,
/// `job:error` and `job:cancelled` arrive as events.
#[tauri::command(rename_all = "snake_case")]
pub async fn import_file(
    app: AppHandle,
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    jobs: State<'_, Jobs>,
    path: String,
    title: Option<String>,
) -> CmdResult<JobId> {
    let job_id = JobId::new();
    let req = ImportRequest {
        job_id: job_id.clone(),
        path: PathBuf::from(&path),
        title: title.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()),
        diarize: true,
    };
    log::info!("import queued: {path} (job {job_id})");
    let store = Arc::clone(&store.0);
    let data_paths = paths.paths.clone();
    jobs.0.submit(job_id.clone(), move |token| {
        run_import(&app, store.as_ref(), &data_paths, req, token)
    });
    Ok(job_id)
}

/// The job body: the selected engine and models, the current settings, the
/// decoder chain, then `pipeline::import_file`.
fn run_import(
    app: &AppHandle,
    store: &dyn RunStore,
    paths: &DataPaths,
    req: ImportRequest,
    token: &CancelToken,
) {
    let job_id = req.job_id.clone();
    let source = req.path.display().to_string();
    let fail = |message: String| {
        log::error!("import failed: {source}: {message}");
        emit_job(
            app,
            &JobEvent::Error {
                job_id: job_id.clone(),
                meeting_id: None,
                message,
            },
        );
    };
    let settings = match settings::effective(store) {
        Ok(s) => s,
        Err(e) => return fail(e.to_string()),
    };
    let built = match models::select_engine(paths, store, DEVICE) {
        Ok(b) => b,
        Err(e) => return fail(e.to_string()),
    };
    log::info!(
        "import {job_id}: engine {} | asr {} | diarizer {}{}",
        built.selection.engine_version.as_deref().unwrap_or("?"),
        built.selection.asr_file.display_name,
        built.selection.diarizer_file.display_name,
        if built.experimental {
            " (experimental cascade)"
        } else {
            ""
        }
    );
    let mut opts = ImportOptions::new(&paths.root, &settings);
    opts.diarize_runner = Some(built.diarize_runner);
    let started = Instant::now();
    let result = pipeline::import_file(
        store,
        &*built.engine,
        &ChainDecoder::new(),
        req,
        &opts,
        token,
        &|ev| emit_job(app, &ev),
    );
    let secs = started.elapsed().as_secs_f64();
    match result {
        Ok(id) => log::info!("import done: {source} -> meeting {id} in {secs:.1} s"),
        Err(CoreError::Cancelled) => log::info!("import cancelled: {source} after {secs:.1} s"),
        Err(e) => log::error!("import failed: {source}: {e}"),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cancel_job(app: AppHandle, jobs: State<'_, Jobs>, job_id: JobId) -> CmdResult<()> {
    match jobs.0.cancel(&job_id) {
        // The running job notices its token and emits job:cancelled itself.
        CancelOutcome::Signalled => Ok(()),
        CancelOutcome::Dequeued => {
            // A queued post-pass leaves its meeting with the live transcript.
            let meeting_id = crate::live::dequeued_post_pass(&app, &job_id);
            emit_job(&app, &JobEvent::Cancelled { job_id, meeting_id });
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

/// Clears the "new label" badge.
#[tauri::command(rename_all = "snake_case")]
pub async fn mark_reviewed(store: State<'_, AppStore>, speaker_id: SpeakerId) -> CmdResult<()> {
    with_store(&store, move |s| s.mark_reviewed(speaker_id)).await
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

// ---- Export ------------------------------------------------------------------------

/// The meeting rendered as `format` (`meet_core::export`) and its file name.
async fn render_export(
    store: &State<'_, AppStore>,
    id: MeetingId,
    format: ExportFormat,
) -> CmdResult<(String, String)> {
    with_store(store, move |s| -> meet_core::Result<(String, String)> {
        let doc = ExportDoc::from_store(s, id)?;
        Ok((
            export::file_name(&doc.meeting.title, format),
            export::render(&doc, format),
        ))
    })
    .await
}

fn write_export(id: MeetingId, format: ExportFormat, path: &Path, text: &str) -> CmdResult<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
    log::info!("exported meeting {id} as {format:?} to {}", path.display());
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
    let (name, text) = render_export(&store, id, format).await?;
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
        write_export(id, format, &path, &text)?;
        Ok(Some(path.display().to_string()))
    })
    .await
}

/// Writes the transcript to `path` without a dialog (automation, tests).
#[tauri::command(rename_all = "snake_case")]
pub async fn export_meeting_to(
    store: State<'_, AppStore>,
    id: MeetingId,
    format: ExportFormat,
    path: String,
) -> CmdResult<String> {
    let (_, text) = render_export(&store, id, format).await?;
    blocking(move || {
        let path = PathBuf::from(path);
        write_export(id, format, &path, &text)?;
        Ok(path.display().to_string())
    })
    .await
}

// ---- Audio --------------------------------------------------------------------

/// The track to play (`Store::playback_track`, or the newest of `channel`),
/// its file resolved against the data root, and the meeting's uid.
async fn playable_track(
    store: &State<'_, AppStore>,
    paths: &DataPaths,
    id: MeetingId,
    channel: Option<TrackChannel>,
) -> CmdResult<(AudioTrack, PathBuf, String)> {
    let (track, uid) = with_store(store, move |s| {
        let track = match channel {
            None | Some(TrackChannel::Playback) => s.playback_track(id)?,
            Some(c) => s
                .list_audio_tracks(id)?
                .into_iter()
                .filter(|t| t.channel == c)
                .max_by_key(|t| t.id),
        };
        Ok::<_, meet_core::store::StoreError>((track, s.get_meeting(id)?.meeting.uid))
    })
    .await?;
    let track = track.ok_or_else(|| "No audio file for this meeting".to_owned())?;
    let file = paths.absolute(&track.path);
    if !file.is_file() {
        return Err(format!("The audio file is missing: {}", file.display()));
    }
    Ok((track, file, uid))
}

/// Asset-protocol URL of the meeting's audio. The file is added to the asset
/// scope on demand (imports live outside the data root).
#[tauri::command(rename_all = "snake_case")]
pub async fn get_audio_url(
    app: AppHandle,
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    id: MeetingId,
    channel: Option<TrackChannel>,
) -> CmdResult<String> {
    let (_, file, _) = playable_track(&store, &paths.paths, id, channel).await?;
    app.asset_protocol_scope()
        .allow_file(&file)
        .map_err(|e| e.to_string())?;
    Ok(asset_url(&file))
}

/// Peak envelope of the playback track (`meet_core::audio::peaks`), cached
/// next to stored audio or in the meeting folder for an import's original.
#[tauri::command(rename_all = "snake_case")]
pub async fn get_waveform(
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    id: MeetingId,
    buckets: u32,
) -> CmdResult<Waveform> {
    let (track, file, uid) = playable_track(&store, &paths.paths, id, None).await?;
    let root = paths.paths.root.clone();
    blocking(move || {
        let cache = audio::peaks::cache_path(&root, &uid, &track, &file);
        audio::peaks_cached(&file, buckets as usize, &cache, &ChainDecoder::new())
            .map_err(|e| e.to_string())
    })
    .await
}

// ---- Settings, models, engine -------------------------------------------------

/// Stored settings laid over `meet_core::settings::DEFAULTS`.
#[tauri::command]
pub async fn get_settings(store: State<'_, AppStore>) -> CmdResult<Settings> {
    with_store(&store, |s| settings::effective(s)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_setting(store: State<'_, AppStore>, key: String, value: String) -> CmdResult<()> {
    with_store(&store, move |s| s.set_setting(&key, &value)).await
}

/// Restores a setting's default.
#[tauri::command(rename_all = "snake_case")]
pub async fn remove_setting(store: State<'_, AppStore>, key: String) -> CmdResult<()> {
    with_store(&store, move |s| s.remove_setting(&key)).await
}

#[tauri::command]
pub async fn engine_status(
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    gpu: State<'_, GpuCache>,
) -> CmdResult<EngineStatus> {
    let data_paths = paths.paths.clone();
    if paths.demo {
        return with_store(&store, move |s| -> meet_core::Result<EngineStatus> {
            let (free, device) = demo::gpu();
            Ok(EngineStatus::from_parts(
                data_paths.root.display().to_string(),
                s.list_model_files()?,
                &s.get_settings()?,
                models::TESTED_ENGINE_VERSIONS,
                free,
                device,
            ))
        })
        .await;
    }
    let cache = Arc::clone(&gpu.0);
    with_store(&store, move |s| {
        let doctor = {
            let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
            match c.as_ref() {
                Some((at, report)) if at.elapsed() < DOCTOR_TTL => report.clone(),
                _ => {
                    let report = models::selected_file(s, ModelRole::Engine)
                        .ok()
                        .flatten()
                        .and_then(|e| models::doctor(&data_paths.absolute(&e.path)).ok());
                    *c = Some((Instant::now(), report.clone()));
                    report
                }
            }
        };
        models::status(&data_paths, s, doctor.as_ref())
    })
    .await
}

#[tauri::command]
pub async fn list_model_files(store: State<'_, AppStore>) -> CmdResult<Vec<ModelFile>> {
    with_store(&store, |s| s.list_model_files()).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn select_model(
    store: State<'_, AppStore>,
    gpu: State<'_, GpuCache>,
    role: ModelRole,
    id: ModelFileId,
) -> CmdResult<()> {
    let file = with_store(&store, move |s| models::select_by_id(s, role, id)).await?;
    if role == ModelRole::Engine {
        *gpu.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    log::info!("selected {role}: {} ({})", file.display_name, file.path);
    Ok(())
}

/// Scans `models\` and `engine\` (`meet_core::models::scan`), keeping the
/// selections valid. The demo has no folders to scan.
#[tauri::command]
pub async fn rescan_models(
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    gpu: State<'_, GpuCache>,
) -> CmdResult<Vec<ModelFile>> {
    if paths.demo {
        return with_store(&store, |s| s.list_model_files()).await;
    }
    *gpu.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let data_paths = paths.paths.clone();
    with_store(&store, move |s| -> meet_core::Result<Vec<ModelFile>> {
        let report = models::scan(&data_paths, s, &Manifest::load(&data_paths.root)?)?;
        for n in &report.notes {
            log::info!("model scan: {n}");
        }
        for e in &report.errors {
            log::warn!("model scan: {e}");
        }
        Ok(report.files)
    })
    .await
}

/// Downloads and extracts a runtime from the manifest (`name` or version;
/// the default tested engine when absent), then rescans. Progress arrives as
/// `models:progress`. Returns the installed folder.
#[tauri::command(rename_all = "snake_case")]
pub async fn install_engine(
    app: AppHandle,
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    name: Option<String>,
) -> CmdResult<String> {
    let data_paths = paths.paths.clone();
    with_store(&store, move |s| -> meet_core::Result<String> {
        let manifest = Manifest::load(&data_paths.root)?;
        let entry = match name.as_deref() {
            Some(n) => manifest
                .engines
                .iter()
                .find(|e| e.name == n || e.version == n),
            None => manifest.default_engine(),
        }
        .ok_or_else(|| CoreError::Models("no such engine in the manifest".into()))?
        .clone();
        let progress = |done: u64, total: Option<u64>, finished: bool, error: Option<String>| {
            emit_download(
                &app,
                &DownloadProgress {
                    kind: "engine",
                    name: entry.name.clone(),
                    done_bytes: done,
                    total_bytes: total,
                    finished,
                    error,
                },
            )
        };
        let mut last = Instant::now() - Duration::from_secs(1);
        let result = models::install_engine(&entry, &data_paths, &mut |done, total| {
            if last.elapsed() >= Duration::from_millis(250) {
                last = Instant::now();
                progress(done, total, false, None);
            }
        })
        .and_then(|dir| {
            models::scan(&data_paths, s, &manifest)?;
            Ok(dir)
        });
        let size = Some(entry.size_bytes);
        match &result {
            Ok(dir) => {
                log::info!("installed engine {}", dir.display());
                progress(entry.size_bytes, size, true, None);
            }
            Err(e) => progress(0, size, true, Some(e.to_string())),
        }
        result.map(|d| d.display().to_string())
    })
    .await
}

/// Pulls a manifest model (`name` or its pull name) with the selected
/// runtime's `nemo-speech pull`, verifies it, then rescans. Returns the file.
#[tauri::command(rename_all = "snake_case")]
pub async fn pull_model(
    app: AppHandle,
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    name: String,
) -> CmdResult<String> {
    let data_paths = paths.paths.clone();
    with_store(&store, move |s| -> meet_core::Result<String> {
        let manifest = Manifest::load(&data_paths.root)?;
        let entry = manifest
            .model(&name)
            .ok_or_else(|| CoreError::Models(format!("no model {name:?} in the manifest")))?
            .clone();
        let engine = match models::selected_file(s, ModelRole::Engine)? {
            Some(e) => e,
            None => {
                models::scan(&data_paths, s, &manifest)?;
                models::selected_file(s, ModelRole::Engine)?.ok_or_else(|| {
                    CoreError::Models("no engine installed; install one first".into())
                })?
            }
        };
        let progress = |done: u64, finished: bool, error: Option<String>| {
            emit_download(
                &app,
                &DownloadProgress {
                    kind: "model",
                    name: entry.name.clone(),
                    done_bytes: done,
                    total_bytes: Some(entry.size_bytes),
                    finished,
                    error,
                },
            )
        };
        progress(0, false, None);
        let result = models::pull(
            &data_paths.absolute(&engine.path),
            &data_paths.models(),
            &entry,
        )
        .and_then(|p| {
            models::scan(&data_paths, s, &manifest)?;
            Ok(p)
        });
        match &result {
            Ok(p) => {
                log::info!("pulled {} to {}", entry.name, p.path.display());
                progress(entry.size_bytes, true, None);
            }
            Err(e) => progress(0, true, Some(e.to_string())),
        }
        result.map(|p| p.path.display().to_string())
    })
    .await
}

/// One item of "Download recommended": the default runtime and the required
/// models from the manifest, with whether a matching file is already there.
// TODO(phase3b): move into `meet_core::views` with `DownloadProgress`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DownloadItem {
    /// `engine` or `model`.
    pub kind: &'static str,
    /// What `install_engine` / `pull_model` take.
    pub name: String,
    pub label: String,
    pub size_bytes: u64,
    pub installed: bool,
}

/// What "Download recommended" would fetch, engine first.
#[tauri::command]
pub async fn recommended_downloads(
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
) -> CmdResult<Vec<DownloadItem>> {
    let data_paths = paths.paths.clone();
    with_store(&store, move |s| -> meet_core::Result<Vec<DownloadItem>> {
        let manifest = Manifest::load(&data_paths.root)?;
        let files = s.list_model_files()?;
        let mut items = Vec::new();
        if let Some(e) = manifest.default_engine() {
            items.push(DownloadItem {
                kind: "engine",
                name: e.name.clone(),
                label: format!("Engine {} ({})", e.version, e.backend),
                size_bytes: e.size_bytes,
                installed: files.iter().any(|f| {
                    f.role == ModelRole::Engine
                        && !f.missing
                        && f.architecture.as_deref() == Some(e.version.as_str())
                }),
            });
        }
        for m in manifest.required() {
            items.push(DownloadItem {
                kind: "model",
                name: m.name.clone(),
                label: format!("{} ({})", m.name, m.role),
                size_bytes: m.size_bytes,
                installed: files
                    .iter()
                    .any(|f| !f.missing && f.manifest_name.as_deref() == Some(m.name.as_str())),
            });
        }
        Ok(items)
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
    open_folder(&paths.paths.models())
}

#[tauri::command]
pub async fn open_data_folder(paths: State<'_, AppPaths>) -> CmdResult<()> {
    open_folder(&paths.paths.root)
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

/// The runtime's stderr from the most recent engine job (its `engine.log` in
/// the meeting folder), or `None` when no job has run on this data root.
#[tauri::command]
pub async fn last_engine_stderr(paths: State<'_, AppPaths>) -> CmdResult<Option<String>> {
    let audio_root = paths.paths.audio();
    blocking(move || {
        Ok(pipeline::last_engine_log(&audio_root, ENGINE_LOG_TAIL)
            .map(|(path, tail)| format!("{}\n{tail}", path.display())))
    })
    .await
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
