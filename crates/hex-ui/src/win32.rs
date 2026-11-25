//! Win32 window handle lookup, lobby auto-docking, in-game overlay transparency, and Alt hotkey polling.

/// Logical sidebar width in DIPs (`code_artifact.md` Section 3.1).
pub const SIDEBAR_WIDTH_DIP: f32 = 320.0;

/// Screen-space window geometry and DPI metadata for a target client window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowRectInfo {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub dpi: u32,
}

impl WindowRectInfo {
    /// Returns the window width in physical pixels.
    pub fn width(self) -> i32 {
        (self.right - self.left).max(0)
    }

    /// Returns the window height in physical pixels.
    pub fn height(self) -> i32 {
        (self.bottom - self.top).max(0)
    }

    /// Returns the DPI scale factor (`1.0` at 96 DPI).
    #[allow(dead_code)]
    pub fn scale(self) -> f32 {
        if self.dpi == 0 {
            1.0
        } else {
            self.dpi as f32 / 96.0
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::path::Path;
    use std::process::Command;

    use anyhow::{Context, Result};
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_MENU};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect,
        GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetWindowLongPtrW, SetWindowPos,
        ShowWindow, SystemParametersInfoW, GWL_EXSTYLE, HWND_TOPMOST, SM_CXVIRTUALSCREEN,
        SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SPI_GETWORKAREA, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE,
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE, WS_EX_APPWINDOW, WS_EX_LAYERED,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    };

    use super::{WindowRectInfo, SIDEBAR_WIDTH_DIP};

    const MIN_TARGET_WINDOW_DIM: i32 = 200;

    fn extract_hwnd(window: &Window) -> Result<HWND> {
        let handle = HasWindowHandle::window_handle(window)
            .map_err(|e| anyhow::anyhow!("failed to obtain raw window handle from GPUI window: {e:?}"))?;

        match handle.as_raw() {
            RawWindowHandle::Win32(win32) => {
                Ok(HWND(win32.hwnd.get() as *mut core::ffi::c_void))
            }
            other => anyhow::bail!("expected Win32 raw window handle, got {other:?}"),
        }
    }

    struct EnumTargetState {
        target_pid: u32,
        best_area: i64,
        best_rect: Option<WindowRectInfo>,
    }

    unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam.0 as *mut EnumTargetState);

        let mut win_pid: u32 = 0;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut win_pid));
        if win_pid != state.target_pid {
            return BOOL::from(true);
        }

        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return BOOL::from(true);
        }

        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return BOOL::from(true);
        }

        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width < MIN_TARGET_WINDOW_DIM || height < MIN_TARGET_WINDOW_DIM {
            return BOOL::from(true);
        }

        let area = i64::from(width) * i64::from(height);
        if area > state.best_area {
            let dpi = GetDpiForWindow(hwnd);
            state.best_area = area;
            state.best_rect = Some(WindowRectInfo {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
                dpi: if dpi == 0 { 96 } else { dpi },
            });
        }

        BOOL::from(true)
    }

    /// Finds the largest visible, non-minimized top-level window belonging to `pid`.
    pub fn find_window_by_pid(pid: u32) -> Option<WindowRectInfo> {
        let mut state = EnumTargetState {
            target_pid: pid,
            best_area: 0,
            best_rect: None,
        };

        // SAFETY: `enum_windows_proc` only reads `state` through the stack-pinned `LPARAM`
        // pointer for the synchronous duration of `EnumWindows`.
        unsafe {
            let _ = EnumWindows(
                Some(enum_windows_proc),
                LPARAM((&raw mut state) as isize),
            );
        }

        state.best_rect
    }

    fn clamp_bounds_for_target(target: WindowRectInfo) -> RECT {
        let mut work_rect = RECT::default();
        // SAFETY: `work_rect` is a valid out-pointer to `RECT`.
        let has_work = unsafe {
            SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some((&raw mut work_rect).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
            .is_ok()
        };

        let center_x = target.left + target.width() / 2;
        let center_y = target.top + target.height() / 2;
        if has_work
            && center_x >= work_rect.left
            && center_x <= work_rect.right
            && center_y >= work_rect.top
            && center_y <= work_rect.bottom
        {
            return work_rect;
        }

        // SAFETY: `GetSystemMetrics` reads virtual desktop bounds without side effects.
        unsafe {
            let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
            if vw > 0 && vh > 0 {
                RECT {
                    left: vx,
                    top: vy,
                    right: vx + vw,
                    bottom: vy + vh,
                }
            } else if has_work {
                work_rect
            } else {
                RECT {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                }
            }
        }
    }

    /// Returns `true` while the Alt key (`VK_MENU`) is currently held down (compliant polling, zero hooks).
    pub fn is_alt_hotkey_pressed() -> bool {
        // SAFETY: `GetAsyncKeyState` is a standard stateless Win32 key state query.
        unsafe { GetAsyncKeyState(i32::from(VK_MENU.0)) < 0 }
    }

    /// Applies topmost, layered, click-through, and tool-window extended styles for in-game overlay mode.
    #[allow(dead_code)]
    pub fn apply_overlay_styles(window: &Window) -> Result<()> {
        let hwnd = extract_hwnd(window)?;

        // SAFETY: `hwnd` belongs to the live GPUI window on the UI thread; we only update
        // standard extended window styles, Z-order, and non-activating visibility.
        unsafe {
            let current_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let overlay_style = (WINDOW_EX_STYLE(current_style as u32) & !WS_EX_APPWINDOW)
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

            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }

        Ok(())
    }

    /// Aligns the full-window transparent overlay over `League of Legends.exe` and toggles click-through.
    pub fn set_in_game_overlay_mode(
        window: &Window,
        target_rect: Option<WindowRectInfo>,
        interactive: bool,
        visible: bool,
    ) -> Result<()> {
        let hwnd = extract_hwnd(window)?;

        // SAFETY: `hwnd` belongs to the active GPUI window on the UI thread.
        unsafe {
            let current = WINDOW_EX_STYLE(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32);
            let base = (current & !WS_EX_APPWINDOW)
                | WS_EX_TOPMOST
                | WS_EX_LAYERED
                | WS_EX_TOOLWINDOW;
            let next = if interactive {
                base & !WS_EX_TRANSPARENT
            } else {
                base | WS_EX_TRANSPARENT
            };

            if next != current {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next.0 as isize);
            }

            let Some(target) = target_rect.filter(|_| visible) else {
                let _ = ShowWindow(hwnd, SW_HIDE);
                return Ok(());
            };

            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                target.left,
                target.top,
                target.width(),
                target.height(),
                SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
            .context("SetWindowPos(in-game overlay) failed")?;

            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }

        Ok(())
    }

    /// Configures interactive Champ Select sidebar mode docked beside `LeagueClientUx.exe`.
    pub fn set_lobby_dock_mode(window: &Window, target_rect: Option<WindowRectInfo>) -> Result<()> {
        let hwnd = extract_hwnd(window)?;

        // SAFETY: `hwnd` is owned by the active GPUI window on the UI thread.
        unsafe {
            let current = WINDOW_EX_STYLE(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32);
            let next = (current & !WS_EX_TRANSPARENT & !WS_EX_APPWINDOW)
                | WS_EX_TOPMOST
                | WS_EX_TOOLWINDOW;

            if next != current {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next.0 as isize);
            }

            let Some(target) = target_rect else {
                let _ = ShowWindow(hwnd, SW_HIDE);
                return Ok(());
            };

            let own_dpi = GetDpiForWindow(hwnd);
            let dpi = if own_dpi > 0 { own_dpi } else { target.dpi.max(96) };
            let scale = dpi as f32 / 96.0;

            let width_px = (SIDEBAR_WIDTH_DIP * scale).round() as i32;
            let gap_px = (4.0 * scale).round() as i32;
            let min_height_px = (420.0 * scale).round() as i32;

            let bounds = clamp_bounds_for_target(target);
            let max_width = (bounds.right - bounds.left).max(200);
            let width_px = width_px.min(max_width);

            let mut x = target.right + gap_px;
            if x + width_px > bounds.right {
                let left_dock = target.left - width_px - gap_px;
                if left_dock >= bounds.left {
                    x = left_dock;
                } else {
                    x = (bounds.right - width_px).max(bounds.left);
                }
            }

            let y = target
                .top
                .clamp(bounds.top, (bounds.bottom - 200).max(bounds.top));
            let avail_h = (bounds.bottom - y).max(200);
            let height_px = target.height().max(min_height_px).min(avail_h);

            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                width_px,
                height_px,
                SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
            .context("SetWindowPos(lobby dock) failed")?;

            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }

        Ok(())
    }

    /// Hides the overlay window without destroying its GPUI entity state.
    pub fn hide_window(window: &Window) -> Result<()> {
        let hwnd = extract_hwnd(window)?;
        // SAFETY: `hwnd` is a valid top-level GPUI window on the UI thread.
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
        Ok(())
    }

    /// Shows the overlay window without stealing keyboard focus.
    #[allow(dead_code)]
    pub fn show_window_no_activate(window: &Window) -> Result<()> {
        let hwnd = extract_hwnd(window)?;
        // SAFETY: `hwnd` is a valid top-level GPUI window on the UI thread.
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        Ok(())
    }

    /// Opens the local data directory in Windows Explorer.
    pub fn open_data_directory(path: &Path) -> Result<()> {
        let _ = std::fs::create_dir_all(path);
        Command::new("explorer.exe")
            .arg(path)
            .spawn()
            .with_context(|| format!("failed to open data directory {}", path.display()))?;
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use std::path::Path;

    use anyhow::Result;
    use gpui::Window;

    use super::WindowRectInfo;

    /// Non-Windows stub for `is_alt_hotkey_pressed`.
    pub fn is_alt_hotkey_pressed() -> bool {
        false
    }

    /// Non-Windows stub for `apply_overlay_styles`.
    #[allow(dead_code)]
    pub fn apply_overlay_styles(_window: &Window) -> Result<()> {
        Ok(())
    }

    /// Non-Windows stub for `set_in_game_overlay_mode`.
    pub fn set_in_game_overlay_mode(
        _window: &Window,
        _target_rect: Option<WindowRectInfo>,
        _interactive: bool,
        _visible: bool,
    ) -> Result<()> {
        Ok(())
    }

    /// Non-Windows stub for `find_window_by_pid`.
    pub fn find_window_by_pid(_pid: u32) -> Option<WindowRectInfo> {
        None
    }

    /// Non-Windows stub for `set_lobby_dock_mode`.
    pub fn set_lobby_dock_mode(_window: &Window, _target_rect: Option<WindowRectInfo>) -> Result<()> {
        Ok(())
    }

    /// Non-Windows stub for `hide_window`.
    pub fn hide_window(_window: &Window) -> Result<()> {
        Ok(())
    }

    /// Non-Windows stub for `show_window_no_activate`.
    #[allow(dead_code)]
    pub fn show_window_no_activate(_window: &Window) -> Result<()> {
        Ok(())
    }

    /// Non-Windows stub for `open_data_directory`.
    pub fn open_data_directory(path: &Path) -> Result<()> {
        tracing::info!(path = %path.display(), "open_data_directory requested on non-Windows");
        Ok(())
    }
}

pub use imp::{
    find_window_by_pid, hide_window, is_alt_hotkey_pressed, open_data_directory,
    set_in_game_overlay_mode, set_lobby_dock_mode,
};
