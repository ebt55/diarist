//! Launch-time and periodic housekeeping: the startup report (what crash
//! recovery and a data-folder move did, shown once by the UI) and audio
//! retention (`meet_core::retention`) at launch and then once a day.

use std::time::Duration;

use meet_core::retention;
use meet_core::store::RunStore;
use meet_core::views::{RecoveredItem, StartupReport};
use tauri::{AppHandle, Emitter, Manager};

use crate::{AppPaths, AppStore, Startup};

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// How often retention runs while the app stays open.
const RETENTION_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// Imports a crash interrupted become `failed` (before any import is queued).
pub(crate) fn recover_imports(store: &dyn RunStore, report: &mut StartupReport) {
    match meet_core::pipeline::recover_interrupted_imports(store) {
        Ok(list) => {
            for (id, title) in list {
                log::warn!("import of meeting {id} ({title:?}) was interrupted; marked failed");
                report.recovered.push(RecoveredItem {
                    meeting_id: id,
                    title,
                    was: "import".into(),
                    action: "The import was interrupted. Import the file again.".into(),
                });
            }
        }
        Err(e) => log::error!("checking for interrupted imports failed: {e}"),
    }
}

/// Adds what live recovery did, marks recovery done and tells the UI.
pub(crate) fn finish_recovery(app: &AppHandle, items: Vec<RecoveredItem>, error: Option<String>) {
    let payload = {
        let startup = app.state::<Startup>();
        let mut r = startup.0.lock().unwrap_or_else(|e| e.into_inner());
        r.recovered.extend(items);
        r.recovery_done = true;
        r.recovery_error = error;
        r.clone()
    };
    if let Err(e) = app.emit(StartupReport::EVENT, &payload) {
        log::warn!("emit {} failed: {e}", StartupReport::EVENT);
    }
}

/// Deletes audio past retention (never transcripts) and logs what it did.
pub(crate) fn run_retention(app: &AppHandle) -> Result<retention::RetentionReport, String> {
    let paths = app.state::<AppPaths>();
    if paths.demo {
        return Ok(retention::RetentionReport::default());
    }
    let store = std::sync::Arc::clone(&app.state::<AppStore>().0);
    let plan = retention::plan_from_settings(&*store, &paths.paths.root, now_ms())
        .map_err(|e| e.to_string())?;
    if plan.meetings.is_empty() {
        return Ok(retention::RetentionReport::default());
    }
    let report = retention::apply(&*store, &plan);
    log::info!(
        "retention ({} days): deleted {} audio files ({} MB) of {} meetings; {} pinned kept{}",
        plan.days,
        report.files,
        report.bytes >> 20,
        report.meetings,
        plan.pinned_skipped,
        if report.errors.is_empty() {
            String::new()
        } else {
            format!("; errors: {:?}", report.errors)
        }
    );
    Ok(report)
}

/// Runs retention now and then once a day on its own thread.
pub(crate) fn start_retention(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("retention".into())
        .spawn(move || loop {
            if let Err(e) = run_retention(&app) {
                log::warn!("retention failed: {e}");
            }
            std::thread::sleep(RETENTION_EVERY);
        });
    if let Err(e) = spawned {
        log::warn!("cannot start the retention job: {e}");
    }
}
