//! Window and tray icons at the sizes Windows draws them, from the
//! matching frames of `icons/icon.ico` (16, 20, 24, 32, 48, ... px; the
//! 16, 20 and 24 px frames are drawn on the pixel grid, `branding/icon-sizes`).
//!
//! Tauri gives the window one icon, loaded at the system's large-icon size
//! before the process is DPI-aware (32 px), as the small icon only; the
//! taskbar then scales that 32 px image to its button (48 px at 200
//! percent) and the title bar and tray scale it to theirs. Here the window
//! gets a small icon (title bar, the tray's size) and a big one (the taskbar
//! button, which Windows 11 takes from the window's big icon while the app
//! runs), both loaded from the exe's icon resource for the window's DPI with
//! `LoadIconWithScaleDown`, and the tray an image of the small size for the
//! taskbar's DPI. Both are redone when the window's scale changes.

/// Logical sizes: the small icon (title bar, notification area) and the
/// taskbar button's icon.
const SMALL: u32 = 16;
const TASKBAR: u32 = 24;

/// `(small, taskbar)` edge lengths in pixels for a DPI (96 = 100 percent):
/// 16 and 24 at 100 percent, 20 and 30 at 125, 24 and 36 at 150, 32 and 48
/// at 200.
pub(crate) fn icon_edges(dpi: u32) -> (u32, u32) {
    let dpi = if dpi == 0 { 96 } else { dpi };
    let at = |logical: u32| (logical * dpi + 48) / 96;
    (at(SMALL), at(TASKBAR))
}

#[cfg(windows)]
mod win {
    use std::sync::{Mutex, OnceLock};

    use windows::core::{s, w, HRESULT, PCWSTR};
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
    use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
    use windows::Win32::UI::WindowsAndMessaging::{
        DestroyIcon, FindWindowW, LoadImageW, SendMessageW, HICON, ICON_BIG, ICON_SMALL,
        IMAGE_ICON, LR_DEFAULTCOLOR, WM_SETICON,
    };

    type ScaleDown = unsafe extern "system" fn(HINSTANCE, PCWSTR, i32, i32, *mut HICON) -> HRESULT;

    /// `LoadIconWithScaleDown`, from the Common Controls 6 the app's
    /// manifest asks for. Looked up at run time: the comctl32 that a
    /// process without that manifest gets (a test binary) does not have it.
    fn scale_down() -> Option<ScaleDown> {
        static F: OnceLock<Option<usize>> = OnceLock::new();
        let f = *F.get_or_init(|| unsafe {
            let module = LoadLibraryW(w!("comctl32.dll")).ok()?;
            GetProcAddress(module, s!("LoadIconWithScaleDown")).map(|f| f as usize)
        });
        // SAFETY: the address of LoadIconWithScaleDown, whose signature is `ScaleDown`.
        f.map(|f| unsafe { std::mem::transmute::<usize, ScaleDown>(f) })
    }

    /// The app icon (the exe's icon resource) at `size` px: the frame of
    /// that size, else a larger one scaled down.
    pub(super) fn load_app_icon(size: u32) -> Option<HICON> {
        let size = size as i32;
        unsafe {
            let hinst: HINSTANCE = GetModuleHandleW(PCWSTR::null()).ok()?.into();
            let id =
                PCWSTR(tauri::utils::platform::WINDOWS_APP_ICON_RESOURCE_ID as usize as *const u16);
            if let Some(f) = scale_down() {
                let mut icon = HICON::default();
                if f(hinst, id, size, size, &mut icon).is_ok() && !icon.is_invalid() {
                    return Some(icon);
                }
            }
            LoadImageW(Some(hinst), id, IMAGE_ICON, size, size, LR_DEFAULTCOLOR)
                .ok()
                .map(|h| HICON(h.0))
        }
    }

    /// The icons this module set last (small, big), destroyed when replaced.
    static SET: Mutex<Option<(isize, isize)>> = Mutex::new(None);

    pub(super) fn apply(hwnd: HWND) -> Option<(u32, u32, u32)> {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let (small, big) = super::icon_edges(dpi);
        let (s, b) = (load_app_icon(small)?, load_app_icon(big)?);
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
        Some((dpi, small, big))
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

/// Sets the window's small and big icons for its DPI (Windows; elsewhere
/// the default icon stays).
pub(crate) fn apply_window_icons<R: tauri::Runtime>(window: &tauri::Window<R>) {
    #[cfg(windows)]
    {
        let Ok(hwnd) = window.hwnd() else {
            return;
        };
        match win::apply(hwnd) {
            Some((dpi, small, big)) => {
                log::info!("window icons {small} and {big} px for {dpi} dpi")
            }
            None => log::warn!("window icons not loaded from the app icon resource"),
        }
    }
    #[cfg(not(windows))]
    let _ = window;
}

/// The tray's image: the app icon at the notification area's size for the
/// taskbar's DPI (Windows), else the default window icon.
pub(crate) fn tray_image<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Option<tauri::image::Image<'static>> {
    #[cfg(windows)]
    {
        let (small, _) = icon_edges(win::taskbar_dpi());
        match tauri::image::Image::from_icon_resource(
            tauri::utils::platform::WINDOWS_APP_ICON_RESOURCE_ID,
            small,
            small,
        ) {
            Ok(i) => return Some(i),
            Err(e) => log::warn!("tray icon at {small} px not loaded: {e}"),
        }
    }
    app.default_window_icon()
        .cloned()
        .map(tauri::image::Image::to_owned)
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
}
