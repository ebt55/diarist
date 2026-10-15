//! Tauri shell for meet-assist. Only command wrappers and event plumbing live
//! here; logic belongs in `meet-core` (docs/01-architecture.md). Commands call
//! [`Store`] methods inside `spawn_blocking` and map errors to strings.
//!
//! Mock-only pieces (import timer, demo audio, GPU numbers) are confined to
//! `mock.rs` and written against the `Store` trait.

mod audio;
mod commands;
pub mod events;
mod export;
mod mock;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use meet_core::pipeline::JobQueue;
use meet_core::store::{MemoryStore, Store};
use tauri::Manager;
use tauri_plugin_log::{RotationStrategy, TimezoneStrategy};

/// If the frontend never calls `show()` (crashed bundle, failed load), reveal
/// the window anyway so the app is not an invisible process.
const SHOW_FALLBACK: Duration = Duration::from_secs(3);

/// The store behind every command. Until Phase 1 lands `SqliteStore`, this is
/// the seeded in-memory mock; swapping it is a one-line change in [`run`].
pub(crate) struct AppStore(pub(crate) Arc<dyn Store>);

/// One GPU job at a time (imports now, post-passes later).
pub(crate) struct Jobs(pub(crate) JobQueue);

/// Resolved locations under the data root.
pub(crate) struct AppPaths {
    pub(crate) data_root: PathBuf,
}

pub fn run() {
    // The one line to change when SqliteStore lands.
    let store: Arc<dyn Store> = Arc::new(MemoryStore::demo());
    let setup_store = Arc::clone(&store);

    tauri::Builder::default()
        .manage(AppStore(store))
        .manage(Jobs(JobQueue::new()))
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
            commands::reassign_segment,
            commands::split_segment,
            commands::set_meeting_title,
            commands::set_audio_keep,
            commands::delete_meeting,
            commands::export_meeting,
            commands::get_audio_url,
            commands::get_waveform,
            commands::get_settings,
            commands::set_setting,
            commands::engine_status,
            commands::list_model_files,
            commands::select_model,
            commands::rescan_models,
            commands::open_models_folder,
            commands::open_data_folder,
            commands::read_log_tail,
            commands::last_engine_stderr,
            commands::ui_log,
        ])
        .setup(move |app| {
            let data_root = app.path().app_local_data_dir()?;
            std::fs::create_dir_all(&data_root)?;
            // Playback reads stored audio through the asset protocol; the
            // scope starts at the data root (imports add their file on demand).
            app.asset_protocol_scope()
                .allow_directory(&data_root, true)?;
            commands::apply_default_settings(setup_store.as_ref());
            mock::attach_demo_audio(setup_store.as_ref(), &data_root);
            log::info!(
                "meet-assist {} started; data root {}",
                meet_core::VERSION,
                data_root.display()
            );
            app.manage(AppPaths { data_root });

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
        .run(tauri::generate_context!())
        .expect("error while running meet-assist");
}
