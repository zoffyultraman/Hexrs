//! Win32 window handle extraction and overlay style helpers.

#[cfg(target_os = "windows")]
mod imp {
    use anyhow::{Context, Result};
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, HWND_TOPMOST,
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WINDOW_EX_STYLE,
        WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    };

    /// Extracts the native Win32 `HWND` from a GPUI `Window`.
    fn extract_hwnd(window: &Window) -> Result<HWND> {
        let handle = HasWindowHandle::window_handle(window)
            .context("failed to obtain raw window handle from GPUI window")?;

        match handle.as_raw() {
            RawWindowHandle::Win32(win32) => {
                Ok(HWND(win32.hwnd.get() as *mut core::ffi::c_void))
            }
            other => anyhow::bail!("expected Win32 raw window handle, got {other:?}"),
        }
    }

    /// Applies topmost, layered, click-through, and tool-window extended styles to the overlay.
    pub fn apply_overlay_styles(window: &Window) -> Result<()> {
        let hwnd = extract_hwnd(window)?;

        // SAFETY: `hwnd` belongs to the live GPUI window on the UI thread; we only update
        // standard extended window styles and Z-order flags without injecting or hooking.
        unsafe {
            let current_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let overlay_style = WINDOW_EX_STYLE(current_style as u32)
                | WS_EX_TOPMOST
                | WS_EX_LAYERED
                | WS_EX_TRANSPARENT
                | WS_EX_TOOLWINDOW;

            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, overlay_style.0 as isize);

            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
            .context("SetWindowPos(HWND_TOPMOST) failed")?;
        }

        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use anyhow::Result;
    use gpui::Window;

    /// Non-Windows stub for `apply_overlay_styles`.
    pub fn apply_overlay_styles(_window: &Window) -> Result<()> {
        Ok(())
    }
}

pub use imp::apply_overlay_styles;
