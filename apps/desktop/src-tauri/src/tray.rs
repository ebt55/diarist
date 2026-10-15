//! Notification-area icon: a red dot while recording, Record/Stop, Open
//! and Quit. Left click opens the window. Stop asks for a confirmation
//! (`live::toggle`): the in-app dialog when the window is on screen, else
//! a second Stop within 3 s.

use std::sync::Mutex;

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, Wry};

use crate::live::{self, Phase};

const ID_TOGGLE: &str = "tray-toggle";
const ID_OPEN: &str = "tray-open";
const ID_QUIT: &str = "tray-quit";

/// The tray and what changes with the recording state.
pub(crate) struct Tray {
    icon: TrayIcon<Wry>,
    toggle: MenuItem<Wry>,
    idle: Image<'static>,
    recording: Image<'static>,
    shown: Mutex<Option<Phase>>,
}

/// `base` with a red dot (white ring) in the lower right corner.
fn with_red_dot(base: &Image<'_>) -> Image<'static> {
    let (w, h) = (base.width() as i64, base.height() as i64);
    let mut px = base.rgba().to_vec();
    let r = (w.min(h) as f64) * 0.27;
    let ring = r + (w.min(h) as f64 * 0.06).max(1.0);
    let (cx, cy) = (w as f64 - ring - 0.5, h as f64 - ring - 0.5);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
            let i = ((y * w + x) * 4) as usize;
            let color = if d <= r {
                [0xDC, 0x26, 0x26, 0xFF]
            } else if d <= ring {
                [0xFF, 0xFF, 0xFF, 0xFF]
            } else {
                continue;
            };
            px[i..i + 4].copy_from_slice(&color);
        }
    }
    Image::new_owned(px, w as u32, h as u32)
}

pub(crate) fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Builds the tray (once, in setup). Failure leaves the app without a tray.
pub(crate) fn build(app: &App) -> tauri::Result<()> {
    let Some(base) = app.default_window_icon().cloned() else {
        log::warn!("no window icon; the tray is not shown");
        return Ok(());
    };
    let idle = base.to_owned();
    let recording = with_red_dot(&idle);
    let toggle = MenuItem::with_id(app, ID_TOGGLE, "Start recording", true, Some("Ctrl+Alt+R"))?;
    let open = MenuItem::with_id(app, ID_OPEN, "Open meet-assist", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&toggle, &open, &sep, &quit])?;
    let icon = TrayIconBuilder::with_id("main")
        .icon(idle.clone())
        .tooltip("meet-assist")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            ID_TOGGLE => live::toggle(app, diarist_core::live::ToggleVia::Tray),
            ID_OPEN => show_main(app),
            ID_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    app.manage(Tray {
        icon,
        toggle,
        idle,
        recording,
        shown: Mutex::new(None),
    });
    Ok(())
}

/// While a stop is armed (first press with the window hidden), the menu
/// item says so; disarmed, it reads "Stop recording" again.
pub(crate) fn set_stop_armed(app: &AppHandle, armed: bool) {
    let Some(tray) = app.try_state::<Tray>() else {
        return;
    };
    let recording = *tray.shown.lock().unwrap_or_else(|e| e.into_inner()) == Some(Phase::Recording);
    if !recording {
        return;
    }
    let _ = tray.toggle.set_text(if armed {
        "Stop recording: choose again to confirm"
    } else {
        "Stop recording"
    });
}

/// Reflects the recording state (any thread).
pub(crate) fn update(app: &AppHandle, phase: Phase, started_at: Option<i64>) {
    let Some(tray) = app.try_state::<Tray>() else {
        return;
    };
    {
        let mut shown = tray.shown.lock().unwrap_or_else(|e| e.into_inner());
        if *shown == Some(phase) {
            return;
        }
        *shown = Some(phase);
    }
    let recording = matches!(phase, Phase::Recording | Phase::Stopping);
    let _ = tray.icon.set_icon(Some(if recording {
        tray.recording.clone()
    } else {
        tray.idle.clone()
    }));
    let (text, enabled) = match phase {
        Phase::Idle => ("Start recording", true),
        Phase::Starting => ("Starting…", false),
        Phase::Recording => ("Stop recording", true),
        Phase::Stopping => ("Stopping…", false),
    };
    let _ = tray.toggle.set_text(text);
    let _ = tray.toggle.set_enabled(enabled);
    let tip = match (phase, started_at) {
        (Phase::Recording, Some(t)) => {
            let fmt = time::macros::format_description!("[hour]:[minute]");
            let since = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(t) * 1_000_000)
                .ok()
                .map(|d| match time::UtcOffset::current_local_offset() {
                    Ok(off) => d.to_offset(off),
                    Err(_) => d,
                })
                .and_then(|d| d.format(&fmt).ok())
                .unwrap_or_default();
            format!("meet-assist: recording since {since}")
        }
        (Phase::Recording, None) => "meet-assist: recording".into(),
        (Phase::Starting, _) => "meet-assist: starting a recording".into(),
        (Phase::Stopping, _) => "meet-assist: stopping".into(),
        (Phase::Idle, _) => "meet-assist".into(),
    };
    let _ = tray.icon.set_tooltip(Some(tip));
}
