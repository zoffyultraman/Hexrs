//! Win32 window handle lookup (`RCLIENT`/`RiotWindowClass`/PID/title), DirectComposition transparency,
//! lobby auto-docking, Alt hotkey polling, and diagnostics window management.

use hex_core::AugmentOfferLayout;

/// Logical sidebar width in DIPs (`code_artifact.md` Section 3.1).
pub const SIDEBAR_WIDTH_DIP: f32 = 320.0;

/// Logical diagnostics floating panel width and height in DIPs.
pub const DIAGNOSTICS_WIDTH_DIP: f32 = 440.0;
pub const DIAGNOSTICS_HEIGHT_DIP: f32 = 560.0;

/// Compact in-game corner HUD dimensions in DIPs.
pub const IN_GAME_HUD_WIDTH_DIP: f32 = 324.0;
pub const IN_GAME_HUD_HEIGHT_DIP: f32 = 204.0;

const CARD_WIDTH_RATIO: f32 = 0.86;
const CARD_GAP_RATIO: f32 = 0.14;
const STRIP_TOTAL_RATIO: f32 = 2.86;
const AUGMENT_BAR_HEIGHT_DIP: f32 = 94.0;

/// Pixel-calibrated Champ Select bench strip geometry relative to `LeagueClientUx.exe`.
pub const BENCH_SLOT0_CX: f32 = 0.2912;
pub const BENCH_SLOT_PITCH_FRAC: f32 = 0.0460;
pub const BENCH_SLOT_WIDTH_FRAC: f32 = 0.0400;
pub const BENCH_SLOT_HEIGHT_FRAC: f32 = 0.0285;
pub const BENCH_STRIP_LEFT_FRAC: f32 = BENCH_SLOT0_CX - BENCH_SLOT_WIDTH_FRAC * 0.5;

/// Corner anchor for the compact in-game HUD card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayCorner {
    #[default]
    TopLeft,
    TopRight,
}

/// Deferred Win32 window repositioning or visibility command executed after GPUI releases `Window`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Win32WindowAction {
    Hide,
    Diagnostics,
    LobbyDock(Option<WindowRectInfo>),
    BenchBadgeStrip {
        target_rect: Option<WindowRectInfo>,
        slot_count: usize,
    },
    TeamBadgeStrip {
        target_rect: Option<WindowRectInfo>,
    },
    InGameOverlay {
        target_rect: Option<WindowRectInfo>,
        corner: OverlayCorner,
        visible: bool,
    },
    InGameAugmentBar {
        target_rect: Option<WindowRectInfo>,
        visible: bool,
        layout: Option<AugmentOfferLayout>,
    },
}

/// Deferred Win32 command targeting a specific `HWND` outside GPUI's re-entrant `Window` lock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Win32WindowCommand {
    pub hwnd_raw: isize,
    pub action: Win32WindowAction,
}

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
    pub fn scale(self) -> f32 {
        if self.dpi == 0 {
            1.0
        } else {
            self.dpi as f32 / 96.0
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct AugmentBarGeometry {
    x: i32,
    y: i32,
    width_px: i32,
    height_px: i32,
    gap_dip: f32,
}

fn compute_augment_bar_geometry(
    target: WindowRectInfo,
    scale: f32,
    layout: Option<AugmentOfferLayout>,
) -> AugmentBarGeometry {
    let scale = if scale <= 0.0 { 1.0 } else { scale };
    let target_w = target.width().max(320);
    let target_h = target.height().max(90);
    let height_px = ((AUGMENT_BAR_HEIGHT_DIP * scale).round() as i32).min(target_h);

    let (raw_x, raw_y, width_px) = match layout {
        Some(l) if l.slot_centers_x[2] > l.slot_centers_x[0] + 0.16 => {
            let cx0 = l.slot_centers_x[0];
            let cx2 = l.slot_centers_x[2];
            let spacing_norm = ((cx2 - cx0) * 0.5).clamp(0.14, 0.36);
            let spacing_px = spacing_norm * target_w as f32;
            let strip_left_px = target.left
                + ((cx0 - spacing_norm * (CARD_WIDTH_RATIO * 0.5)) * target_w as f32).round()
                    as i32;
            let min_w = (360.0 * scale).round() as i32;
            let width_px = (spacing_px * STRIP_TOTAL_RATIO)
                .round()
                .clamp(min_w.min(target_w) as f32, target_w as f32)
                as i32;
            let card_top_px =
                target.top + (l.title_y * target_h as f32 - spacing_px * 0.69).round() as i32;
            let v_gap_px = (8.0 * scale).round() as i32;
            let y = (card_top_px - height_px - v_gap_px).clamp(
                target.top + (4.0 * scale).round() as i32,
                (target.bottom - height_px).max(target.top),
            );
            (strip_left_px, y, width_px)
        }
        _ => {
            let spacing_norm = if target_w as f32 / target_h.max(1) as f32 >= 2.05 {
                0.165
            } else {
                0.225
            };
            let min_w = (480.0 * scale).round() as i32;
            let width_px = ((target_w as f32 * spacing_norm * STRIP_TOTAL_RATIO).round() as i32)
                .clamp(min_w.min(target_w), target_w);
            let x = target.left + (target_w - width_px) / 2;
            let y = target.top + (target_h as f32 * 0.08).round() as i32;
            (x, y, width_px)
        }
    };

    let x = raw_x.clamp(target.left, (target.right - width_px).max(target.left));
    let y = raw_y.clamp(target.top, (target.bottom - height_px).max(target.top));
    let gap_dip =
        ((width_px as f32 / scale) * (CARD_GAP_RATIO / STRIP_TOTAL_RATIO)).clamp(8.0, 160.0);

    AugmentBarGeometry {
        x,
        y,
        width_px,
        height_px,
        gap_dip,
    }
}

/// Computes the exact DIP flex gap matching the aligned 3-card augment strip geometry.
pub fn compute_augment_bar_gap_dip(
    target: Option<WindowRectInfo>,
    layout: Option<AugmentOfferLayout>,
) -> f32 {
    let Some(rect) = target else {
        return 28.0;
    };
    compute_augment_bar_geometry(rect, rect.scale(), layout).gap_dip
}

#[cfg(target_os = "windows")]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::process::Command;

    use anyhow::{Context, Result};
    use gpui::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::core::{w, BOOL, PCWSTR};
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::InvalidateRect;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowW, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect,
        GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow,
        SystemParametersInfoW, GWL_EXSTYLE, HWND_TOPMOST, LWA_ALPHA, SM_CXVIRTUALSCREEN,
        SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SPI_GETWORKAREA, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE, SW_SHOWNORMAL,
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE, WS_EX_APPWINDOW, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_EX_TRANSPARENT,
    };

    use super::{
        compute_augment_bar_geometry, AugmentOfferLayout, OverlayCorner, Win32WindowAction,
        Win32WindowCommand, WindowRectInfo, BENCH_SLOT_PITCH_FRAC, BENCH_SLOT_WIDTH_FRAC,
        BENCH_STRIP_LEFT_FRAC, DIAGNOSTICS_HEIGHT_DIP, DIAGNOSTICS_WIDTH_DIP,
        IN_GAME_HUD_HEIGHT_DIP, IN_GAME_HUD_WIDTH_DIP, SIDEBAR_WIDTH_DIP,
    };

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

    /// Extracts the raw Win32 `HWND` handle value for deferred window commands.
    pub fn window_hwnd_raw(window: &Window) -> Result<isize> {
        extract_hwnd(window).map(|hwnd| hwnd.0 as isize)
    }

    /// Returns the live DPI scale factor for `hwnd_raw`, falling back to `fallback` if unavailable.
    pub fn window_scale_for_hwnd(hwnd_raw: isize, fallback: f32) -> f32 {
        if hwnd_raw == 0 {
            return fallback;
        }
        let hwnd = HWND(hwnd_raw as *mut core::ffi::c_void);
        // SAFETY: `hwnd` belongs to the active GPUI window on the UI thread.
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        if dpi > 0 {
            dpi as f32 / 96.0
        } else {
            fallback
        }
    }

    unsafe fn inspect_window_rect(hwnd: HWND, min_w: i32, min_h: i32) -> Option<WindowRectInfo> {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }

        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return None;
        }

        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width < min_w || height < min_h {
            return None;
        }

        let dpi = GetDpiForWindow(hwnd);
        Some(WindowRectInfo {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
            dpi: if dpi == 0 { 96 } else { dpi },
        })
    }

    unsafe fn read_window_title(hwnd: HWND) -> String {
        let mut buf = [0_u16; 256];
        let len = GetWindowTextW(hwnd, &mut buf);
        if len <= 0 {
            String::new()
        } else {
            String::from_utf16_lossy(&buf[..len as usize])
        }
    }

    struct EnumPidState {
        target_pid: u32,
        best_area: i64,
        best_rect: Option<WindowRectInfo>,
    }

    unsafe extern "system" fn enum_pid_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam.0 as *mut EnumPidState);

        let mut win_pid: u32 = 0;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut win_pid));
        if win_pid != state.target_pid {
            return BOOL::from(true);
        }

        if let Some(info) = inspect_window_rect(hwnd, MIN_TARGET_WINDOW_DIM, MIN_TARGET_WINDOW_DIM) {
            let area = i64::from(info.width()) * i64::from(info.height());
            if area > state.best_area {
                state.best_area = area;
                state.best_rect = Some(info);
            }
        }

        BOOL::from(true)
    }

    /// Finds the largest visible, non-minimized top-level window belonging to `pid`.
    pub fn find_window_by_pid(pid: u32) -> Option<WindowRectInfo> {
        let mut state = EnumPidState {
            target_pid: pid,
            best_area: 0,
            best_rect: None,
        };

        // SAFETY: `enum_pid_windows_proc` only accesses `state` for the synchronous `EnumWindows` call.
        unsafe {
            let _ = EnumWindows(
                Some(enum_pid_windows_proc),
                LPARAM((&raw mut state) as isize),
            );
        }

        state.best_rect
    }

    #[derive(Clone, Copy)]
    enum TitleMatchKind {
        Lobby,
        Game,
    }

    struct EnumTitleState {
        kind: TitleMatchKind,
        best_area: i64,
        best_rect: Option<WindowRectInfo>,
    }

    unsafe extern "system" fn enum_title_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam.0 as *mut EnumTitleState);

        let (min_w, min_h) = match state.kind {
            TitleMatchKind::Lobby => (800, 500),
            TitleMatchKind::Game => (640, 480),
        };

        let Some(info) = inspect_window_rect(hwnd, min_w, min_h) else {
            return BOOL::from(true);
        };

        let title = read_window_title(hwnd);
        if title.is_empty() {
            return BOOL::from(true);
        }

        let matched = match state.kind {
            TitleMatchKind::Lobby => {
                (title.contains("League of Legends") || title.contains("英雄联盟"))
                    && !title.contains("Client")
                    && !title.contains("(TM)")
            }
            TitleMatchKind::Game => {
                title.contains("League of Legends (TM) Client")
                    || title.contains("League of Legends (TM)")
            }
        };

        if matched {
            let area = i64::from(info.width()) * i64::from(info.height());
            if area > state.best_area {
                state.best_area = area;
                state.best_rect = Some(info);
            }
        }

        BOOL::from(true)
    }

    /// Finds the League client lobby window via `RCLIENT` class, PID fallback, or title match.
    pub fn find_league_lobby_window(pid: Option<u32>) -> Option<WindowRectInfo> {
        // SAFETY: `FindWindowW` and `inspect_window_rect` only inspect top-level window handles.
        unsafe {
            if let Ok(hwnd) = FindWindowW(w!("RCLIENT"), PCWSTR::null()) {
                if let Some(info) = inspect_window_rect(hwnd, 600, 400) {
                    return Some(info);
                }
            }
        }

        if let Some(info) = pid.and_then(find_window_by_pid) {
            if info.width() >= 600 && info.height() >= 400 {
                return Some(info);
            }
        }

        let mut state = EnumTitleState {
            kind: TitleMatchKind::Lobby,
            best_area: 0,
            best_rect: None,
        };
        // SAFETY: `enum_title_windows_proc` accesses `state` synchronously during `EnumWindows`.
        unsafe {
            let _ = EnumWindows(
                Some(enum_title_windows_proc),
                LPARAM((&raw mut state) as isize),
            );
        }
        state.best_rect
    }

    /// Finds the active in-game League window via `RiotWindowClass`, PID fallback, or title match.
    pub fn find_league_game_window(pid: Option<u32>) -> Option<WindowRectInfo> {
        // SAFETY: `FindWindowW` and `inspect_window_rect` only inspect top-level window handles.
        unsafe {
            if let Ok(hwnd) = FindWindowW(w!("RiotWindowClass"), PCWSTR::null()) {
                if let Some(info) = inspect_window_rect(hwnd, 640, 480) {
                    return Some(info);
                }
            }
        }

        if let Some(info) = pid.and_then(find_window_by_pid) {
            if info.width() >= 640 && info.height() >= 480 {
                return Some(info);
            }
        }

        let mut state = EnumTitleState {
            kind: TitleMatchKind::Game,
            best_area: 0,
            best_rect: None,
        };
        // SAFETY: `enum_title_windows_proc` accesses `state` synchronously during `EnumWindows`.
        unsafe {
            let _ = EnumWindows(
                Some(enum_title_windows_proc),
                LPARAM((&raw mut state) as isize),
            );
        }
        state.best_rect
    }

    fn primary_work_area() -> RECT {
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
        if has_work {
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

    fn clamp_bounds_for_target(target: WindowRectInfo) -> RECT {
        let work_rect = primary_work_area();
        let center_x = target.left + target.width() / 2;
        let center_y = target.top + target.height() / 2;
        if center_x >= work_rect.left
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
            } else {
                work_rect
            }
        }
    }

    /// Returns a work-area rect suitable for previewing UI modes when League is not running.
    pub fn default_preview_lobby_rect() -> WindowRectInfo {
        let work = primary_work_area();
        let w = (work.right - work.left).min(1280);
        let h = (work.bottom - work.top).min(760);
        let left = work.left + ((work.right - work.left - w) / 2).max(340);
        let top = work.top + ((work.bottom - work.top - h) / 2).max(0);
        WindowRectInfo {
            left,
            top,
            right: (left + w - 340).min(work.right),
            bottom: top + h,
            dpi: 96,
        }
    }

    /// Returns a work-area rect suitable for previewing the corner HUD when League is not running.
    pub fn default_preview_game_rect() -> WindowRectInfo {
        let work = primary_work_area();
        WindowRectInfo {
            left: work.left,
            top: work.top,
            right: work.right,
            bottom: work.bottom,
            dpi: 96,
        }
    }

    unsafe fn apply_transparent_ex_style(hwnd: HWND, interactive: bool) {
        let current = WINDOW_EX_STYLE(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32);
        let uses_dcomp = current.contains(WS_EX_NOREDIRECTIONBITMAP);

        let mut next = if uses_dcomp {
            // DirectComposition (`WS_EX_NOREDIRECTIONBITMAP`) MUST NOT have `WS_EX_LAYERED`
            // set, otherwise Windows compositing turns the window surface invisible.
            (current & !WS_EX_LAYERED & !WS_EX_APPWINDOW)
                | WS_EX_NOREDIRECTIONBITMAP
                | WS_EX_TOPMOST
                | WS_EX_TOOLWINDOW
                | WS_EX_NOACTIVATE
        } else {
            (current & !WS_EX_APPWINDOW)
                | WS_EX_LAYERED
                | WS_EX_TOPMOST
                | WS_EX_TOOLWINDOW
                | WS_EX_NOACTIVATE
        };

        next = if interactive {
            next & !WS_EX_TRANSPARENT
        } else {
            next | WS_EX_TRANSPARENT
        };

        if next != current {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next.0 as isize);
        }

        if !uses_dcomp {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        }
    }

    /// Applies topmost, click-through, and tool-window extended styles (DirectComposition-safe).
    #[allow(dead_code)]
    pub fn apply_overlay_styles(window: &Window) -> Result<()> {
        let hwnd = extract_hwnd(window)?;

        // SAFETY: `hwnd` belongs to the live GPUI window on the UI thread.
        unsafe {
            apply_transparent_ex_style(hwnd, false);

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
            let _ = InvalidateRect(Some(hwnd), None, false);
        }

        Ok(())
    }

    /// Executes a deferred Win32 window move/resize/show command outside GPUI's `Window` borrow.
    pub fn apply_window_command(cmd: Win32WindowCommand) -> Result<()> {
        let hwnd = HWND(cmd.hwnd_raw as *mut core::ffi::c_void);
        match cmd.action {
            Win32WindowAction::Hide => hide_window_hwnd(hwnd),
            Win32WindowAction::Diagnostics => set_diagnostics_window_mode_hwnd(hwnd),
            Win32WindowAction::LobbyDock(target_rect) => {
                set_lobby_dock_mode_hwnd(hwnd, target_rect)
            }
            Win32WindowAction::BenchBadgeStrip {
                target_rect,
                slot_count,
            } => set_bench_badge_strip_hwnd(hwnd, target_rect, slot_count),
            Win32WindowAction::TeamBadgeStrip { target_rect } => {
                set_team_badge_strip_hwnd(hwnd, target_rect)
            }
            Win32WindowAction::InGameOverlay {
                target_rect,
                corner,
                visible,
            } => set_in_game_overlay_mode_hwnd(hwnd, target_rect, corner, visible),
            Win32WindowAction::InGameAugmentBar {
                target_rect,
                visible,
                layout,
            } => set_in_game_augment_bar_mode_hwnd(hwnd, target_rect, visible, layout),
        }
    }

    /// Anchors the compact `324x204` click-through corner HUD onto `League of Legends.exe`.
    fn set_in_game_overlay_mode_hwnd(
        hwnd: HWND,
        target_rect: Option<WindowRectInfo>,
        corner: OverlayCorner,
        visible: bool,
    ) -> Result<()> {

        // SAFETY: `hwnd` belongs to the active GPUI window on the UI thread.
        unsafe {
            apply_transparent_ex_style(hwnd, false);

            let Some(target) = target_rect.filter(|_| visible) else {
                let _ = ShowWindow(hwnd, SW_HIDE);
                return Ok(());
            };

            let own_dpi = GetDpiForWindow(hwnd);
            let dpi = if own_dpi > 0 { own_dpi } else { target.dpi.max(96) };
            let scale = dpi as f32 / 96.0;

            let target_w = target.width().max(160);
            let target_h = target.height().max(80);
            let width_px = ((IN_GAME_HUD_WIDTH_DIP * scale).round() as i32).min(target_w);
            let height_px = ((IN_GAME_HUD_HEIGHT_DIP * scale).round() as i32).min(target_h);
            let margin_x = (14.0 * scale).round() as i32;

            let (raw_x, raw_y) = match corner {
                OverlayCorner::TopLeft => (
                    target.left + margin_x,
                    target.top + (14.0 * scale).round() as i32,
                ),
                OverlayCorner::TopRight => (
                    target.right - width_px - margin_x,
                    target.top + (56.0 * scale).round() as i32,
                ),
            };

            let x = raw_x.clamp(target.left, (target.right - width_px).max(target.left));
            let y = raw_y.clamp(target.top, (target.bottom - height_px).max(target.top));

            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                width_px,
                height_px,
                SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
            .context("SetWindowPos(in-game corner HUD) failed")?;

            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }

        Ok(())
    }

    /// Anchors a permanently click-through horizontal strip strictly ABOVE the 3 augment cards.
    fn set_in_game_augment_bar_mode_hwnd(
        hwnd: HWND,
        target_rect: Option<WindowRectInfo>,
        visible: bool,
        layout: Option<AugmentOfferLayout>,
    ) -> Result<()> {
        // SAFETY: `hwnd` belongs to the active GPUI window on the UI thread.
        unsafe {
            apply_transparent_ex_style(hwnd, false);

            let Some(target) = target_rect.filter(|_| visible) else {
                let _ = ShowWindow(hwnd, SW_HIDE);
                return Ok(());
            };

            let own_dpi = GetDpiForWindow(hwnd);
            let dpi = if own_dpi > 0 { own_dpi } else { target.dpi.max(96) };
            let geom = compute_augment_bar_geometry(target, dpi as f32 / 96.0, layout);

            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                geom.x,
                geom.y,
                geom.width_px,
                geom.height_px,
                SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
            .context("SetWindowPos(in-game augment bar) failed")?;

            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }

        Ok(())
    }

    /// Anchors the narrow horizontal bench badge strip strictly below the 10 bench portraits.
    fn set_bench_badge_strip_hwnd(
        hwnd: HWND,
        target_rect: Option<WindowRectInfo>,
        slot_count: usize,
    ) -> Result<()> {
        // SAFETY: `hwnd` belongs to the active GPUI window on the UI thread.
        unsafe {
            apply_transparent_ex_style(hwnd, false);

            let Some(target) = target_rect.filter(|_| slot_count > 0) else {
                let _ = ShowWindow(hwnd, SW_HIDE);
                return Ok(());
            };

            let own_dpi = GetDpiForWindow(hwnd);
            let dpi = if own_dpi > 0 { own_dpi } else { target.dpi.max(96) };
            let scale = dpi as f32 / 96.0;

            let target_w = target.width().max(320);
            let target_h = target.height().max(200);
            let count = slot_count.clamp(1, 10);

            let x = (target.left + (BENCH_STRIP_LEFT_FRAC * target_w as f32).round() as i32)
                .clamp(target.left, target.right.saturating_sub(1));
            let max_w = (target.right - x).max(1);
            let raw_w = (((count - 1) as f32 * BENCH_SLOT_PITCH_FRAC + BENCH_SLOT_WIDTH_FRAC)
                * target_w as f32)
                .round() as i32
                + (8.0 * scale).round() as i32;
            let w = raw_w.max((40.0 * scale).round() as i32).min(max_w);

            let y = (target.top + (0.0835 * target_h as f32).round() as i32)
                .clamp(target.top, target.bottom.saturating_sub(1));
            let max_h = (target.bottom - y).max(1);
            let min_h = (18.0 * scale).round() as i32;
            let cap_h = (40.0 * scale).round() as i32;
            let h = ((0.0290 * target_h as f32).round() as i32)
                .clamp(min_h, cap_h)
                .min(max_h);

            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                w,
                h,
                SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
            .context("SetWindowPos(bench badge strip) failed")?;

            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }

        Ok(())
    }

    /// Anchors the narrow vertical allied badge strip strictly to the right of the 5 hero cards.
    fn set_team_badge_strip_hwnd(hwnd: HWND, target_rect: Option<WindowRectInfo>) -> Result<()> {
        // SAFETY: `hwnd` belongs to the active GPUI window on the UI thread.
        unsafe {
            apply_transparent_ex_style(hwnd, false);

            let Some(target) = target_rect else {
                let _ = ShowWindow(hwnd, SW_HIDE);
                return Ok(());
            };

            let own_dpi = GetDpiForWindow(hwnd);
            let dpi = if own_dpi > 0 { own_dpi } else { target.dpi.max(96) };
            let scale = dpi as f32 / 96.0;

            let target_w = target.width().max(320);
            let target_h = target.height().max(200);

            let x = (target.left + (0.2980 * target_w as f32).round() as i32)
                .clamp(target.left, target.right.saturating_sub(1));
            let max_w = (target.right - x).max(1);
            let w = ((target_w as f32 * 0.068).round() as i32)
                .clamp((104.0 * scale).round() as i32, (156.0 * scale).round() as i32)
                .min(max_w);

            let y = (target.top + (0.1261 * target_h as f32).round() as i32)
                .clamp(target.top, target.bottom.saturating_sub(1));
            let max_h = (target.bottom - y).max(1);
            let h = ((0.5647 * target_h as f32).round() as i32)
                .max((120.0 * scale).round() as i32)
                .min(max_h);

            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                w,
                h,
                SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
            .context("SetWindowPos(team badge strip) failed")?;

            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }

        Ok(())
    }

    /// Configures interactive Champ Select sidebar mode docked beside `LeagueClientUx.exe`.
    fn set_lobby_dock_mode_hwnd(hwnd: HWND, target_rect: Option<WindowRectInfo>) -> Result<()> {

        // SAFETY: `hwnd` is owned by the active GPUI window on the UI thread.
        unsafe {
            let current = WINDOW_EX_STYLE(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32);
            let next = (current & !WS_EX_TRANSPARENT & !WS_EX_LAYERED & !WS_EX_APPWINDOW)
                | WS_EX_TOPMOST
                | WS_EX_TOOLWINDOW
                | WS_EX_NOACTIVATE;

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

            let mut x = target.left - width_px - gap_px;
            if x < bounds.left {
                let right_dock = target.right + gap_px;
                x = if right_dock + width_px <= bounds.right {
                    right_dock
                } else {
                    bounds.left
                };
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
            let _ = InvalidateRect(Some(hwnd), None, false);
        }

        Ok(())
    }

    /// Positions an interactive floating diagnostics panel near the top-right of the primary work area.
    fn set_diagnostics_window_mode_hwnd(hwnd: HWND) -> Result<()> {

        // SAFETY: `hwnd` belongs to the active GPUI window on the UI thread.
        unsafe {
            let current = WINDOW_EX_STYLE(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32);
            let next = (current & !WS_EX_TRANSPARENT & !WS_EX_LAYERED & !WS_EX_APPWINDOW)
                | WS_EX_TOPMOST
                | WS_EX_TOOLWINDOW;

            if next != current {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next.0 as isize);
            }

            let dpi = GetDpiForWindow(hwnd);
            let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
            let width_px = (DIAGNOSTICS_WIDTH_DIP * scale).round() as i32;
            let height_px = (DIAGNOSTICS_HEIGHT_DIP * scale).round() as i32;
            let margin_px = (24.0 * scale).round() as i32;

            let work = primary_work_area();
            let x = (work.right - width_px - margin_px).max(work.left);
            let y = (work.top + margin_px).min((work.bottom - height_px).max(work.top));
            let final_w = width_px.min((work.right - work.left).max(240));
            let final_h = height_px.min((work.bottom - work.top).max(240));

            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                final_w,
                final_h,
                SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
            .context("SetWindowPos(diagnostics) failed")?;

            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }

        Ok(())
    }

    /// Hides the overlay window without destroying its GPUI entity state.
    fn hide_window_hwnd(hwnd: HWND) -> Result<()> {
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

    /// Opens the runtime log file (`hexrs.log`) in Notepad.
    pub fn open_log_file(path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if !path.exists() {
            let _ = std::fs::File::create(path);
        }
        Command::new("notepad.exe")
            .arg(path)
            .spawn()
            .with_context(|| format!("failed to open log file {}", path.display()))?;
        Ok(())
    }

    /// Relaunches the current `Hexrs.exe` elevated via Windows UAC (`runas`).
    pub fn relaunch_as_administrator() -> Result<()> {
        let exe = std::env::current_exe().context("failed to resolve current Hexrs.exe path")?;
        let exe_w: Vec<u16> = exe
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        // SAFETY: `exe_w` is null-terminated UTF-16 and lives across the synchronous `ShellExecuteW` call.
        let res = unsafe {
            ShellExecuteW(
                None,
                w!("runas"),
                PCWSTR::from_raw(exe_w.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };

        if (res.0 as isize) <= 32 {
            anyhow::bail!(
                "UAC 提权启动失败或被用户取消 (ShellExecuteW code={})",
                res.0 as isize
            );
        }
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use std::path::Path;

    use anyhow::Result;
    use gpui::Window;

    use super::{Win32WindowCommand, WindowRectInfo};

    /// Non-Windows stub for `window_hwnd_raw`.
    pub fn window_hwnd_raw(_window: &Window) -> Result<isize> {
        Ok(0)
    }

    /// Non-Windows stub for `window_scale_for_hwnd`.
    pub fn window_scale_for_hwnd(_hwnd_raw: isize, fallback: f32) -> f32 {
        fallback
    }

    /// Non-Windows stub for `apply_window_command`.
    pub fn apply_window_command(_cmd: Win32WindowCommand) -> Result<()> {
        Ok(())
    }

    /// Non-Windows stub for `default_preview_lobby_rect`.
    pub fn default_preview_lobby_rect() -> WindowRectInfo {
        WindowRectInfo {
            left: 100,
            top: 100,
            right: 1060,
            bottom: 820,
            dpi: 96,
        }
    }

    /// Non-Windows stub for `default_preview_game_rect`.
    pub fn default_preview_game_rect() -> WindowRectInfo {
        WindowRectInfo {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
            dpi: 96,
        }
    }

    /// Non-Windows stub for `apply_overlay_styles`.
    #[allow(dead_code)]
    pub fn apply_overlay_styles(_window: &Window) -> Result<()> {
        Ok(())
    }

    /// Non-Windows stub for `find_window_by_pid`.
    #[allow(dead_code)]
    pub fn find_window_by_pid(_pid: u32) -> Option<WindowRectInfo> {
        None
    }

    /// Non-Windows stub for `find_league_lobby_window`.
    pub fn find_league_lobby_window(_pid: Option<u32>) -> Option<WindowRectInfo> {
        None
    }

    /// Non-Windows stub for `find_league_game_window`.
    pub fn find_league_game_window(_pid: Option<u32>) -> Option<WindowRectInfo> {
        None
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

    /// Non-Windows stub for `open_log_file`.
    pub fn open_log_file(path: &Path) -> Result<()> {
        tracing::info!(path = %path.display(), "open_log_file requested on non-Windows");
        Ok(())
    }

    /// Non-Windows stub for `relaunch_as_administrator`.
    pub fn relaunch_as_administrator() -> Result<()> {
        tracing::warn!("relaunch_as_administrator requested on non-Windows");
        anyhow::bail!("仅支持 Windows UAC 提权")
    }
}

pub use imp::{
    apply_window_command, default_preview_game_rect, default_preview_lobby_rect,
    find_league_game_window, find_league_lobby_window, open_data_directory, open_log_file,
    relaunch_as_administrator, window_hwnd_raw, window_scale_for_hwnd,
};
