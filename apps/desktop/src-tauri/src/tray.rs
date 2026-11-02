//! Notification-area icon: a red dot while recording, Record/Stop, Open
//! and Quit. Left click opens the window. Stop asks for a confirmation
//! (`live::toggle`): the in-app dialog when the window is on screen, else
//! a second Stop within 3 s. The icon is the app icon's frame for the
//! notification area's size and the taskbar's theme (`icons.rs`), chosen
//! again when either changes; while recording, the same frame with the
//! red dot drawn into its lower-right corner (`branding/recording/`).

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
    /// Idle, and with the recording dot.
    images: Mutex<(Image<'static>, Image<'static>)>,
    shown: Mutex<Option<Phase>>,
}

pub(crate) fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
    // A hidden window has no taskbar button, so its overlay went with it.
    crate::taskbar::reapply(app);
}

/// Builds the tray (once, in setup). Failure leaves the app without a tray.
pub(crate) fn build(app: &App) -> tauri::Result<()> {
    let Some((idle, recording)) = crate::icons::tray_images() else {
        log::warn!("no app icon; the tray is not shown");
        return Ok(());
    };
    log::info!("tray icon {} px", idle.width());
    let toggle = MenuItem::with_id(app, ID_TOGGLE, "Start recording", true, Some("Ctrl+Alt+R"))?;
    let open = MenuItem::with_id(app, ID_OPEN, "Open Diarist", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&toggle, &open, &sep, &quit])?;
    let icon = TrayIconBuilder::with_id("main")
        .icon(idle.clone())
        .tooltip("Diarist")
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
        images: Mutex::new((idle, recording)),
        shown: Mutex::new(None),
    });
    Ok(())
}

/// Reloads the icon for the current scale and taskbar theme.
pub(crate) fn rescale(app: &AppHandle) {
    let Some(tray) = app.try_state::<Tray>() else {
        return;
    };
    let Some((idle, recording)) = crate::icons::tray_images() else {
        return;
    };
    let mut images = tray.images.lock().unwrap_or_else(|e| e.into_inner());
    if images.0.width() == idle.width() && images.0.rgba() == idle.rgba() {
        return;
    }
    log::info!("tray icon {} px", idle.width());
    *images = (idle, recording);
    let shown = *tray.shown.lock().unwrap_or_else(|e| e.into_inner());
    let _ = tray.icon.set_icon(Some(icon_for(&images, shown)));
}

/// The image for a phase: with the dot while recording or stopping.
fn icon_for(images: &(Image<'static>, Image<'static>), phase: Option<Phase>) -> Image<'static> {
    if phase.is_some_and(crate::taskbar::is_recording) {
        images.1.clone()
    } else {
        images.0.clone()
    }
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
        "Stop recording? Choose again to stop"
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
    let image = icon_for(
        &tray.images.lock().unwrap_or_else(|e| e.into_inner()),
        Some(phase),
    );
    let _ = tray.icon.set_icon(Some(image));
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
            format!("Diarist: recording since {since}")
        }
        (Phase::Recording, None) => "Diarist: recording".into(),
        (Phase::Starting, _) => "Diarist: starting a recording".into(),
        (Phase::Stopping, _) => "Diarist: stopping".into(),
        (Phase::Idle, _) => "Diarist".into(),
    };
    let _ = tray.icon.set_tooltip(Some(tip));
}
