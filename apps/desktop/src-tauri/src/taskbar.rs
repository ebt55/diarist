//! The taskbar button while recording: the window title reads
//! "● Recording · Diarist" and the button carries an overlay, a red dot in
//! a white ring (`branding/recording/overlay-*.svg`), drawn at the frame
//! for the window's DPI (`icons.rs`). Both follow the tray's dot: on while
//! recording or stopping, off otherwise.
//!
//! The overlay goes through `ITaskbarList3::SetOverlayIcon` directly
//! rather than Tauri's `set_overlay_icon` (the same call), because only the
//! direct call carries a description, "Recording", which screen readers
//! announce with the button. An overlay belongs to the taskbar button, and
//! the button goes away while the window is hidden to the tray (and when
//! Explorer restarts), so it is set again whenever Windows sends
//! `TaskbarButtonCreated`, when the window is shown again
//! (`tray::show_main`) and when its DPI changes (`icons::refresh`).

use std::sync::Mutex;

use tauri::{AppHandle, Manager};

use crate::live::Phase;

/// The window title when not recording (as in `tauri.conf.json`).
pub(crate) const TITLE: &str = "Diarist";
/// The window title while recording.
pub(crate) const TITLE_RECORDING: &str = "\u{25cf} Recording \u{b7} Diarist";

/// Whether a phase shows the recording state (the tray's dot, the overlay
/// and the title): from the first recorded second until the stop is done.
pub(crate) fn is_recording(phase: Phase) -> bool {
    matches!(phase, Phase::Recording | Phase::Stopping)
}

/// The window title for a phase.
pub(crate) fn title(phase: Phase) -> &'static str {
    if is_recording(phase) {
        TITLE_RECORDING
    } else {
        TITLE
    }
}

/// What the title and the overlay show (`None` until the first update).
static SHOWN: Mutex<Option<bool>> = Mutex::new(None);

fn shown() -> Option<bool> {
    *SHOWN.lock().unwrap_or_else(|e| e.into_inner())
}

/// Reflects the recording state (any thread); does nothing when it did
/// not change.
pub(crate) fn update(app: &AppHandle, phase: Phase) {
    let on = is_recording(phase);
    {
        let mut shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
        if *shown == Some(on) {
            return;
        }
        *shown = Some(on);
    }
    if let Some(window) = app.get_webview_window("main") {
        if let Err(e) = window.set_title(title(phase)) {
            log::warn!("window title not set: {e}");
        }
    }
    sync(app);
}

/// Sets the overlay again while recording (the window was shown again, or
/// its DPI changed).
pub(crate) fn reapply(app: &AppHandle) {
    if shown() == Some(true) {
        sync(app);
    }
}

/// Brings the overlay in line with [`SHOWN`] on the main thread, where the
/// window and COM live. The state is read there, so calls made in any
/// order end on the latest one.
fn sync(app: &AppHandle) {
    #[cfg(windows)]
    {
        let handle = app.clone();
        let run = app.run_on_main_thread(move || {
            let Some(window) = handle.get_webview_window("main") else {
                return;
            };
            let Ok(hwnd) = window.hwnd() else {
                return;
            };
            win::watch(hwnd);
            win::set_overlay(hwnd, shown() == Some(true));
        });
        if let Err(e) = run {
            log::warn!("taskbar overlay not scheduled: {e}");
        }
    }
    #[cfg(not(windows))]
    let _ = app;
}

#[cfg(windows)]
mod win {
    use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
    use std::sync::Mutex;

    use windows::core::{w, IUnknown, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::{
        DefSubclassProc, ITaskbarList3, RemoveWindowSubclass, SetWindowSubclass, TaskbarList,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DestroyIcon, RegisterWindowMessageW, HICON, WM_NCDESTROY,
    };

    /// The overlay set last, destroyed once replaced or cleared.
    static ICON: Mutex<Option<isize>> = Mutex::new(None);
    /// The window whose messages are watched, and `TaskbarButtonCreated`.
    static WATCHED: AtomicIsize = AtomicIsize::new(0);
    static BUTTON_CREATED: AtomicU32 = AtomicU32::new(0);
    const SUBCLASS_ID: usize = 1;

    /// Watches the window for a new taskbar button (main thread, once).
    pub(super) fn watch(hwnd: HWND) {
        if WATCHED.load(Ordering::Relaxed) == hwnd.0 as isize {
            return;
        }
        let msg = unsafe { RegisterWindowMessageW(w!("TaskbarButtonCreated")) };
        BUTTON_CREATED.store(msg, Ordering::Relaxed);
        if unsafe { SetWindowSubclass(hwnd, Some(proc), SUBCLASS_ID, 0) }.as_bool() {
            WATCHED.store(hwnd.0 as isize, Ordering::Relaxed);
        } else {
            log::warn!("taskbar button events not watched; the overlay may not return after the window is hidden");
        }
    }

    unsafe extern "system" fn proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        let created = BUTTON_CREATED.load(Ordering::Relaxed);
        if created != 0 && msg == created && super::shown() == Some(true) {
            set_overlay(hwnd, true);
        } else if msg == WM_NCDESTROY {
            unsafe {
                let _ = RemoveWindowSubclass(hwnd, Some(proc), SUBCLASS_ID);
            }
            WATCHED.store(0, Ordering::Relaxed);
        }
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    /// Sets (the frame for the window's DPI) or clears the overlay (main
    /// thread). Fails quietly while the window has no taskbar button.
    pub(super) fn set_overlay(hwnd: HWND, on: bool) {
        let icon = if on {
            match crate::icons::overlay_icon(hwnd) {
                Some((icon, _)) => Some(icon),
                None => {
                    log::warn!("taskbar overlay frame not loaded");
                    return;
                }
            }
        } else {
            None
        };
        let result = unsafe {
            CoCreateInstance::<_, ITaskbarList3>(
                &TaskbarList,
                None::<&IUnknown>,
                CLSCTX_INPROC_SERVER,
            )
            .and_then(|list| {
                list.HrInit()?;
                match icon {
                    Some(icon) => list.SetOverlayIcon(hwnd, icon, w!("Recording")),
                    None => list.SetOverlayIcon(hwnd, HICON::default(), PCWSTR::null()),
                }
            })
        };
        match &result {
            Ok(()) => log::info!("taskbar overlay {}", if on { "on" } else { "off" }),
            Err(e) => log::debug!(
                "taskbar overlay not {}: {e}",
                if on { "set" } else { "cleared" }
            ),
        }
        // Keep the icon the taskbar was given; destroy the one it replaced.
        let keep = match (icon, result.is_ok()) {
            (Some(icon), true) => Some(icon.0 as isize),
            (Some(icon), false) => {
                unsafe {
                    let _ = DestroyIcon(icon);
                }
                return;
            }
            (None, _) => None,
        };
        let old = std::mem::replace(&mut *ICON.lock().unwrap_or_else(|e| e.into_inner()), keep);
        if let Some(old) = old {
            unsafe {
                let _ = DestroyIcon(HICON(old as *mut _));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recording_state_follows_the_phase() {
        assert!(!is_recording(Phase::Idle));
        assert!(!is_recording(Phase::Starting));
        assert!(is_recording(Phase::Recording));
        assert!(is_recording(Phase::Stopping));
    }

    #[test]
    fn the_title_says_recording_and_returns_to_the_name() {
        assert_eq!(title(Phase::Recording), "● Recording · Diarist");
        assert_eq!(title(Phase::Stopping), "● Recording · Diarist");
        assert_eq!(title(Phase::Idle), "Diarist");
        assert_eq!(title(Phase::Starting), "Diarist");
        // Back to the window's configured title, not a guess at it.
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        assert_eq!(config["app"]["windows"][0]["title"], TITLE);
    }
}
