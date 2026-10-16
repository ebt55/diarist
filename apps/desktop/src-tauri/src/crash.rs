//! Crash capture in the app (docs/plans/11-hardening.md 8a): installed first
//! thing at launch, the launch notice ("Diarist closed unexpectedly"), the
//! Diagnostics list and WebView error forwarding. Reports are written by
//! `diarist_core::crash`; nothing is sent anywhere.

use std::sync::Mutex;

use diarist_core::crash::{self, CrashConfig, CrashReport};
use diarist_core::models::DataPaths;
use tauri::State;

use crate::AppPaths;

type CmdResult<T> = Result<T, String>;

/// The report the previous run left, until the notice is acknowledged.
pub(crate) struct CrashNotice(pub(crate) Mutex<Option<CrashReport>>);

/// What [`install`] found, logged once the logger runs.
pub(crate) struct Launch {
    pub(crate) notice: Option<CrashReport>,
    pub(crate) pruned: Vec<String>,
}

/// Turns on crash capture (panic hook, native exception filter, engine
/// exit reports) before anything else runs, after reading what the previous
/// run left for the notice. Old reports beyond five of each kind go.
pub(crate) fn install(paths: &DataPaths) -> Launch {
    let logs = paths.logs();
    let notice = crash::pending_notice(&logs);
    let pruned = crash::install(CrashConfig {
        logs_dir: logs.clone(),
        app: format!("Diarist {}", diarist_core::VERSION),
        // tauri-plugin-log's file target (`file_name: "diarist"`).
        app_log: Some(logs.join("diarist.log")),
        prune: true,
    });
    Launch { notice, pruned }
}

pub(crate) fn log_launch(launch: &Launch) {
    if let Some(r) = &launch.notice {
        log::warn!(
            "the previous run left a crash report: logs\\{}{}",
            r.name,
            r.dump
                .as_ref()
                .map_or(String::new(), |d| format!(" (minidump {d})"))
        );
    }
    if !launch.pruned.is_empty() {
        log::info!(
            "deleted old crash reports (the newest five of each kind stay): {}",
            launch.pruned.join(", ")
        );
    }
}

/// Runs before any page script: uncaught errors and unhandled rejections go
/// to the app log through `webview_error`, with only the error text and the
/// stack (never the value a promise was rejected with, unless it is a
/// string or an Error). At most 50 per page load.
pub(crate) const WEBVIEW_ERRORS_JS: &str = r#"(() => {
  if (window.__diaristErrors) return;
  window.__diaristErrors = true;
  let sent = 0;
  const send = (kind, message, stack) => {
    if (sent >= 50) return;
    sent += 1;
    const args = {
      kind,
      message: String(message).slice(0, 2000),
      stack: stack ? String(stack).slice(0, 8000) : null,
    };
    const go = () => window.__TAURI_INTERNALS__.invoke("webview_error", args).catch(() => {});
    try {
      go();
    } catch (_) {
      setTimeout(() => {
        try { go(); } catch (_) {}
      }, 1000);
    }
  };
  window.addEventListener("error", (e) => {
    if (!(e instanceof ErrorEvent)) return;
    const err = e.error;
    const where = e.filename ? `${e.filename}:${e.lineno}:${e.colno}` : null;
    send("error", e.message || (err && err.message) || "Script error", (err && err.stack) || where);
  });
  window.addEventListener("unhandledrejection", (e) => {
    const r = e.reason;
    if (r instanceof Error) send("unhandledrejection", `${r.name}: ${r.message}`, r.stack);
    else if (typeof r === "string") send("unhandledrejection", r, null);
    else send("unhandledrejection", `a value of type ${r === null ? "null" : typeof r} (not logged)`, null);
  });
})();"#;

async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> CmdResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

/// `s` cut to at most `max` bytes on a character boundary.
fn clip(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// The newest app crash report the previous run left, unacknowledged.
#[tauri::command]
pub async fn crash_notice(notice: State<'_, CrashNotice>) -> CmdResult<Option<CrashReport>> {
    Ok(notice.0.lock().unwrap_or_else(|e| e.into_inner()).clone())
}

/// Every report in the log folder, newest first (Diagnostics).
#[tauri::command]
pub async fn crash_reports(paths: State<'_, AppPaths>) -> CmdResult<Vec<CrashReport>> {
    let logs = paths.paths.logs();
    blocking(move || Ok(crash::list_reports(&logs))).await
}

/// A text report for "Copy report", with the profile folder as `~`.
#[tauri::command(rename_all = "snake_case")]
pub async fn crash_report_text(paths: State<'_, AppPaths>, name: String) -> CmdResult<String> {
    let logs = paths.paths.logs();
    blocking(move || crash::read_report(&logs, &name).map_err(|e| e.to_string())).await
}

/// The notice for `name` (and anything older) was seen.
#[tauri::command(rename_all = "snake_case")]
pub async fn acknowledge_crash(
    paths: State<'_, AppPaths>,
    notice: State<'_, CrashNotice>,
    name: String,
) -> CmdResult<()> {
    {
        let mut n = notice.0.lock().unwrap_or_else(|e| e.into_inner());
        if n.as_ref().is_some_and(|r| r.name == name) {
            *n = None;
        }
    }
    let logs = paths.paths.logs();
    blocking(move || crash::acknowledge(&logs, &name).map_err(|e| e.to_string())).await
}

/// Opens the log folder with the report (or its minidump) selected.
#[tauri::command(rename_all = "snake_case")]
pub async fn reveal_crash_report(paths: State<'_, AppPaths>, name: String) -> CmdResult<()> {
    let logs = paths.paths.logs();
    blocking(move || {
        let known = crash::list_reports(&logs)
            .iter()
            .any(|r| r.name == name || r.dump.as_deref() == Some(name.as_str()));
        if !known {
            return Err(format!(
                "No crash report named {name} in {}",
                logs.display()
            ));
        }
        let mut cmd = std::process::Command::new(if cfg!(windows) {
            "explorer"
        } else {
            "xdg-open"
        });
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.raw_arg(format!("/select,\"{}\"", logs.join(&name).display()));
        }
        #[cfg(not(windows))]
        cmd.arg(&logs);
        cmd.spawn()
            .map(|_| ())
            .map_err(|e| format!("Cannot open {}: {e}", logs.display()))
    })
    .await
}

/// An uncaught error or unhandled rejection in the WebView, for the app log.
#[tauri::command(rename_all = "snake_case")]
pub async fn webview_error(kind: String, message: String, stack: Option<String>) -> CmdResult<()> {
    let what = if kind == "unhandledrejection" {
        "unhandled promise rejection"
    } else {
        "uncaught error"
    };
    let message = clip(&message, 2_000);
    match stack.as_deref().map(|s| clip(s, 8_000).trim_end()) {
        Some(stack) if !stack.is_empty() => {
            log::error!(target: "webview", "{what}: {message}\n{stack}")
        }
        _ => log::error!(target: "webview", "{what}: {message}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::clip;

    #[test]
    fn clip_keeps_whole_characters() {
        assert_eq!(clip("abc", 5), "abc");
        assert_eq!(clip("abc", 2), "ab");
        assert_eq!(clip("aé", 2), "a");
    }
}
