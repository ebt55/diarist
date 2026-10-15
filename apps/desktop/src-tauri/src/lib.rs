//! Tauri shell for meet-assist. Only command wrappers and event plumbing live
//! here; logic belongs in `meet-core` (docs/01-architecture.md).

mod fixtures;

use std::time::Duration;

use meet_core::Meeting;
use tauri::Manager;

/// If the frontend never calls `show()` (crashed bundle, failed load), reveal
/// the window anyway so the app is not an invisible process.
const SHOW_FALLBACK: Duration = Duration::from_secs(3);

/// All meetings, newest first. Phase 0: fixtures.
#[tauri::command]
fn list_meetings() -> Vec<Meeting> {
    fixtures::meetings()
}

pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(tauri_plugin_log::log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![list_meetings])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                std::thread::spawn(move || {
                    std::thread::sleep(SHOW_FALLBACK);
                    if !window.is_visible().unwrap_or(true) {
                        let _ = window.show();
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running meet-assist");
}
