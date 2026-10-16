//! Window and tray icons that follow the taskbar: the app icon comes as a
//! pair (`branding/icon.svg`, the ink page for light backgrounds, and
//! `branding/icon-dark.svg`, the paper page for dark ones), each drawn at
//! 16, 20, 24, 32 and 48 px (`icons/theme/<light|dark>/<size>.png`, the
//! small ones on the pixel grid). The exe's own icon (`icon.ico`, the paper
//! page) stays what Explorer, the Start menu and a pinned button show.
//!
//! Tauri gives the window one 32 px icon as its small icon only, so the
//! taskbar enlarges it to its button (48 px at 200 percent). Here the window
//! gets a small icon (title bar) and a big one (the taskbar button, which
//! Windows 11 takes from the window's big icon while the app runs, and
//! Alt+Tab), and the tray an image of the notification area's size, from
//! the set for the taskbar's theme (`SystemUsesLightTheme`) and the frame
//! for the scale. They are chosen again when the window's scale or theme
//! changes and when it gets the focus (a cheap registry read), so a taskbar
//! switched between light and dark is followed at the latest the next time
//! the window is used.

/// Logical sizes: the small icon (title bar, notification area) and the
/// taskbar button's icon.
const SMALL: u32 = 16;
const TASKBAR: u32 = 24;

/// The frames of each set, smallest first.
const FRAMES: [u32; 5] = [16, 20, 24, 32, 48];

macro_rules! frames {
    ($set:literal) => {
        [
            include_bytes!(concat!("../icons/theme/", $set, "/16.png")).as_slice(),
            include_bytes!(concat!("../icons/theme/", $set, "/20.png")).as_slice(),
            include_bytes!(concat!("../icons/theme/", $set, "/24.png")).as_slice(),
            include_bytes!(concat!("../icons/theme/", $set, "/32.png")).as_slice(),
            include_bytes!(concat!("../icons/theme/", $set, "/48.png")).as_slice(),
        ]
    };
}

/// `icon.svg`, the ink page, for a light taskbar.
static LIGHT: [&[u8]; 5] = frames!("light");
/// `icon-dark.svg`, the paper page, for a dark taskbar.
static DARK: [&[u8]; 5] = frames!("dark");

/// `(small, taskbar)` edge lengths in pixels for a DPI (96 = 100 percent):
/// 16 and 24 at 100 percent, 20 and 30 at 125, 24 and 36 at 150, 32 and 48
/// at 200.
pub(crate) fn icon_edges(dpi: u32) -> (u32, u32) {
    let dpi = if dpi == 0 { 96 } else { dpi };
    let at = |logical: u32| (logical * dpi + 48) / 96;
    (at(SMALL), at(TASKBAR))
}

/// The frame drawn for `px`: the smallest one at least that big (Windows
/// then only ever scales down), else the largest. Returns its size and PNG.
pub(crate) fn frame(light: bool, px: u32) -> (u32, &'static [u8]) {
    let set = if light { &LIGHT } else { &DARK };
    let i = FRAMES
        .iter()
        .position(|&f| f >= px)
        .unwrap_or(FRAMES.len() - 1);
    (FRAMES[i], set[i])
}

/// RGBA pixels of an 8-bit RGBA PNG (the frames are written that way).
fn decode_rgba(png_bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("frame too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!(
            "frame is {:?} {:?}, not 8-bit RGBA",
            info.color_type, info.bit_depth
        ));
    }
    buf.truncate(info.line_size * info.height as usize);
    Ok((buf, info.width, info.height))
}

/// Whether the taskbar is light (`SystemUsesLightTheme` = 1); dark when
/// the value is missing, as on Windows 10.
pub(crate) fn taskbar_is_light() -> bool {
    #[cfg(windows)]
    {
        win::system_uses_light_theme().unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
mod win {
    use std::sync::Mutex;

    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateIconFromResourceEx, DestroyIcon, FindWindowW, SendMessageW, HICON, ICON_BIG,
        ICON_SMALL, LR_DEFAULTCOLOR, WM_SETICON,
    };

    pub(super) fn system_uses_light_theme() -> Option<bool> {
        let mut value = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
                w!("SystemUsesLightTheme"),
                RRF_RT_REG_DWORD,
                None,
                Some((&mut value as *mut u32).cast()),
                Some(&mut size),
            )
        };
        rc.is_ok().then_some(value != 0)
    }

    /// An icon from a PNG frame (Windows decodes PNG icon images itself).
    fn icon_from_png(png: &[u8], size: u32) -> Option<HICON> {
        let size = size as i32;
        unsafe { CreateIconFromResourceEx(png, true, 0x0003_0000, size, size, LR_DEFAULTCOLOR) }
            .ok()
    }

    /// The icons this module set last (small, big), destroyed when replaced.
    static SET: Mutex<Option<(isize, isize)>> = Mutex::new(None);

    /// Sets ICON_BIG and ICON_SMALL; returns the DPI and both frame sizes.
    pub(super) fn apply(hwnd: HWND, light: bool) -> Option<(u32, u32, u32)> {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let (small, big) = super::icon_edges(dpi);
        let ((sf, sp), (bf, bp)) = (super::frame(light, small), super::frame(light, big));
        let (s, b) = (icon_from_png(sp, sf)?, icon_from_png(bp, bf)?);
        let send = |kind: u32, icon: HICON| unsafe {
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(kind as usize)),
                Some(LPARAM(icon.0 as isize)),
            );
        };
        // Big first: Explorer reads both again when the small one changes.
        send(ICON_BIG, b);
        send(ICON_SMALL, s);
        let old = SET
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace((s.0 as isize, b.0 as isize));
        if let Some((os, ob)) = old {
            unsafe {
                let _ = DestroyIcon(HICON(os as *mut _));
                let _ = DestroyIcon(HICON(ob as *mut _));
            }
        }
        Some((dpi, sf, bf))
    }

    /// The DPI of the taskbar that holds the notification area.
    pub(super) fn taskbar_dpi() -> u32 {
        unsafe {
            match FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) {
                Ok(h) if GetDpiForWindow(h) > 0 => GetDpiForWindow(h),
                _ => GetDpiForSystem(),
            }
        }
    }
}

/// What the icons were last chosen for: (light taskbar, window DPI).
static CHOSEN: std::sync::Mutex<Option<(bool, u32)>> = std::sync::Mutex::new(None);

/// Sets the window's small and big icons for its DPI and the taskbar's
/// theme, and the tray's (Windows; elsewhere the default icon stays).
/// Does nothing when neither changed since the last call, unless `force`.
pub(crate) fn refresh(window: &tauri::Window, force: bool) {
    #[cfg(windows)]
    {
        use tauri::Manager;
        let Ok(hwnd) = window.hwnd() else {
            return;
        };
        let light = taskbar_is_light();
        let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) };
        {
            let mut chosen = CHOSEN.lock().unwrap_or_else(|e| e.into_inner());
            if !force && *chosen == Some((light, dpi)) {
                return;
            }
            *chosen = Some((light, dpi));
        }
        let theme = if light { "light" } else { "dark" };
        match win::apply(hwnd, light) {
            Some((dpi, small, big)) => {
                log::info!("window icons {small} and {big} px ({theme} taskbar, {dpi} dpi)")
            }
            None => log::warn!("window icons not set from the {theme} frames"),
        }
        crate::tray::rescale(window.app_handle());
    }
    #[cfg(not(windows))]
    let _ = (window, force, &CHOSEN);
}

/// The tray's image: the frame for the notification area's size at the
/// taskbar's DPI, from the set for the taskbar's theme.
pub(crate) fn tray_image() -> Option<tauri::image::Image<'static>> {
    #[cfg(windows)]
    let dpi = win::taskbar_dpi();
    #[cfg(not(windows))]
    let dpi = 96;
    let (small, _) = icon_edges(dpi);
    let (size, png) = frame(taskbar_is_light(), small);
    match decode_rgba(png) {
        Ok((rgba, w, h)) => Some(tauri::image::Image::new_owned(rgba, w, h)),
        Err(e) => {
            log::warn!("tray icon frame {size} px not decoded: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_edges_follow_the_scale() {
        assert_eq!(icon_edges(96), (16, 24));
        assert_eq!(icon_edges(120), (20, 30));
        assert_eq!(icon_edges(144), (24, 36));
        assert_eq!(icon_edges(168), (28, 42));
        assert_eq!(icon_edges(192), (32, 48));
        assert_eq!(icon_edges(0), (16, 24));
    }

    #[test]
    fn frames_are_picked_by_size_and_theme() {
        // Small icon and tray at 100, 125, 150, 175 and 200 percent.
        let small: Vec<u32> = [96, 120, 144, 168, 192]
            .iter()
            .map(|&d| frame(false, icon_edges(d).0).0)
            .collect();
        assert_eq!(small, [16, 20, 24, 32, 32]);
        // The taskbar button: 24 px at 100 percent, 48 at 200.
        let big: Vec<u32> = [96, 120, 144, 192]
            .iter()
            .map(|&d| frame(false, icon_edges(d).1).0)
            .collect();
        assert_eq!(big, [24, 32, 48, 48]);
        assert_eq!(frame(false, 300).0, 48);
        assert_ne!(frame(true, 16).1, frame(false, 16).1);
        assert!(std::ptr::eq(frame(true, 20).1, LIGHT[1]));
        assert!(std::ptr::eq(frame(false, 20).1, DARK[1]));
    }

    #[test]
    fn every_frame_decodes_at_its_size() {
        for (set, frames) in [("light", &LIGHT), ("dark", &DARK)] {
            for (png, size) in frames.iter().zip(FRAMES) {
                let (rgba, w, h) = decode_rgba(png).unwrap_or_else(|e| panic!("{set} {size}: {e}"));
                assert_eq!((w, h), (size, size), "{set}");
                assert_eq!(rgba.len(), (size * size * 4) as usize, "{set} {size}");
                // An opaque tile in the middle.
                let mid = ((size / 2 * size + size / 2) * 4 + 3) as usize;
                assert_eq!(rgba[mid], 255, "{set} {size}");
            }
        }
    }
}
