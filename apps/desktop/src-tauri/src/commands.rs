//! Tauri commands: thin wrappers that run one `diarist_core` call (or a few) on
//! the blocking pool and map errors to strings. Argument names are
//! snake_case on the wire, matching `docs/plans/03-phase2-desktop-ui.md`.

use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use diarist_core::audio::{self, ChainDecoder};
use diarist_core::diagnostics::{self, DiagnosticsInput, HealthCheck};
use diarist_core::engine::ServeStatus;
use diarist_core::export::{self, ExportDoc, ExportFormat};
use diarist_core::gpu;
use diarist_core::models::releases::{self, RuntimeRelease};
use diarist_core::models::{self, legacy, relocate, DataPaths, Manifest};
use diarist_core::pipeline::{self, CancelOutcome, CancelToken, ImportOptions, ImportRequest};
use diarist_core::retention::{self, RetentionPlan, RetentionReport};
use diarist_core::settings;
use diarist_core::store::{RunStore, Store};
use diarist_core::types::{
    AudioTrack, JobId, MeetingId, ModelFile, ModelFileId, ModelRole, SegmentId, SpeakerId,
    TrackChannel, Word, WordId,
};
use diarist_core::views::{
    DataRootInfo, DownloadItem, DownloadKind, DownloadProgress, EngineStatus, JobEvent,
    MeetingDetail, MeetingSummary, MoveProgress, MoveResult, SearchHit, SegmentView, Settings,
    StartupReport, UpdateCheck, Waveform,
};
use diarist_core::CoreError;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::asset::asset_url;
use crate::events::{emit_download, emit_job};
use crate::{demo, AppPaths, AppStore, GpuCache, Jobs, Legacy, Startup};

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

/// Queues an import on the process-wide [`Jobs`] queue in `language`
/// (default: the `default_language` setting). Progress, `job:done`,
/// `job:error` and `job:cancelled` arrive as events.
#[tauri::command(rename_all = "snake_case")]
pub async fn import_file(
    app: AppHandle,
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    jobs: State<'_, Jobs>,
    path: String,
    title: Option<String>,
    language: Option<String>,
) -> CmdResult<JobId> {
    let job_id = JobId::new();
    let req = ImportRequest {
        job_id: job_id.clone(),
        path: PathBuf::from(&path),
        title: title.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()),
        diarize: true,
    };
    let language = language
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty());
    log::info!(
        "import queued: {path} (job {job_id}, language {})",
        language.as_deref().unwrap_or("default")
    );
    let store = Arc::clone(&store.0);
    let data_paths = paths.paths.clone();
    jobs.0.submit(job_id.clone(), move |token| {
        run_import(&app, store.as_ref(), &data_paths, req, language, token)
    });
    Ok(job_id)
}

/// The job body: the engine and models for `language` (default
/// `default_language`), the current settings, the decoder chain, then
/// `pipeline::import_file`.
fn run_import(
    app: &AppHandle,
    store: &dyn RunStore,
    paths: &DataPaths,
    req: ImportRequest,
    language: Option<String>,
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
    let low_vram = settings::get_bool(&settings, settings::ENGINE_LOW_VRAM);
    let language = models::languages::canonical(
        &language.unwrap_or_else(|| settings::default_language(&settings)),
    );
    // A language the selected model does not cover runs on Nemotron 3.5, or
    // fails here naming it (the readiness check offers the download first).
    let (built, switched) =
        match models::select_import_engine(paths, store, DEVICE, low_vram, &language) {
            Ok(b) => b,
            Err(e) => return fail(e.to_string()),
        };
    let notice = switched.and_then(|reason| {
        let selected = models::selected_file(store, ModelRole::Asr)
            .ok()
            .flatten()?;
        Some(models::switch_notice(
            reason,
            &selected,
            &built.selection.asr_file,
            &language,
        ))
    });
    log::info!(
        "import {job_id}: engine {} | asr {}{} | diarizer {}{} | language {language}",
        built.selection.engine_version.as_deref().unwrap_or("?"),
        built.selection.asr_file.display_name,
        notice.map(|n| format!(" ({n})")).unwrap_or_default(),
        built.selection.diarizer_file.display_name,
        if built.experimental {
            " (experimental cascade)"
        } else {
            ""
        }
    );
    let mut opts = ImportOptions::new(&paths.root, &settings);
    opts.language = language;
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

/// Renames a speaker while recording: the anchor is the time range (meeting
/// ms) of the live line the rename was invoked on, since there are no
/// segments yet.
#[tauri::command(rename_all = "snake_case")]
pub async fn rename_speaker_at(
    store: State<'_, AppStore>,
    speaker_id: SpeakerId,
    name: String,
    start_ms: i64,
    end_ms: i64,
) -> CmdResult<()> {
    with_store(&store, move |s| {
        s.rename_speaker_at(speaker_id, &name, start_ms, end_ms)
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

/// The meeting rendered as `format` (`diarist_core::export`) and its file name.
async fn render_export(
    store: &State<'_, AppStore>,
    id: MeetingId,
    format: ExportFormat,
) -> CmdResult<(String, String)> {
    with_store(store, move |s| -> diarist_core::Result<(String, String)> {
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
        Ok::<_, diarist_core::store::StoreError>((track, s.get_meeting(id)?.meeting.uid))
    })
    .await?;
    let track = track.ok_or_else(|| "No audio file for this meeting (audio retention may have deleted it; pin meetings with P to keep their audio)".to_owned())?;
    let file = paths.absolute(&track.path);
    if !file.is_file() {
        return Err(format!(
            "The audio file is missing: {} (it was moved or deleted; put it back there to play it)",
            file.display()
        ));
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

/// Peak envelope of the playback track (`diarist_core::audio::peaks`), cached
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

/// Stored settings laid over `diarist_core::settings::DEFAULTS`.
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
        return with_store(&store, move |s| -> diarist_core::Result<EngineStatus> {
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

/// Scans `models\` and `engine\` (`diarist_core::models::scan`), keeping the
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
    with_store(&store, move |s| -> diarist_core::Result<Vec<ModelFile>> {
        let manifest = Manifest::load(&data_paths.root)?;
        install_dropped(&data_paths, &manifest);
        let report = models::scan(&data_paths, s, &manifest)?;
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

/// The last download that failed (for the firewall hint in the checklist):
/// what it was, which process opened the connection, and the error.
#[derive(Default)]
pub(crate) struct LastDownloadError(pub(crate) std::sync::Mutex<Option<(String, String, String)>>);

fn note_download(app: &AppHandle, what: &str, process: &str, result: &Result<(), String>) {
    let state = app.state::<LastDownloadError>();
    let mut slot = state.0.lock().unwrap_or_else(|e| e.into_inner());
    *slot = result
        .as_ref()
        .err()
        .map(|e| (what.to_owned(), process.to_owned(), e.clone()));
}

/// Emits `models:progress` for one item.
fn emit_progress(
    app: &AppHandle,
    kind: DownloadKind,
    name: &str,
    done: u64,
    total: Option<u64>,
    finished: bool,
    error: Option<String>,
) {
    emit_download(
        app,
        &DownloadProgress {
            kind,
            name: name.to_owned(),
            done_bytes: done,
            total_bytes: total,
            finished,
            error,
        },
    );
}

/// Downloads, checks and extracts `entry`, rescans and selects it (the user
/// chose this package). Progress arrives as `models:progress`.
fn run_engine_install(
    app: &AppHandle,
    s: &dyn RunStore,
    data_paths: &DataPaths,
    manifest: &Manifest,
    entry: &models::EngineEntry,
) -> diarist_core::Result<String> {
    let mut last = Instant::now() - Duration::from_secs(1);
    let size = Some(entry.size_bytes);
    let result = models::install_engine(entry, data_paths, &mut |done, total| {
        if last.elapsed() >= Duration::from_millis(250) {
            last = Instant::now();
            emit_progress(
                app,
                DownloadKind::Engine,
                &entry.name,
                done,
                total,
                false,
                None,
            );
        }
    })
    .and_then(|dir| {
        models::scan(data_paths, s, manifest)?;
        let rel = data_paths.relative(&dir);
        if let Ok(f) = models::select(s, ModelRole::Engine, &rel) {
            log::info!("selected engine {} ({})", f.display_name, f.path);
        }
        Ok(dir)
    });
    match &result {
        Ok(dir) => {
            log::info!("installed engine {}", dir.display());
            emit_progress(
                app,
                DownloadKind::Engine,
                &entry.name,
                entry.size_bytes,
                size,
                true,
                None,
            );
        }
        Err(e) => {
            log::error!("engine install {} failed: {e}", entry.name);
            emit_progress(
                app,
                DownloadKind::Engine,
                &entry.name,
                0,
                size,
                true,
                Some(e.to_string()),
            );
        }
    }
    note_download(
        app,
        &entry.name,
        "diarist.exe",
        &result.as_ref().map(|_| ()).map_err(|e| e.to_string()),
    );
    result.map(|d| d.display().to_string())
}

/// Downloads and extracts a runtime package from the manifest (`name` or
/// version; the recommended package for this GPU when absent), then rescans
/// and selects it. Progress arrives as `models:progress`. Returns the folder.
#[tauri::command(rename_all = "snake_case")]
pub async fn install_engine(
    app: AppHandle,
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    gpu: State<'_, GpuCache>,
    name: Option<String>,
) -> CmdResult<String> {
    *gpu.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let data_paths = paths.paths.clone();
    with_store(&store, move |s| -> diarist_core::Result<String> {
        let manifest = Manifest::load(&data_paths.root)?;
        let backend = recommended_backend_now();
        let entry = match name.as_deref() {
            Some(n) => manifest
                .engines
                .iter()
                .find(|e| e.name == n || e.version == n),
            None => manifest
                .engine_for_backend(backend)
                .or_else(|| manifest.default_engine()),
        }
        .ok_or_else(|| {
            CoreError::Models(
                "no such runtime in the manifest; use Check for newer runtime, or put an extracted \
                 runtime into the engine folder"
                    .into(),
            )
        })?
        .clone();
        run_engine_install(&app, s, &data_paths, &manifest, &entry)
    })
    .await
}

/// The runtime package this machine should use (NVML, then DXGI).
fn recommended_backend_now() -> &'static str {
    gpu::recommended_backend(gpu::gpu_info(0).as_ref(), &gpu::adapters())
}

/// Fetches a manifest model and verifies it, then rescans. Shipped models go
/// through the selected runtime's `nemo-speech pull` (its downloader is
/// System32 `curl.exe`); `models.user.json` entries are downloaded from their
/// URL directly. Returns the file.
#[tauri::command(rename_all = "snake_case")]
pub async fn pull_model(
    app: AppHandle,
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    name: String,
) -> CmdResult<String> {
    let data_paths = paths.paths.clone();
    with_store(&store, move |s| -> diarist_core::Result<String> {
        let manifest = Manifest::load(&data_paths.root)?;
        let entry = manifest
            .model(&name)
            .ok_or_else(|| {
                CoreError::Models(format!(
                    "no model {name:?} in the manifest; press Refresh, or add it with Add from URL"
                ))
            })?
            .clone();
        let total = Some(entry.size_bytes);
        let progress = |done: u64, finished: bool, error: Option<String>| {
            emit_progress(
                &app,
                DownloadKind::Model,
                &entry.name,
                done,
                total,
                finished,
                error,
            )
        };
        progress(0, false, None);
        let mut last_done = u64::MAX;
        let mut tick = |done: u64| {
            if done != last_done {
                last_done = done;
                progress(done, false, None);
            }
        };
        let (process, result) = if entry.direct {
            let dest = entry.dest_dir(&data_paths.models()).join(&entry.file);
            let r =
                models::download_file(&entry.url, &dest, &entry.sha256, total, &mut |d, _| tick(d))
                    .map(|()| dest);
            ("diarist.exe", r)
        } else {
            let engine = match models::selected_file(s, ModelRole::Engine)? {
                Some(e) => e,
                None => {
                    models::scan(&data_paths, s, &manifest)?;
                    models::selected_file(s, ModelRole::Engine)?.ok_or_else(|| {
                        CoreError::Models(
                            "the models are fetched by the speech engine, which is not installed; \
                             install the engine first"
                                .into(),
                        )
                    })?
                }
            };
            let r = models::pull_with_progress(
                &data_paths.absolute(&engine.path),
                &data_paths.models(),
                &entry,
                &mut tick,
            )
            .map(|p| p.path);
            ("curl.exe (started by nemo-speech.exe)", r)
        };
        let result = result.and_then(|p| {
            models::scan(&data_paths, s, &manifest)?;
            Ok(p)
        });
        match &result {
            Ok(p) => {
                log::info!("fetched {} to {}", entry.name, p.display());
                progress(entry.size_bytes, true, None);
            }
            Err(e) => {
                log::error!("fetching {} failed: {e}", entry.name);
                progress(0, true, Some(e.to_string()))
            }
        }
        note_download(
            &app,
            &entry.name,
            process,
            &result.as_ref().map(|_| ()).map_err(|e| e.to_string()),
        );
        result.map(|p| p.display().to_string())
    })
    .await
}

/// Runtime releases on GitHub with their Windows packages ("Check for
/// newer runtime"); packages already under `engine\` are marked installed.
#[tauri::command]
pub async fn runtime_releases(paths: State<'_, AppPaths>) -> CmdResult<Vec<RuntimeRelease>> {
    let engine_root = paths.paths.engine();
    blocking(move || {
        let mut list = releases::fetch_releases().map_err(|e| e.to_string())?;
        for r in &mut list {
            for p in &mut r.packages {
                p.installed = engine_root.join(&p.name).join("bin").is_dir();
            }
        }
        Ok(list)
    })
    .await
}

/// Installs a package from the GitHub releases into `engine\<name>\`,
/// checked against the release's `.sha256`, and selects it.
#[tauri::command(rename_all = "snake_case")]
pub async fn install_runtime(
    app: AppHandle,
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    gpu: State<'_, GpuCache>,
    package: releases::RuntimePackage,
) -> CmdResult<String> {
    *gpu.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let data_paths = paths.paths.clone();
    with_store(&store, move |s| -> diarist_core::Result<String> {
        let sha_url = package.sha_url.clone().ok_or_else(|| {
            CoreError::Models(format!(
                "{} has no published SHA-256; download it by hand from {} and put the zip into the \
                 engine folder",
                package.name,
                releases::RELEASES_PAGE
            ))
        })?;
        let sha = releases::fetch_sha(&sha_url)?;
        let entry = releases::package_entry(&package, sha);
        let manifest = Manifest::load(&data_paths.root)?;
        run_engine_install(&app, s, &data_paths, &manifest, &entry)
    })
    .await
}

/// "Add from URL": downloads a model no manifest lists into `models\user\`
/// (resumable; checked against `sha256` when given), then rescans. Progress
/// arrives as `models:progress` under the file name. Returns the file.
#[tauri::command(rename_all = "snake_case")]
pub async fn add_model_url(
    app: AppHandle,
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    url: String,
    sha256: Option<String>,
) -> CmdResult<String> {
    let data_paths = paths.paths.clone();
    with_store(&store, move |s| -> diarist_core::Result<String> {
        let name = models::add::file_name_from_url(url.trim())?;
        let mut last = Instant::now() - Duration::from_secs(1);
        let result = models::add::add_from_url(
            &data_paths,
            url.trim(),
            sha256.as_deref(),
            &mut |done, total| {
                if last.elapsed() >= Duration::from_millis(250) {
                    last = Instant::now();
                    emit_progress(&app, DownloadKind::Model, &name, done, total, false, None);
                }
            },
        )
        .and_then(|p| {
            let manifest = Manifest::load(&data_paths.root)?;
            models::scan(&data_paths, s, &manifest)?;
            Ok(p)
        });
        match &result {
            Ok(p) => {
                log::info!("added {} from {url}", p.display());
                let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                emit_progress(
                    &app,
                    DownloadKind::Model,
                    &name,
                    size,
                    Some(size),
                    true,
                    None,
                );
            }
            Err(e) => {
                log::error!("adding {url} failed: {e}");
                emit_progress(
                    &app,
                    DownloadKind::Model,
                    &name,
                    0,
                    None,
                    true,
                    Some(e.to_string()),
                );
            }
        }
        note_download(
            &app,
            &name,
            "diarist.exe",
            &result.as_ref().map(|_| ()).map_err(|e| e.to_string()),
        );
        result.map(|p| p.display().to_string())
    })
    .await
}

/// "Add from file": copies (or with `link`, hard-links) a model file into
/// `models\user\`, then rescans. Returns the new file.
#[tauri::command(rename_all = "snake_case")]
pub async fn add_model_file(
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    path: String,
    link: bool,
) -> CmdResult<String> {
    let data_paths = paths.paths.clone();
    with_store(&store, move |s| -> diarist_core::Result<String> {
        let dest = models::add::add_from_file(&data_paths, Path::new(&path), link)?;
        let manifest = Manifest::load(&data_paths.root)?;
        models::scan(&data_paths, s, &manifest)?;
        log::info!(
            "added {} ({}) from {path}",
            dest.display(),
            if link { "linked" } else { "copied" }
        );
        Ok(dest.display().to_string())
    })
    .await
}

/// Whether Import or Record (`action`) can run now; each missing part comes
/// with its fix (`diarist_core::models::readiness`). `language` (imports:
/// default `default_language`) adds a `language` part with the Nemotron 3.5
/// download when neither the selected model nor an installed Nemotron 3.5
/// covers it (`models::select_for_language`).
#[tauri::command(rename_all = "snake_case")]
pub async fn check_ready(
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    action: String,
    language: Option<String>,
) -> CmdResult<models::Readiness> {
    let data_paths = paths.paths.clone();
    let demo = paths.demo;
    with_store(
        &store,
        move |s| -> diarist_core::Result<models::Readiness> {
            let manifest = Manifest::load(&data_paths.root)?;
            if demo {
                return Ok(models::Readiness {
                    action,
                    ok: true,
                    missing: Vec::new(),
                });
            }
            // Only when something is not selected: a scan picks up files dropped in.
            if models::resolve_selection(&data_paths, s).is_err() {
                install_dropped(&data_paths, &manifest);
                models::scan(&data_paths, s, &manifest)?;
            }
            let (gpu, adapters) = (gpu::gpu_info(0), gpu::adapters());
            let mut ready =
                models::readiness(&data_paths, s, &manifest, gpu.as_ref(), &adapters, &action)?;
            let language = language
                .map(|l| l.trim().to_owned())
                .filter(|l| !l.is_empty())
                .or_else(|| {
                    (action == "import").then(|| {
                        settings::default_language(&settings::effective(s).unwrap_or_default())
                    })
                });
            let asr = models::selected_file(s, ModelRole::Asr)?;
            if let (Some(language), Some(asr)) = (language, asr) {
                let files = s.list_model_files()?;
                if let Err(need) = models::select_for_language(&asr, &files, &language, false) {
                    let backend = gpu::recommended_backend(gpu.as_ref(), &adapters);
                    let item = models::download_items(&data_paths, &manifest, &files, backend)
                        .into_iter()
                        .find(|i| i.name == need.install && !i.installed);
                    let fix = if action == "import" {
                        "Download it here, or choose another default language in Settings."
                    } else {
                        "Download it here, or choose another language."
                    };
                    ready.ok = false;
                    ready.missing.push(models::MissingPart {
                        what: "language".into(),
                        message: format!(
                            "{} needs Nemotron 3.5: {}. {fix}",
                            need.what(),
                            need.reason
                        ),
                        item,
                    });
                }
            }
            Ok(ready)
        },
    )
    .await
}

/// Every manifest download (engine first, then required, optional and
/// experimental models) with whether it is installed, its URL and hash.
#[tauri::command]
pub async fn recommended_downloads(
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
) -> CmdResult<Vec<DownloadItem>> {
    let data_paths = paths.paths.clone();
    with_store(
        &store,
        move |s| -> diarist_core::Result<Vec<DownloadItem>> {
            let manifest = Manifest::load(&data_paths.root)?;
            Ok(models::download_items(
                &data_paths,
                &manifest,
                &s.list_model_files()?,
                recommended_backend_now(),
            ))
        },
    )
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

/// The last `max_lines` lines of the newest app log file (`<data root>ogs`).
#[tauri::command(rename_all = "snake_case")]
pub async fn read_log_tail(paths: State<'_, AppPaths>, max_lines: u32) -> CmdResult<String> {
    let dir = paths.paths.logs();
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

// ---- First run ---------------------------------------------------------------------

/// The first-run state: scans `models\` and `engine\`, then reports what is
/// present, what the recommended downloads are (with URLs and hashes), the
/// GPU (NVML) and the free disk space.
#[tauri::command]
pub async fn setup_status(
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    gpu: State<'_, GpuCache>,
) -> CmdResult<models::SetupStatus> {
    let data_paths = paths.paths.clone();
    let demo = paths.demo;
    if !demo {
        *gpu.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    with_store(
        &store,
        move |s| -> diarist_core::Result<models::SetupStatus> {
            let manifest = Manifest::load(&data_paths.root)?;
            let files = if demo {
                s.list_model_files()?
            } else {
                install_dropped(&data_paths, &manifest);
                let report = models::scan(&data_paths, s, &manifest)?;
                for e in &report.errors {
                    log::warn!("model scan: {e}");
                }
                report.files
            };
            Ok(models::setup_status(
                &data_paths,
                &manifest,
                &files,
                gpu::gpu_info(0),
                gpu::adapters(),
            ))
        },
    )
    .await
}

#[tauri::command]
pub async fn open_engine_folder(paths: State<'_, AppPaths>) -> CmdResult<()> {
    open_folder(&paths.paths.engine())
}

/// Runtime zips put into `engine` by hand are checked and extracted.
fn install_dropped(paths: &DataPaths, manifest: &Manifest) {
    for r in models::download::install_dropped_engines(paths, manifest) {
        match r {
            Ok(dir) => log::info!("installed a runtime zip put in by hand: {}", dir.display()),
            Err(e) => log::warn!("runtime zip put in by hand not installed: {e}"),
        }
    }
}

/// Opens the folder a download item goes to, so a file fetched by hand can
/// be dropped there: `engine` for a runtime zip, the model's pull folder
/// for a model.
#[tauri::command(rename_all = "snake_case")]
pub async fn open_download_folder(paths: State<'_, AppPaths>, name: String) -> CmdResult<()> {
    let manifest = Manifest::load(&paths.paths.root).map_err(|e| e.to_string())?;
    if manifest.engines.iter().any(|e| e.name == name) {
        return open_folder(&paths.paths.engine());
    }
    let entry = manifest
        .model(&name)
        .ok_or_else(|| format!("no download named {name:?}"))?;
    open_folder(&models::pull_dir(&paths.paths.models(), entry))
}

#[tauri::command]
pub async fn open_log_folder(paths: State<'_, AppPaths>) -> CmdResult<()> {
    open_folder(&paths.paths.logs())
}

// ---- Data folder ---------------------------------------------------------------------

#[tauri::command]
pub async fn data_root_info(paths: State<'_, AppPaths>) -> CmdResult<DataRootInfo> {
    let mut info = paths.paths.info();
    if paths.demo {
        info.move_blocked = Some("Not available in the demo.".into());
    }
    Ok(info)
}

/// "Move data folder": copies the database (backup API), audio, engine and
/// models to `to`, verifies them, points the registry at it and returns;
/// the UI then restarts the app, and the next launch deletes the old copy
/// (`diarist_core::models::relocate`). Progress arrives as `data:move_progress`.
#[tauri::command(rename_all = "snake_case")]
pub async fn move_data_root(
    app: AppHandle,
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
    jobs: State<'_, Jobs>,
    to: String,
) -> CmdResult<MoveResult> {
    if paths.demo {
        return Err("Not available in the demo.".into());
    }
    if let Some(why) = paths.paths.info().move_blocked {
        return Err(why);
    }
    if crate::live::handle(&app).is_recording() {
        return Err("Stop the recording first.".into());
    }
    if jobs.0.running().is_some() || !jobs.0.queued().is_empty() {
        return Err("Wait until the queued transcriptions have finished.".into());
    }
    let to = PathBuf::from(to.trim());
    let from = paths.paths.root.clone();
    let store = Arc::clone(&store.0);
    blocking(move || {
        let plan = relocate::plan_move(&from, &to).map_err(|e| e.to_string())?;
        log::info!(
            "moving the data folder {} -> {}: {} files, {} MB",
            from.display(),
            to.display(),
            plan.files,
            plan.bytes >> 20
        );
        // Nothing may hold engine files or the GPU while copying.
        crate::live::handle(&app).stop_server();
        let backup = |dest: &Path| -> diarist_core::Result<()> {
            store.backup(dest)?;
            let copy = diarist_core::store::SqliteStore::open(dest)?;
            let (a, b) = (store.list_meetings()?.len(), copy.list_meetings()?.len());
            if a != b {
                return Err(CoreError::Models(format!(
                    "the database copy has {b} meetings, the original {a}"
                )));
            }
            Ok(())
        };
        let report =
            relocate::copy_data(&plan, &backup, &mut move_progress(&app)).map_err(|e| {
                log::error!("moving the data folder failed: {e}");
                e.to_string()
            })?;
        // Back to the default folder: clear the key instead of naming it.
        let is_default = DataPaths::default_root().is_ok_and(|d| {
            d.to_string_lossy()
                .eq_ignore_ascii_case(&to.to_string_lossy())
        });
        models::paths::set_registry_root(if is_default { None } else { Some(&to) })
            .map_err(|e| e.to_string())?;
        log::info!(
            "data folder copied to {} ({} files, {} MB, {} links); restart to use it",
            to.display(),
            report.files,
            report.bytes >> 20,
            report.links
        );
        Ok(MoveResult {
            to: to.display().to_string(),
            files: report.files,
            bytes: report.bytes,
        })
    })
    .await
}

/// Emits `data:move_progress` at most every 200 ms (and at the end).
fn move_progress(app: &AppHandle) -> impl FnMut(u64, u64) + '_ {
    let mut last = Instant::now() - Duration::from_secs(1);
    move |done, total| {
        if last.elapsed() >= Duration::from_millis(200) || done == total {
            last = Instant::now();
            let _ = app.emit(
                MoveProgress::EVENT,
                MoveProgress {
                    done_bytes: done,
                    total_bytes: total,
                },
            );
        }
    }
}

/// First launch after the rename, "Move": copies the pre-rename data root
/// (`diarist_core::models::legacy`) into this one with the data-folder move
/// and verifies it; the UI then restarts the app, and the next launch
/// deletes the old copy. Progress arrives as `data:move_progress`.
#[tauri::command]
pub async fn migrate_legacy_data(
    app: AppHandle,
    paths: State<'_, AppPaths>,
    legacy: State<'_, Legacy>,
) -> CmdResult<MoveResult> {
    let Some(old) = legacy.0.clone() else {
        return Err("There is no earlier data to move.".into());
    };
    let to = paths.paths.root.clone();
    blocking(move || {
        let plan = legacy::plan_legacy_move(&old.root, &to).map_err(|e| e.to_string())?;
        log::info!(
            "moving pre-rename data {} -> {}: {} files, {} MB",
            old.root.display(),
            to.display(),
            plan.files,
            plan.bytes >> 20
        );
        crate::live::handle(&app).stop_server();
        let src = old.root.join(legacy::LEGACY_DB_FILE);
        let backup = |dest: &Path| legacy::copy_legacy_database(&src, dest);
        let report =
            relocate::copy_data(&plan, &backup, &mut move_progress(&app)).map_err(|e| {
                log::error!("moving the pre-rename data failed: {e}");
                e.to_string()
            })?;
        log::info!(
            "pre-rename data copied to {} ({} files, {} MB, {} links); restart to use it",
            to.display(),
            report.files,
            report.bytes >> 20,
            report.links
        );
        Ok(MoveResult {
            to: to.display().to_string(),
            files: report.files,
            bytes: report.bytes,
        })
    })
    .await
}

/// First launch after the rename, "Start fresh": creates the new database
/// (so the question is not asked again) and leaves the old folder alone;
/// the UI then restarts the app.
#[tauri::command]
pub async fn start_fresh(paths: State<'_, AppPaths>, legacy: State<'_, Legacy>) -> CmdResult<()> {
    let Some(old) = legacy.0.clone() else {
        return Ok(());
    };
    let db = paths.paths.db();
    blocking(move || {
        diarist_core::store::SqliteStore::open(&db).map_err(|e| e.to_string())?;
        log::info!(
            "starting fresh in {}; the earlier data in {} is left as it is",
            db.display(),
            old.root.display()
        );
        Ok(())
    })
    .await
}

/// Restarts the app (after a data-folder move or an update).
#[tauri::command]
pub async fn restart_app(app: AppHandle) -> CmdResult<()> {
    log::info!("restarting");
    // The new process must not find this one's single-instance lock.
    tauri_plugin_single_instance::destroy(&app);
    app.request_restart();
    Ok(())
}

// ---- Retention ------------------------------------------------------------------------

/// What retention would delete now (Settings: reclaimable space).
#[tauri::command]
pub async fn retention_preview(
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
) -> CmdResult<RetentionPlan> {
    if paths.demo {
        return Ok(RetentionPlan::default());
    }
    let root = paths.paths.root.clone();
    with_store(&store, move |s| {
        retention::plan_from_settings(s, &root, now_ms())
    })
    .await
}

/// Runs retention now (it also runs at launch and daily).
#[tauri::command]
pub async fn apply_retention(app: AppHandle) -> CmdResult<RetentionReport> {
    blocking(move || crate::maintenance::run_retention(&app)).await
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---- Autostart and updates -------------------------------------------------------------

#[tauri::command]
pub async fn get_autostart(app: AppHandle) -> CmdResult<bool> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

/// Starts Diarist with Windows (in the tray) or stops doing so.
#[tauri::command(rename_all = "snake_case")]
pub async fn set_autostart(app: AppHandle, enabled: bool) -> CmdResult<bool> {
    use tauri_plugin_autostart::ManagerExt;
    let al = app.autolaunch();
    if enabled { al.enable() } else { al.disable() }.map_err(|e| e.to_string())?;
    log::info!("start with Windows: {enabled}");
    al.is_enabled().map_err(|e| e.to_string())
}

/// The update endpoint in `tauri.conf.json` is a real URL, not the
/// placeholder a fork has to fill in.
fn updater_configured(app: &AppHandle) -> bool {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("endpoints"))
        .and_then(|e| e.as_array())
        .is_some_and(|eps| {
            !eps.is_empty()
                && eps
                    .iter()
                    .all(|e| e.as_str().is_some_and(|s| !s.contains("REPLACE_WITH")))
        })
}

/// "Check for updates" (manual only; the app never checks by itself).
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> CmdResult<UpdateCheck> {
    use tauri_plugin_updater::UpdaterExt;
    let current = app.package_info().version.to_string();
    if !updater_configured(&app) {
        return Ok(UpdateCheck {
            configured: false,
            current,
            available: None,
            notes: None,
        });
    }
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| format!("Update check failed: {e}"))?;
    log::info!(
        "update check: {}",
        update.as_ref().map_or("up to date".to_owned(), |u| format!(
            "{} available",
            u.version
        ))
    );
    Ok(UpdateCheck {
        configured: true,
        current,
        available: update.as_ref().map(|u| u.version.clone()),
        notes: update.and_then(|u| u.body),
    })
}

/// Downloads and installs the available update (signature-checked by the
/// updater), then restarts.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> CmdResult<()> {
    use tauri_plugin_updater::UpdaterExt;
    if crate::live::handle(&app).is_recording() {
        return Err("Stop the recording before updating.".into());
    }
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No update is available.".to_owned())?;
    log::info!("installing update {}", update.version);
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| format!("Update failed: {e}"))?;
    tauri_plugin_single_instance::destroy(&app);
    app.request_restart();
    Ok(())
}

// ---- Diagnostics: copy and startup report -----------------------------------------------

/// "Copy diagnostics": versions, OS, GPU and driver, data root, the selected
/// engine and models with hashes, the live server, and the tail of the last
/// engine stderr. No transcript text, titles or audio.
#[tauri::command]
pub async fn diagnostics_report(
    app: AppHandle,
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
) -> CmdResult<String> {
    use tauri_plugin_autostart::ManagerExt;
    let data_paths = paths.paths.clone();
    let live = crate::live::status(&app);
    let autostart = app.autolaunch().is_enabled().unwrap_or(false);
    let server = match &live.server {
        Some(s) => format!(
            "{:?}, {} restarts{}",
            s.server.status,
            s.server.restarts,
            s.resident_vram_mb
                .map_or(String::new(), |m| format!(", holds {m} MB"))
        ),
        None => "not started".into(),
    };
    with_store(&store, move |s| -> diarist_core::Result<String> {
        let stored = settings::effective(s)?;
        let engine_stderr = pipeline::last_engine_log(&data_paths.audio(), ENGINE_LOG_TAIL)
            .map(|(p, tail)| format!("{}\n{tail}", p.display()))
            .or_else(|| {
                let p = data_paths.logs().join("serve.log");
                let bytes = std::fs::read(&p).ok()?;
                let text = String::from_utf8_lossy(&bytes).into_owned();
                Some(format!("{}\n{text}", p.display()))
            });
        let input = DiagnosticsInput {
            app: format!("Diarist {}", diarist_core::VERSION),
            os: diagnostics::os_version(),
            gpu: gpu::gpu_info(0),
            extra: vec![
                ("live server".into(), server),
                (
                    "settings".into(),
                    format!(
                        "preload {}, idle stop {} min, low-VRAM {}, retention {} days",
                        settings::get(&stored, settings::ENGINE_PRELOAD),
                        settings::get(&stored, settings::ENGINE_IDLE_MINUTES),
                        settings::get(&stored, settings::ENGINE_LOW_VRAM),
                        settings::get(&stored, settings::AUDIO_RETENTION_DAYS),
                    ),
                ),
                ("start with Windows".into(), autostart.to_string()),
                ("app log".into(), data_paths.logs().display().to_string()),
                ("meetings".into(), s.list_meetings()?.len().to_string()),
            ],
            engine_stderr,
            stderr_lines: 40,
            checks: health_checks(&app, s, &data_paths),
        };
        Ok(diagnostics::report(&data_paths, s, &input))
    })
    .await
}

/// What launch recovered (and whether a data-folder move finished).
#[tauri::command]
pub async fn startup_report(startup: State<'_, Startup>) -> CmdResult<StartupReport> {
    Ok(startup.0.lock().unwrap_or_else(|e| e.into_inner()).clone())
}

// ---- "Why isn't it working?" ------------------------------------------------------------

/// The download hosts a firewall must allow, by the process that connects.
fn hosts_for(process: &str) -> &'static str {
    if process.starts_with("curl") {
        "huggingface.co and its download servers (*.hf.co, for example us.aws.cdn.hf.co)"
    } else {
        "github.com, objects.githubusercontent.com and huggingface.co"
    }
}

/// An error that looks like the network, a firewall or a proxy, not the file.
fn connection_error(e: &str) -> bool {
    let e = e.to_ascii_lowercase();
    [
        "10013",
        "10060",
        "10061",
        "10065",
        "11001",
        "no such host",
        "dns",
        "resolve",
        "connect",
        "timed out",
        "timeout",
        "revocation",
        "certificate",
        "stalled",
        "network",
        "unreachable",
    ]
    .iter()
    .any(|k| e.contains(k))
}

/// The checklist (Diagnostics, Copy diagnostics). Runs the engine's
/// `doctor` (a second or two) and lists audio devices.
fn health_checks(app: &AppHandle, store: &dyn RunStore, paths: &DataPaths) -> Vec<HealthCheck> {
    use diarist_core::types::ModelFile;
    let mut out = Vec::new();
    let manifest = Manifest::load(&paths.root).ok();
    let gpu_info = gpu::gpu_info(0);
    let adapters = gpu::adapters();

    // 1. Runtime present and runnable.
    let engine = models::selected_file(store, ModelRole::Engine)
        .ok()
        .flatten();
    let doctor = engine
        .as_ref()
        .map(|e| models::doctor(&paths.absolute(&e.path)));
    out.push(match (&engine, &doctor) {
        (None, _) => HealthCheck::new(
            "runtime",
            "Speech engine",
            "fail",
            "Not installed. Install it, or put an extracted runtime into the engine folder and \
             press Refresh.",
        )
        .fix("setup", "Install engine"),
        (Some(e), Some(Ok(r))) => HealthCheck::new(
            "runtime",
            "Speech engine",
            "pass",
            format!(
                "{} runs (nemo-speech {} answered doctor)",
                e.display_name, r.version
            ),
        ),
        (Some(e), Some(Err(err))) => HealthCheck::new(
            "runtime",
            "Speech engine",
            "fail",
            format!(
                "{} does not run: {err}. Windows Smart App Control or an antivirus may block the \
                 unsigned runtime; reinstall it or allow nemo-speech.exe.",
                e.display_name
            ),
        )
        .fix("open_engine", "Open engine folder"),
        (Some(_), None) => unreachable!(),
    });

    // 2. GPU and driver.
    let backend = engine
        .as_ref()
        .and_then(|e| e.quant.clone())
        .unwrap_or_else(|| gpu::recommended_backend(gpu_info.as_ref(), &adapters).to_owned());
    out.push(match &gpu_info {
        Some(g) if g.cuda_ok => HealthCheck::new(
            "gpu",
            "GPU and driver",
            "pass",
            format!(
                "{}, {} MB, driver {}, CUDA {}",
                g.name,
                g.vram_total_mb,
                g.driver_version,
                g.cuda_version.as_deref().unwrap_or("?")
            ),
        ),
        Some(g) => HealthCheck::new(
            "gpu",
            "GPU and driver",
            if backend == "cuda" { "fail" } else { "warn" },
            format!(
                "{} with driver {} supports CUDA {}; the CUDA engine needs CUDA 13 (R580 or newer). \
                 Update the driver from nvidia.com, or install the Vulkan runtime.",
                g.name,
                g.driver_version,
                g.cuda_version.as_deref().unwrap_or("?")
            ),
        )
        .fix("setup", "Choose a runtime"),
        None if !adapters.is_empty() => HealthCheck::new(
            "gpu",
            "GPU and driver",
            if backend == "cuda" { "fail" } else { "warn" },
            format!(
                "No NVIDIA GPU; found {}. Use the Vulkan runtime (untested by us) or the CPU \
                 runtime (slow).",
                adapters[0].name
            ),
        )
        .fix("setup", "Choose a runtime"),
        None => HealthCheck::new(
            "gpu",
            "GPU and driver",
            "warn",
            "No GPU found. Only the CPU runtime works, much slower than real time.",
        )
        .fix("setup", "Choose a runtime"),
    });

    // 3 and 4. Selected models present, hashes as published.
    let model_check = |role: ModelRole, id: &str, label: &str| -> HealthCheck {
        let required = manifest
            .as_ref()
            .and_then(|m| m.models.iter().find(|e| e.required && e.role == role))
            .map(|e| e.name.clone());
        match models::selected_file(store, role).ok().flatten() {
            None => {
                let c = HealthCheck::new(
                    id,
                    label,
                    "fail",
                    "None selected or the file is gone. Download the recommended one, or put a \
                     .gguf into the models folder and press Refresh.",
                );
                match required {
                    Some(n) => c.fix(format!("download:{n}"), "Download"),
                    None => c.fix("open_models", "Open models folder"),
                }
            }
            Some(f) => file_check(&f, manifest.as_ref(), id, label),
        }
    };
    fn file_check(
        f: &ModelFile,
        manifest: Option<&Manifest>,
        id: &str,
        label: &str,
    ) -> HealthCheck {
        let published = manifest
            .zip(f.manifest_name.as_deref())
            .and_then(|(m, n)| m.model(n))
            .map(|e| e.sha256.clone());
        match (published, &f.sha256) {
            (Some(want), Some(got)) if want.eq_ignore_ascii_case(got) => HealthCheck::new(
                id,
                label,
                "pass",
                format!("{}: SHA-256 matches the published one", f.display_name),
            ),
            (Some(_), _) => HealthCheck::new(
                id,
                label,
                "fail",
                format!(
                    "{}: the SHA-256 does not match the published one; delete it and download it again",
                    f.display_name
                ),
            )
            .fix("open_models", "Open models folder"),
            (None, _) if f.role == ModelRole::Unknown => HealthCheck::new(
                id,
                label,
                "warn",
                format!(
                    "{}: not recognised from its header; the runtime decides whether it can run it",
                    f.display_name
                ),
            ),
            (None, _) => HealthCheck::new(
                id,
                label,
                "pass",
                format!("{}: added by you (no published hash to compare)", f.display_name),
            ),
        }
    }
    out.push(model_check(ModelRole::Asr, "asr", "Speech-to-text model"));
    out.push(model_check(
        ModelRole::Diarizer,
        "diarizer",
        "Speaker model",
    ));

    // 5 and 6. Free VRAM for live and for file mode.
    match gpu::nvml_vram(0) {
        Some(r) => {
            out.push(if r.free_mb >= diarist_core::live::MIN_FREE_VRAM_MB {
                HealthCheck::new(
                    "vram_live",
                    "GPU memory for live",
                    "pass",
                    format!(
                        "{} MB free, live needs {} MB",
                        r.free_mb,
                        diarist_core::live::MIN_FREE_VRAM_MB
                    ),
                )
            } else {
                HealthCheck::new(
                    "vram_live",
                    "GPU memory for live",
                    "warn",
                    format!(
                        "{} MB free, live needs {} MB. Close GPU-heavy apps (games, video editors, \
                         other AI tools) before recording.",
                        r.free_mb,
                        diarist_core::live::MIN_FREE_VRAM_MB
                    ),
                )
            });
            let need = models::selected_file(store, ModelRole::Asr)
                .ok()
                .flatten()
                .and_then(|f| {
                    f.capabilities
                        .vram_mb_measured
                        .or(f.capabilities.vram_mb_estimate)
                });
            out.push(match need {
                Some(n) if r.free_mb >= n => HealthCheck::new(
                    "vram_file",
                    "GPU memory for files",
                    "pass",
                    format!("{} MB free, the selected model peaks at about {n} MB", r.free_mb),
                ),
                Some(n) => HealthCheck::new(
                    "vram_file",
                    "GPU memory for files",
                    "warn",
                    format!(
                        "{} MB free, the selected model peaks at about {n} MB. Close GPU-heavy apps \
                         or turn on low-VRAM mode.",
                        r.free_mb
                    ),
                )
                .fix("settings", "Low-VRAM mode"),
                None => HealthCheck::new(
                    "vram_file",
                    "GPU memory for files",
                    "skip",
                    format!("{} MB free; the selected model's need is unknown", r.free_mb),
                ),
            });
        }
        None => out.push(HealthCheck::new(
            "vram_live",
            "GPU memory",
            "skip",
            "Free GPU memory is only known on NVIDIA GPUs.",
        )),
    }

    // 7. Free disk.
    out.push(match models::free_disk_bytes(&paths.root) {
        Some(free) if free >= pipeline::MIN_FREE_BYTES => HealthCheck::new(
            "disk",
            "Free disk",
            "pass",
            format!("{} MB free in {}", free >> 20, paths.root.display()),
        ),
        Some(free) => HealthCheck::new(
            "disk",
            "Free disk",
            "fail",
            format!(
                "Only {} MB free in {}; recording and import need {} MB. Free up old audio or move \
                 the data folder in Settings.",
                free >> 20,
                paths.root.display(),
                pipeline::MIN_FREE_BYTES >> 20
            ),
        )
        .fix("settings", "Open Settings"),
        None => HealthCheck::new("disk", "Free disk", "skip", "Free space unknown."),
    });

    // 8. Audio devices.
    out.push(match audio::capture::list_devices() {
        Ok(devs) => {
            use audio::capture::DeviceFlow;
            let mics = devs.iter().filter(|d| d.flow == DeviceFlow::Capture).count();
            let outs = devs.iter().filter(|d| d.flow == DeviceFlow::Render).count();
            if mics > 0 && outs > 0 {
                HealthCheck::new(
                    "audio",
                    "Microphone and system audio",
                    "pass",
                    format!("{mics} microphones, {outs} outputs to record from"),
                )
            } else {
                HealthCheck::new(
                    "audio",
                    "Microphone and system audio",
                    "fail",
                    format!(
                        "{mics} microphones and {outs} outputs found. Connect or enable a device in \
                         the Windows sound settings, then check again."
                    ),
                )
                .fix("sound_settings", "Sound settings")
            }
        }
        Err(e) => HealthCheck::new(
            "audio",
            "Microphone and system audio",
            "fail",
            format!("Cannot list audio devices: {e}. Open the Windows sound settings and check again."),
        )
        .fix("sound_settings", "Sound settings"),
    });

    // 9. Live server reachable when it should be.
    let live = crate::live::status(app);
    let ready = live
        .server
        .as_ref()
        .is_some_and(|s| matches!(s.server.status, ServeStatus::Ready { .. }));
    let should = live.state.phase != diarist_core::live::LivePhase::Idle
        || settings::effective(store)
            .map(|s| settings::get_bool(&s, settings::ENGINE_PRELOAD))
            .unwrap_or(false);
    out.push(match (should, ready, &live.server) {
        (_, true, _) => HealthCheck::new("server", "Live engine", "pass", "Running and answering"),
        (true, false, Some(s)) => HealthCheck::new(
            "server",
            "Live engine",
            "fail",
            format!(
                "Should be running but is {:?}. Its log is in {}; start it again.",
                s.server.status,
                s.log_path.as_deref().unwrap_or("the logs folder")
            ),
        )
        .fix("start_engine", "Start engine"),
        (true, false, None) => HealthCheck::new(
            "server",
            "Live engine",
            "warn",
            "Not started yet; start it to check that it loads.",
        )
        .fix("start_engine", "Start engine"),
        (false, false, _) => HealthCheck::new(
            "server",
            "Live engine",
            "skip",
            "Starts when you open the record sheet; not needed until then.",
        ),
    });

    // 10. Downloads and the firewall.
    let last = app
        .state::<LastDownloadError>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    out.push(match last {
        Some((what, process, err)) if connection_error(&err) => HealthCheck::new(
            "network",
            "Downloads",
            "fail",
            format!(
                "Downloading {what} failed: {err}. If a firewall such as simplewall asked about \
                 {process}, allow it (it connects to {}) and press Retry; or use Copy link to \
                 fetch the file with a browser and put it into the folder Open folder shows.",
                hosts_for(&process)
            ),
        )
        .fix("setup", "Open downloads"),
        Some((what, _, err)) => HealthCheck::new(
            "network",
            "Downloads",
            "warn",
            format!("Downloading {what} failed: {err}. Retry, or fetch it by hand with Copy link."),
        )
        .fix("setup", "Open downloads"),
        None => HealthCheck::new(
            "network",
            "Downloads",
            "pass",
            "No failed download in this session.",
        ),
    });
    out
}

/// "Why isn't it working?": the checklist with a fix per failing row.
#[tauri::command]
pub async fn health_check(
    app: AppHandle,
    store: State<'_, AppStore>,
    paths: State<'_, AppPaths>,
) -> CmdResult<Vec<HealthCheck>> {
    let data_paths = paths.paths.clone();
    let store = Arc::clone(&store.0);
    blocking(move || Ok(health_checks(&app, &*store, &data_paths))).await
}

/// Opens the Windows sound settings (a fix in the checklist).
#[tauri::command]
pub async fn open_sound_settings() -> CmdResult<()> {
    std::process::Command::new("explorer")
        .arg("ms-settings:sound")
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Cannot open the sound settings ({e}); open Settings, System, Sound"))
}
