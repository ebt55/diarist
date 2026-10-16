//! Tauri shell for Diarist. Only command wrappers and event plumbing live
//! here; logic belongs in `diarist-core` (docs/01-architecture.md). Commands call
//! [`RunStore`] methods (and `diarist_core` functions) on the blocking pool and
//! map errors to strings.
//!
//! The data root comes from `diarist_core::models::paths` (`DIARIST_DATA_DIR`,
//! `portable.txt`, `HKCU\Software\Diarist\DataRoot`, then
//! `%LOCALAPPDATA%\app.diarist.desktop`, which is Tauri's
//! `app_local_data_dir`). It is resolved before the app is built, so the app
//! log (`<root>\logs`), the WebView2 profile (`<root>\EBWebView`) and the
//! window state follow it; the store is `SqliteStore` there. A data-folder
//! move left by the previous run is finished first (`relocate`). When the
//! new root is fresh and a data root from before the rename exists, the app
//! runs on an empty `MemoryStore` and asks once whether to move that data in
//! (`diarist_core::models::legacy`, the `migrate_legacy_data` and
//! `start_fresh` commands). `--demo`
//! runs on the seeded `MemoryStore::demo()` with a scratch data root
//! instead (for screenshots; see `demo.rs`).
//!
//! Live recording (`live.rs`): one recorder at a time, a tray icon with a
//! recording indicator (`tray.rs`) and the global shortcut Ctrl+Alt+R that
//! starts a recording from anywhere and, pressed again, asks to stop.
//! Closing the window while recording hides it to the tray; quitting while
//! recording leaves the meeting for the next launch's crash recovery.
//!
//! One instance at a time (a second launch focuses the first), optional
//! start with Windows (`--autostart`: the window stays in the tray), window
//! size and position remembered, and audio retention at launch and daily
//! (`maintenance.rs`).
//!
//! Crash capture (`crash.rs`, docs/plans/11-hardening.md 8a) is installed
//! before anything else: reports go to `<root>\logs`, the next launch shows
//! a notice, and WebView errors are forwarded to the app log.

mod asset;
mod commands;
mod crash;
mod demo;
pub mod events;
mod icons;
mod live;
mod maintenance;
mod tray;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use diarist_core::models::legacy::{self, LegacyRoot};
use diarist_core::models::relocate::{self, CleanupReport};
use diarist_core::models::{DataPaths, DoctorReport};
use diarist_core::pipeline::JobQueue;
use diarist_core::store::{MemoryStore, RunStore, SqliteStore};
use diarist_core::views::StartupReport;
use tauri::{Manager, RunEvent, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};
use tauri_plugin_window_state::StateFlags;

/// If the frontend never calls `show()` (crashed bundle, failed load), reveal
/// the window anyway so the app is not an invisible process.
const SHOW_FALLBACK: Duration = Duration::from_secs(3);

/// The argument Windows passes when it starts the app at sign-in.
pub(crate) const AUTOSTART_ARG: &str = "--autostart";

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

/// What launch found and fixed (`recovery_report`, `app:startup`).
#[derive(Default)]
pub(crate) struct Startup(pub(crate) Mutex<StartupReport>);

/// The pre-rename data root offered at this launch (the store is then an
/// empty `MemoryStore` until the user answers and the app restarts).
pub(crate) struct Legacy(pub(crate) Option<LegacyRoot>);

/// The data root for this run: the demo's scratch folder, or the resolved one.
fn resolve_paths(demo: bool) -> Result<DataPaths, String> {
    if demo {
        return Ok(DataPaths::at(std::env::temp_dir().join("diarist-demo")));
    }
    DataPaths::resolve().map_err(|e| e.to_string())
}

/// The store on the data root (or the demo's, or an empty one while the
/// move of a pre-rename data root is being offered: no database is created).
fn open_store(
    paths: &DataPaths,
    demo: bool,
    legacy: Option<&LegacyRoot>,
) -> Result<Arc<dyn RunStore>, String> {
    if demo {
        return Ok(Arc::new(MemoryStore::demo()));
    }
    if legacy.is_some() {
        return Ok(Arc::new(MemoryStore::new()));
    }
    let store = SqliteStore::open(paths.db())
        .map_err(|e| format!("cannot open {}: {e}", paths.db().display()))?
        .with_audio_root(paths.audio());
    if let Some(backup) = store.migration_backup() {
        log::info!(
            "database upgraded; the copy from before is {}",
            backup.display()
        );
    }
    Ok(Arc::new(store))
}

/// Shows a fatal start-up error where a user can see it, then exits.
fn fatal(message: &str) -> ! {
    eprintln!("Diarist cannot start: {message}");
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide = |s: &str| -> Vec<u16> {
            std::ffi::OsStr::new(s)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        };
        let (text, title) = (
            wide(&format!("Diarist cannot start:\n\n{message}")),
            wide("Diarist"),
        );
        #[link(name = "user32")]
        extern "system" {
            fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
        }
        unsafe {
            MessageBoxW(0, text.as_ptr(), title.as_ptr(), 0x10);
        }
    }
    std::process::exit(1);
}

pub fn run() {
    let demo = std::env::args().any(|a| a == "--demo");
    let autostart = std::env::args().any(|a| a == AUTOSTART_ARG);
    let paths = resolve_paths(demo).unwrap_or_else(|e| fatal(&e));
    if let Err(e) = std::fs::create_dir_all(paths.logs()) {
        fatal(&format!("cannot create {}: {e}", paths.logs().display()));
    }
    // First, so anything after this that crashes leaves a report.
    let crash_launch = crash::install(&paths);
    // The previous run moved the data folder here: delete the old copy now
    // that nothing holds its files open (logged in setup).
    let moved: Option<CleanupReport> = if demo {
        None
    } else {
        relocate::finish_pending_move(&paths.root)
    };
    // A moved-in pre-rename root: its registry value goes with its data.
    let legacy_key_removed = moved
        .as_ref()
        .map(legacy::forget_legacy_registry)
        .transpose();
    // First launch under the new name with old data around: ask first.
    let legacy: Option<LegacyRoot> = if demo {
        None
    } else {
        legacy::pending_legacy(&paths)
    };
    let window_state_file: PathBuf = paths.root.join(".window-state.json");
    // A saved size and position replace the first-launch sizing in the UI.
    let window_state_saved = window_state_file.is_file();

    let app = tauri::Builder::default()
        // First, so a second launch only focuses this one.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main(app);
        }))
        .manage(Jobs(Arc::new(JobQueue::new())))
        .manage(GpuCache::default())
        .manage(Startup::default())
        .manage(Legacy(legacy.clone()))
        .manage(commands::LastDownloadError::default())
        .manage(live::LiveHandle(Arc::new(live::LiveState::new())))
        .manage(crash::CrashNotice(Mutex::new(crash_launch.notice.clone())))
        .plugin(
            tauri_plugin_log::Builder::new()
                .clear_targets()
                .targets([
                    Target::new(TargetKind::Stdout),
                    Target::new(TargetKind::Folder {
                        path: paths.logs(),
                        file_name: Some("diarist".into()),
                    }),
                ])
                .level(tauri_plugin_log::log::LevelFilter::Info)
                .max_file_size(2_000_000)
                .rotation_strategy(RotationStrategy::KeepOne)
                .timezone_strategy(TimezoneStrategy::UseLocal)
                .build(),
        )
        .plugin(
            tauri_plugin_window_state::Builder::new()
                // Under the data root (an absolute name replaces the plugin's
                // folder); the window stays hidden until the first paint.
                .with_filename(window_state_file.display().to_string())
                .with_state_flags(StateFlags::all() - StateFlags::VISIBLE)
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![AUTOSTART_ARG]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        live::toggle(app, diarist_core::live::ToggleVia::Shortcut);
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
            commands::rename_speaker_at,
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
            commands::open_engine_folder,
            commands::open_download_folder,
            commands::open_data_folder,
            commands::open_log_folder,
            commands::read_log_tail,
            commands::last_engine_stderr,
            commands::ui_log,
            commands::recommended_downloads,
            commands::setup_status,
            commands::data_root_info,
            commands::move_data_root,
            commands::restart_app,
            commands::migrate_legacy_data,
            commands::start_fresh,
            commands::retention_preview,
            commands::apply_retention,
            commands::get_autostart,
            commands::set_autostart,
            commands::check_for_update,
            commands::install_update,
            commands::diagnostics_report,
            commands::startup_report,
            commands::runtime_releases,
            commands::install_runtime,
            commands::add_model_url,
            commands::add_model_file,
            commands::check_ready,
            commands::health_check,
            commands::open_sound_settings,
            commands::open_microphone_privacy,
            crash::crash_notice,
            crash::crash_reports,
            crash::crash_report_text,
            crash::acknowledge_crash,
            crash::reveal_crash_report,
            crash::webview_error,
            live::list_audio_devices,
            live::prepare_recording,
            live::start_recording,
            live::stop_recording,
            live::recording_status,
            live::live_snapshot,
            live::rerun_post_pass,
        ])
        .setup(move |app| {
            let store = open_store(&paths, demo, legacy.as_ref())?;
            // Playback reads stored audio through the asset protocol; the
            // scope starts at the data root (imports add their file on demand).
            app.asset_protocol_scope()
                .allow_directory(&paths.root, true)?;
            if demo {
                demo::attach_demo_audio(store.as_ref(), &paths.root);
            }
            log::info!(
                "Diarist {} started{}{}; data root {} ({:?})",
                diarist_core::VERSION,
                if demo { " in demo mode" } else { "" },
                if autostart { " at sign-in" } else { "" },
                paths.root.display(),
                paths.source
            );
            crash::log_launch(&crash_launch);
            {
                let startup = app.state::<Startup>();
                let mut report = startup.0.lock().unwrap_or_else(|e| e.into_inner());
                report.autostart = autostart;
                if let Some(m) = &moved {
                    log::info!(
                        "data folder moved from {}: removed {:?}, kept {:?}, old folder {}",
                        m.from.display(),
                        m.removed,
                        m.kept,
                        if m.removed_root { "removed" } else { "kept" }
                    );
                    report.moved_from = Some(m.from.display().to_string());
                    report.move_kept = m.kept.clone();
                }
                match &legacy_key_removed {
                    Ok(Some(true)) => log::info!(
                        "removed HKCU\\{}\\DataRoot (its data moved in)",
                        legacy::LEGACY_REGISTRY_KEY
                    ),
                    Err(e) => log::warn!("old registry value not removed: {e}"),
                    _ => {}
                }
                if let Some(l) = &legacy {
                    log::info!(
                        "pre-rename data found in {}{}; asking whether to move it in",
                        l.root.display(),
                        if l.from_registry { " (old registry value)" } else { "" }
                    );
                    report.legacy_root = Some(l.root.display().to_string());
                }
                // Before the first import can be queued: imports a crash
                // interrupted stop showing as busy.
                if !demo {
                    maintenance::recover_imports(store.as_ref(), &mut report);
                }
            }
            app.manage(AppStore(store));
            app.manage(AppPaths {
                paths: paths.clone(),
                demo,
            });

            // The main window, with its WebView2 profile under the data root.
            let config = app
                .config()
                .app
                .windows
                .iter()
                .find(|w| w.label == "main")
                .cloned()
                .ok_or("no main window in tauri.conf.json")?;
            let window = WebviewWindowBuilder::from_config(app.handle(), &config)?
                .data_directory(paths.root.clone())
                .initialization_script(crash::WEBVIEW_ERRORS_JS)
                .initialization_script(format!(
                    "window.__DIARIST_STARTUP__ = {{ autostart: {autostart}, windowStateSaved: {window_state_saved} }};"
                ))
                .build()?;
            icons::apply_window_icons(&window.as_ref().window());

            if let Err(e) = tray::build(app) {
                log::warn!("tray icon not available: {e}");
            }
            if !demo {
                let live = live::handle(app.handle());
                match app.global_shortcut().register(live::SHORTCUT) {
                    Ok(()) => log::info!(
                        "global shortcut {} starts a recording (again: asks to stop)",
                        live::SHORTCUT
                    ),
                    Err(e) => {
                        log::warn!("global shortcut {} not registered: {e}", live::SHORTCUT);
                        live.set_shortcut_error(Some(e.to_string()));
                    }
                }
            }
            // Crash recovery, then audio retention (and daily after that).
            live::startup(app.handle());

            if !autostart {
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
        .on_window_event(|window, event| match event {
            // Recording continues in the tray when the window is closed.
            WindowEvent::CloseRequested { api, .. } => {
                if live::handle(window.app_handle()).is_recording() {
                    api.prevent_close();
                    let _ = window.hide();
                    log::info!("window hidden to the tray while recording");
                }
            }
            // Icons for the new scale (another monitor, a changed setting).
            WindowEvent::ScaleFactorChanged { .. } => {
                icons::apply_window_icons(window);
                tray::rescale(window.app_handle());
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| fatal(&format!("error while building Diarist: {e}")));
    app.run(|app, event| {
        if let RunEvent::Exit = event {
            live::handle(app).shutdown();
            // Reports from panics this run survived: no notice next time.
            diarist_core::crash::clean_exit();
        }
    });
}
