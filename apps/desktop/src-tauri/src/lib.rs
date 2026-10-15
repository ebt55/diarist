//! Tauri shell for meet-assist. Only command wrappers and event plumbing live
//! here; logic belongs in `meet-core` (docs/01-architecture.md). Commands call
//! [`RunStore`] methods (and `meet_core` functions) on the blocking pool and
//! map errors to strings.
//!
//! The data root comes from `meet_core::models::paths` (`MEET_ASSIST_DATA_DIR`,
//! `portable.txt`, `HKCU\Software\meet-assist\DataRoot`, then
//! `app_local_data_dir`); the store is `SqliteStore` there. `--demo` runs on
//! the seeded `MemoryStore::demo()` with a scratch data root instead (for
//! screenshots; see `demo.rs`).
//!
//! Live recording (`live.rs`): one recorder at a time, a tray icon with a
//! recording indicator (`tray.rs`) and the global shortcut Ctrl+Alt+R that
//! toggles recording from anywhere. Closing the window while recording hides
//! it to the tray; quitting while recording leaves the meeting for the next
//! launch's crash recovery.

mod asset;
mod commands;
mod demo;
pub mod events;
mod live;
mod tray;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use meet_core::models::{DataPaths, DataRootSource, DoctorReport};
use meet_core::pipeline::JobQueue;
use meet_core::store::{MemoryStore, RunStore, SqliteStore};
use tauri::{Manager, RunEvent, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_log::{RotationStrategy, TimezoneStrategy};

/// If the frontend never calls `show()` (crashed bundle, failed load), reveal
/// the window anyway so the app is not an invisible process.
const SHOW_FALLBACK: Duration = Duration::from_secs(3);

/// The store behind every command.
pub(crate) struct AppStore(pub(crate) Arc<dyn RunStore>);

/// One GPU job at a time (imports and post-passes), process-wide; the
/// recorder pauses it while recording.
pub(crate) struct Jobs(pub(crate) Arc<JobQueue>);

/// Resolved locations under the data root.
pub(crate) struct AppPaths {
    pub(crate) paths: DataPaths,
    /// Running on `MemoryStore::demo()`.
    pub(crate) demo: bool,
}

/// `nemo-speech doctor` takes a second or two (CUDA init): its report is
/// reused for a short while by `engine_status`.
#[derive(Default)]
pub(crate) struct GpuCache(pub(crate) Arc<Mutex<DoctorSlot>>);

/// When the report was taken, and the report (`None`: no engine or doctor failed).
pub(crate) type DoctorSlot = Option<(Instant, Option<DoctorReport>)>;

/// The data root, the store on it, and whether this is the demo.
fn open_store(app: &tauri::App, demo: bool) -> Result<(DataPaths, Arc<dyn RunStore>), String> {
    if demo {
        let paths = DataPaths::at(std::env::temp_dir().join("meet-assist-demo"));
        return Ok((paths, Arc::new(MemoryStore::demo())));
    }
    let mut paths = DataPaths::resolve().map_err(|e| e.to_string())?;
    if paths.source == DataRootSource::Default {
        // Same folder as `resolve` computes, but from Tauri's own path API so
        // the installer, the asset scope and the store always agree.
        if let Ok(dir) = app.path().app_local_data_dir() {
            paths = DataPaths {
                source: DataRootSource::Default,
                ..DataPaths::at(dir)
            };
        }
    }
    std::fs::create_dir_all(&paths.root)
        .map_err(|e| format!("cannot create {}: {e}", paths.root.display()))?;
    let store = SqliteStore::open(paths.db())
        .map_err(|e| format!("cannot open {}: {e}", paths.db().display()))?
        .with_audio_root(paths.audio());
    Ok((paths, Arc::new(store)))
}

pub fn run() {
    let demo = std::env::args().any(|a| a == "--demo");

    let app = tauri::Builder::default()
        .manage(Jobs(Arc::new(JobQueue::new())))
        .manage(GpuCache::default())
        .manage(live::LiveHandle(Arc::new(live::LiveState::new())))
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(tauri_plugin_log::log::LevelFilter::Info)
                .max_file_size(2_000_000)
                .rotation_strategy(RotationStrategy::KeepOne)
                .timezone_strategy(TimezoneStrategy::UseLocal)
                .build(),
        )
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        live::toggle(app, meet_core::live::ToggleVia::Shortcut);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::list_meetings,
            commands::get_meeting,
            commands::get_segments,
            commands::get_words,
            commands::search,
            commands::import_file,
            commands::cancel_job,
            commands::rename_speaker,
            commands::set_speaker_color,
            commands::merge_speakers,
            commands::unmerge_speaker,
            commands::mark_reviewed,
            commands::reassign_segment,
            commands::split_segment,
            commands::set_meeting_title,
            commands::set_audio_keep,
            commands::delete_meeting,
            commands::export_meeting,
            commands::export_meeting_to,
            commands::get_audio_url,
            commands::get_waveform,
            commands::get_settings,
            commands::set_setting,
            commands::remove_setting,
            commands::engine_status,
            commands::list_model_files,
            commands::select_model,
            commands::rescan_models,
            commands::install_engine,
            commands::pull_model,
            commands::open_models_folder,
            commands::open_data_folder,
            commands::read_log_tail,
            commands::last_engine_stderr,
            commands::ui_log,
            commands::recommended_downloads,
            live::list_audio_devices,
            live::prepare_recording,
            live::start_recording,
            live::stop_recording,
            live::recording_status,
            live::live_snapshot,
            live::rerun_post_pass,
        ])
        .setup(move |app| {
            let (paths, store) = open_store(app, demo)?;
            std::fs::create_dir_all(&paths.root)?;
            // Playback reads stored audio through the asset protocol; the
            // scope starts at the data root (imports add their file on demand).
            app.asset_protocol_scope()
                .allow_directory(&paths.root, true)?;
            if demo {
                demo::attach_demo_audio(store.as_ref(), &paths.root);
            }
            log::info!(
                "meet-assist {} started{}; data root {} ({:?})",
                meet_core::VERSION,
                if demo { " in demo mode" } else { "" },
                paths.root.display(),
                paths.source
            );
            app.manage(AppStore(store));
            app.manage(AppPaths { paths, demo });

            if let Err(e) = tray::build(app) {
                log::warn!("tray icon not available: {e}");
            }
            if !demo {
                let live = live::handle(app.handle());
                match app.global_shortcut().register(live::SHORTCUT) {
                    Ok(()) => log::info!("global shortcut {} toggles recording", live::SHORTCUT),
                    Err(e) => {
                        log::warn!("global shortcut {} not registered: {e}", live::SHORTCUT);
                        live.set_shortcut_error(Some(e.to_string()));
                    }
                }
            }
            // Crash recovery first, then the live server warms up.
            live::startup(app.handle());

            if let Some(window) = app.get_webview_window("main") {
                std::thread::spawn(move || {
                    std::thread::sleep(SHOW_FALLBACK);
                    if !window.is_visible().unwrap_or(true) {
                        log::warn!("frontend did not show the window; showing it");
                        let _ = window.show();
                    }
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Recording continues in the tray when the window is closed.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if live::handle(window.app_handle()).is_recording() {
                    api.prevent_close();
                    let _ = window.hide();
                    log::info!("window hidden to the tray while recording");
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building meet-assist");
    app.run(|app, event| {
        if let RunEvent::Exit = event {
            live::handle(app).shutdown();
        }
    });
}
