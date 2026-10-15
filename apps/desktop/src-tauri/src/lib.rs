//! Tauri shell for meet-assist. Only command wrappers and event plumbing live
//! here; logic belongs in `meet-core` (docs/01-architecture.md). Commands call
//! [`Store`] methods inside `spawn_blocking` and map errors to strings.

use std::sync::Arc;
use std::time::Duration;

use meet_core::store::{MemoryStore, Store};
use meet_core::views::MeetingSummary;
use tauri::{Manager, State};

/// If the frontend never calls `show()` (crashed bundle, failed load), reveal
/// the window anyway so the app is not an invisible process.
const SHOW_FALLBACK: Duration = Duration::from_secs(3);

/// The store behind every command. Until Phase 1 lands `SqliteStore`, this is
/// the seeded in-memory mock; swapping it is a one-line change in [`run`].
struct AppStore(Arc<dyn Store>);

/// Runs a store call on the blocking pool.
async fn with_store<T, F>(store: &State<'_, AppStore>, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&dyn Store) -> meet_core::store::StoreResult<T> + Send + 'static,
{
    let store = Arc::clone(&store.0);
    tauri::async_runtime::spawn_blocking(move || f(store.as_ref()))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// All meetings, newest first.
#[tauri::command]
async fn list_meetings(store: State<'_, AppStore>) -> Result<Vec<MeetingSummary>, String> {
    with_store(&store, |s| s.list_meetings()).await
}

pub fn run() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::demo());

    tauri::Builder::default()
        .manage(AppStore(store))
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
