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
pub const IN_GAME_HUD_COLLAPSED_HEIGHT_DIP: f32 = 46.0;

const CARD_WIDTH_RATIO: f32 = 0.78;
const CARD_GAP_RATIO: f32 = 0.22;
pub const AUGMENT_BAR_TOP_PAD_DIP: f32 = 12.0;
pub const AUGMENT_BAR_CARD_HEIGHT_DIP: f32 = 46.0;
const AUGMENT_BAR_BOTTOM_PAD_DIP: f32 = 2.0;
const AUGMENT_BAR_HEIGHT_DIP: f32 =
    AUGMENT_BAR_TOP_PAD_DIP + AUGMENT_BAR_CARD_HEIGHT_DIP + AUGMENT_BAR_BOTTOM_PAD_DIP;

/// Pixel-calibrated Champ Select bench strip geometry relative to `LeagueClientUx.exe`.
pub const BENCH_SLOT0_CX: f32 = 0.2906;
pub const BENCH_SLOT_PITCH_FRAC: f32 = 0.04594;
pub const BENCH_SLOT_WIDTH_FRAC: f32 = 0.0384;
pub const BENCH_SLOT_HEIGHT_FRAC: f32 = 0.0195;
pub const BENCH_STRIP_Y_FRAC: f32 = 0.0824;
pub const BENCH_STRIP_LEFT_FRAC: f32 = BENCH_SLOT0_CX - BENCH_SLOT_WIDTH_FRAC * 0.5;

/// Pixel-calibrated Champ Select center champion pick card strip geometry relative to `LeagueClientUx.exe`.
pub const PICK_CARD_WIDTH_FRAC: f32 = 0.1401;
pub const PICK_CARD_GAP_FRAC: f32 = 0.0283;
pub const PICK_CARD_PITCH_FRAC: f32 = PICK_CARD_WIDTH_FRAC + PICK_CARD_GAP_FRAC;
pub const PICK_CARD_STRIP_Y_FRAC: f32 = 0.6740;
pub const PICK_CARD_BADGE_HEIGHT_FRAC: f32 = 0.0340;

/// Pixel-calibrated Champ Select allied team badge strip right-edge anchor relative to `LeagueClientUx.exe`.
pub const TEAM_STRIP_RIGHT_FRAC: f32 = 0.2580;

/// Computes the logical DIP width of `TeamBadgeStrip` from the client width in DIPs.
pub fn compute_team_strip_width_dip(target_width_dip: f32) -> f32 {
    (target_width_dip * 0.076).clamp(110.0, 125.0)
}

/// Corner anchor for the compact in-game HUD card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayCorner {
    TopLeft,
    #[default]
    TopRight,
}

/// Side wing of the hold-Tab in-game scoreboard overlay (`Left` = team_one, `Right` = team_two).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabScoreboardSide {
    Left,
    Right,
}

/// Deferred Win32 window repositioning or visibility command executed after GPUI releases `Window`.
#[derive(Debug, Clone, PartialEq)]
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
    CardBadgeStrip {
        target_rect: Option<WindowRectInfo>,
        card_count: usize,
    },
    InGameOverlay {
        target_rect: Option<WindowRectInfo>,
        corner: OverlayCorner,
        visible: bool,
        collapsed: bool,
    },
    InGameAugmentBar {
        target_rect: Option<WindowRectInfo>,
        visible: bool,
        slot_count: usize,
        layout: Option<AugmentOfferLayout>,
    },
    LoadingScreenOverlay {
        target_rect: Option<WindowRectInfo>,
        visible: bool,
    },
    TabScoreboardSideOverlay {
        target_rect: Option<WindowRectInfo>,
        side: TabScoreboardSide,
        has_augment_wings: bool,
        visible: bool,
    },
}

/// Deferred Win32 command targeting a specific `HWND` outside GPUI's re-entrant `Window` lock.
#[derive(Debug, Clone, PartialEq)]
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
    pub hwnd_raw: isize,
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

fn fallback_augment_bar_bounds(
    target: WindowRectInfo,
    scale: f32,
    slot_count: usize,
    target_w: i32,
    target_h: i32,
) -> (i32, i32, i32, f32) {
    let n = slot_count.clamp(3, 4);
    let strip_total_ratio = (n - 1) as f32 + CARD_WIDTH_RATIO;
    let spacing_norm = if target_w as f32 / target_h.max(1) as f32 >= 2.05 {
        0.165
    } else {
        0.225
    };
    let min_w = (480.0 * scale).round() as i32;
    let width_px = ((target_w as f32 * spacing_norm * strip_total_ratio).round() as i32)
        .clamp(min_w.min(target_w), target_w);
    let x = target.left + (target_w - width_px) / 2;
    let y = target.top + (target_h as f32 * 0.08).round() as i32;
    (x, y, width_px, strip_total_ratio)
}

fn compute_augment_bar_geometry(
    target: WindowRectInfo,
    scale: f32,
    slot_count: usize,
    layout: Option<&AugmentOfferLayout>,
) -> AugmentBarGeometry {
    let scale = if scale <= 0.0 { 1.0 } else { scale };
    let target_w = target.width().max(320);
    let target_h = target.height().max(90);
    let height_px = ((AUGMENT_BAR_HEIGHT_DIP * scale).round() as i32).min(target_h);

    let (raw_x, raw_y, width_px, strip_total_ratio) = match layout {
        Some(l) if l.slot_centers_x.len() >= 2 => {
            let n = l.slot_centers_x.len().clamp(2, 4);
            let cx_first = l.slot_centers_x[0];
            let cx_last = l.slot_centers_x[n - 1];
            if cx_last > cx_first + 0.08 * (n - 1) as f32 {
                let spacing_norm = ((cx_last - cx_first) / (n - 1) as f32).clamp(0.14, 0.36);
                let strip_total_ratio = (n - 1) as f32 + CARD_WIDTH_RATIO;
                let spacing_px = spacing_norm * target_w as f32;
                let strip_left_px = target.left
                    + ((cx_first - spacing_norm * (CARD_WIDTH_RATIO * 0.5)) * target_w as f32)
                        .round() as i32;
                let min_w = (360.0 * scale).round() as i32;
                let width_px = (spacing_px * strip_total_ratio)
                    .round()
                    .clamp(min_w.min(target_w) as f32, target_w as f32)
                    as i32;
                let card_top_px =
                    target.top + (l.title_y * target_h as f32 - spacing_px * 0.66).round() as i32;
                let v_gap_px = (4.0 * scale).round() as i32;
                let y = (card_top_px - height_px - v_gap_px).clamp(
                    target.top + (4.0 * scale).round() as i32,
                    (target.bottom - height_px).max(target.top),
                );
                (strip_left_px, y, width_px, strip_total_ratio)
            } else {
                fallback_augment_bar_bounds(target, scale, slot_count, target_w, target_h)
            }
        }
        _ => fallback_augment_bar_bounds(target, scale, slot_count, target_w, target_h),
    };

    let x = raw_x.clamp(target.left, (target.right - width_px).max(target.left));
    let y = raw_y.clamp(target.top, (target.bottom - height_px).max(target.top));
    let gap_dip =
        ((width_px as f32 / scale) * (CARD_GAP_RATIO / strip_total_ratio)).clamp(8.0, 160.0);

    AugmentBarGeometry {
        x,
        y,
        width_px,
        height_px,
        gap_dip,
    }
}

/// Computes the exact DIP flex gap matching the aligned 3-card or 4-card augment strip geometry.
pub fn compute_augment_bar_gap_dip(
    target: Option<WindowRectInfo>,
    slot_count: usize,
    layout: Option<&AugmentOfferLayout>,
) -> f32 {
    let Some(rect) = target else {
        return 28.0;
    };
    compute_augment_bar_geometry(rect, rect.scale(), slot_count, layout).gap_dip
}

/// Logical DIP coordinates for the 10-player loading screen card strips and center VS summary bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoadingLayoutGeometry {
    pub row_left_dip: f32,
    pub card_w_dip: f32,
    pub card_gap_dip: f32,
    pub card_strip_h_dip: f32,
    pub top_row_y_dip: f32,
    pub bottom_row_y_dip: f32,
    pub center_bar_left_dip: f32,
    pub center_bar_y_dip: f32,
    pub center_bar_w_dip: f32,
    pub center_bar_h_dip: f32,
}

/// Computes the 10-card top/bottom strip and top VS summary bar layout for a loading screen window.
pub fn compute_loading_layout_dip(window_w_dip: f32, window_h_dip: f32) -> LoadingLayoutGeometry {
    let w = window_w_dip.max(320.0);
    let h = window_h_dip.max(200.0);
    let ui_scale = (h / 1080.0).clamp(0.85, 1.35);

    let raw_card_w = h * 0.2328;
    let raw_gap = h * 0.0411;
    let raw_total = 5.0 * raw_card_w + 4.0 * raw_gap;
    let max_row_w = w * 0.90;
    let (card_w_dip, card_gap_dip) = if raw_total > max_row_w && raw_total > 0.0 {
        let scale = max_row_w / raw_total;
        (raw_card_w * scale, raw_gap * scale)
    } else {
        (raw_card_w, raw_gap)
    };

    let row_w = 5.0 * card_w_dip + 4.0 * card_gap_dip;
    let row_left_dip = ((w - row_w) * 0.5 - w * 0.0033).max(0.0);
    let card_strip_h_dip = (48.0 * ui_scale).clamp(44.0, 62.0);
    let top_row_y_dip = h * 0.067;
    let bottom_row_y_dip = h * 0.553;

    let center_bar_h_dip = (29.0 * ui_scale).clamp(24.0, 34.0);
    let center_bar_y_dip = (h * 0.028).min((top_row_y_dip - center_bar_h_dip - 4.0).max(2.0));
    let center_bar_w_dip = row_w
        .min(660.0 * ui_scale)
        .max(380.0)
        .min((w - 24.0).max(200.0));
    let center_bar_left_dip = ((w - center_bar_w_dip) * 0.5).max(0.0);

    LoadingLayoutGeometry {
        row_left_dip,
        card_w_dip,
        card_gap_dip,
        card_strip_h_dip,
        top_row_y_dip,
        bottom_row_y_dip,
        center_bar_left_dip,
        center_bar_y_dip,
        center_bar_w_dip,
        center_bar_h_dip,
    }
}

/// Logical DIP geometry for the left and right hold-Tab in-game scoreboard side panels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabScoreboardLayoutGeometry {
    pub left_x_dip: f32,
    pub right_x_dip: f32,
    pub window_top_dip: f32,
    pub panel_w_dip: f32,
    pub window_h_dip: f32,
    pub header_h_dip: f32,
    pub header_gap_dip: f32,
    pub row_h_dip: f32,
    pub row_pitch_dip: f32,
    pub ui_scale: f32,
}

/// Computes left/right side-panel DIP coordinates aligned to the native League Tab scoreboard rows.
pub fn compute_tab_scoreboard_layout_dip(
    window_w_dip: f32,
    window_h_dip: f32,
    has_augment_wings: bool,
) -> TabScoreboardLayoutGeometry {
    let w = window_w_dip.max(320.0);
    let h = window_h_dip.max(200.0);
    let ui_scale = (h / 1080.0).clamp(0.85, 1.35);

    let half_board_w = h * if has_augment_wings { 0.632 } else { 0.535 };
    let board_left = (w * 0.5 - half_board_w).max(0.0);
    let board_right = (w * 0.5 + half_board_w).min(w);

    let gap = (6.0 * ui_scale).clamp(4.0, 10.0);
    let outer_margin = (6.0 * ui_scale).clamp(4.0, 10.0);
    let avail_side = (board_left - gap - outer_margin)
        .min(w - board_right - gap - outer_margin)
        .max(0.0);
    let desired_w = (232.0 * ui_scale).clamp(176.0, 310.0);
    let panel_w_dip = if avail_side >= 132.0 {
        desired_w.min(avail_side)
    } else {
        desired_w
            .min(avail_side.max(120.0))
            .min((w * 0.25).max(120.0))
    };

    let left_x_dip = (board_left - gap - panel_w_dip).max(outer_margin);
    let right_x_dip = (board_right + gap).min((w - outer_margin - panel_w_dip).max(0.0));

    let row0_top = h * 0.2871;
    let row_h_dip = h * 0.0645;
    let row_pitch_dip = h * 0.0710;
    let header_h_dip = (26.0 * ui_scale).clamp(22.0, 32.0);
    let header_gap_dip = (5.0 * ui_scale).clamp(4.0, 8.0);
    let window_top_dip = (row0_top - header_h_dip - header_gap_dip).max(4.0);
    let rows_total_h = 4.0 * row_pitch_dip + row_h_dip;
    let window_h_dip = header_h_dip + header_gap_dip + rows_total_h + 2.0;

    TabScoreboardLayoutGeometry {
        left_x_dip,
        right_x_dip,
        window_top_dip,
        panel_w_dip,
        window_h_dip,
        header_h_dip,
        header_gap_dip,
        row_h_dip,
        row_pitch_dip,
        ui_scale,
    }
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
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWM_WINDOW_CORNER_PREFERENCE,
    };
    use windows::Win32::Graphics::Gdi::InvalidateRect;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_TAB,
    };
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowW, GetClassNameW, GetForegroundWindow, GetSystemMetrics,
        GetWindowLongPtrW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
        IsWindowVisible, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow,
        SystemParametersInfoW, GWL_EXSTYLE, HWND_TOPMOST, LWA_ALPHA, SM_CXVIRTUALSCREEN,
        SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SPI_GETWORKAREA, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE, SW_SHOWNORMAL,
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE, WS_EX_APPWINDOW, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_EX_TRANSPARENT,
    };

    use super::{
        compute_augment_bar_geometry, compute_tab_scoreboard_layout_dip, AugmentOfferLayout,
        OverlayCorner, TabScoreboardSide, Win32WindowAction, Win32WindowCommand, WindowRectInfo,
        BENCH_SLOT_HEIGHT_FRAC, BENCH_SLOT_PITCH_FRAC, BENCH_SLOT_WIDTH_FRAC,
        BENCH_STRIP_LEFT_FRAC, BENCH_STRIP_Y_FRAC, DIAGNOSTICS_HEIGHT_DIP, DIAGNOSTICS_WIDTH_DIP,
        IN_GAME_HUD_COLLAPSED_HEIGHT_DIP, IN_GAME_HUD_HEIGHT_DIP, IN_GAME_HUD_WIDTH_DIP,
        PICK_CARD_BADGE_HEIGHT_FRAC, PICK_CARD_GAP_FRAC, PICK_CARD_STRIP_Y_FRAC,
        PICK_CARD_WIDTH_FRAC, SIDEBAR_WIDTH_DIP, TEAM_STRIP_RIGHT_FRAC,
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
            hwnd_raw: hwnd.0 as isize,
        })
    }

    unsafe fn read_window_class(hwnd: HWND) -> String {
        let mut buf = [0_u16; 128];
        let len = GetClassNameW(hwnd, &mut buf);
        if len <= 0 {
            String::new()
        } else {
            String::from_utf16_lossy(&buf[..len as usize])
        }
    }

    /// Returns `true` when the League lobby client (`RCLIENT`) or Hexrs itself owns foreground focus.
    pub fn is_lobby_window_foreground(
        lcu_pid: Option<u32>,
        lobby_hwnd_raw: isize,
        own_hwnd_raw: isize,
    ) -> bool {
        // SAFETY: Foreground window, PID, and class queries are read-only Win32 calls.
        unsafe {
            let fg = GetForegroundWindow();
            if fg.0.is_null() {
                return false;
            }
            let fg_raw = fg.0 as isize;
            if (lobby_hwnd_raw != 0 && fg_raw == lobby_hwnd_raw)
                || (own_hwnd_raw != 0 && fg_raw == own_hwnd_raw)
            {
                return true;
            }
            let mut fg_pid: u32 = 0;
            let _ = GetWindowThreadProcessId(fg, Some(&mut fg_pid));
            if fg_pid != 0 && (Some(fg_pid) == lcu_pid || fg_pid == std::process::id()) {
                return true;
            }
            read_window_class(fg) == "RCLIENT"
        }
    }

    /// Returns `true` when the active League in-game window (`RiotWindowClass`) or Hexrs HUD owns foreground focus.
    pub fn is_game_window_foreground(
        game_pid: Option<u32>,
        game_hwnd_raw: isize,
        own_hwnd_raw: isize,
    ) -> bool {
        // SAFETY: Foreground window, PID, and class queries are read-only Win32 calls.
        unsafe {
            let fg = GetForegroundWindow();
            if fg.0.is_null() {
                return false;
            }
            let fg_raw = fg.0 as isize;
            if (game_hwnd_raw != 0 && fg_raw == game_hwnd_raw)
                || (own_hwnd_raw != 0 && fg_raw == own_hwnd_raw)
            {
                return true;
            }
            let mut fg_pid: u32 = 0;
            let _ = GetWindowThreadProcessId(fg, Some(&mut fg_pid));
            if fg_pid != 0 && Some(fg_pid) == game_pid {
                return true;
            }
            read_window_class(fg) == "RiotWindowClass"
        }
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
            hwnd_raw: 0,
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
            hwnd_raw: 0,
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

        // Disable Windows 11 automatic rounded corners and 1px accent/border frame on overlays.
        let corner_pref: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const corner_pref).cast(),
            core::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
        let border_color: u32 = DWMWA_COLOR_NONE;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            (&raw const border_color).cast(),
            core::mem::size_of::<u32>() as u32,
        );
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
    pub fn apply_window_command(cmd: &Win32WindowCommand) -> Result<()> {
        let hwnd = HWND(cmd.hwnd_raw as *mut core::ffi::c_void);
        match &cmd.action {
            Win32WindowAction::Hide => hide_window_hwnd(hwnd),
            Win32WindowAction::Diagnostics => set_diagnostics_window_mode_hwnd(hwnd),
            Win32WindowAction::LobbyDock(target_rect) => {
                set_lobby_dock_mode_hwnd(hwnd, *target_rect)
            }
            Win32WindowAction::BenchBadgeStrip {
                target_rect,
                slot_count,
            } => set_bench_badge_strip_hwnd(hwnd, *target_rect, *slot_count),
            Win32WindowAction::TeamBadgeStrip { target_rect } => {
                set_team_badge_strip_hwnd(hwnd, *target_rect)
            }
            Win32WindowAction::CardBadgeStrip {
                target_rect,
                card_count,
            } => set_card_badge_strip_hwnd(hwnd, *target_rect, *card_count),
            Win32WindowAction::InGameOverlay {
                target_rect,
                corner,
                visible,
                collapsed,
            } => set_in_game_overlay_mode_hwnd(hwnd, *target_rect, *corner, *visible, *collapsed),
            Win32WindowAction::InGameAugmentBar {
                target_rect,
                visible,
                slot_count,
                layout,
            } => set_in_game_augment_bar_mode_hwnd(
                hwnd,
                *target_rect,
                *visible,
                *slot_count,
                layout.as_ref(),
            ),
            Win32WindowAction::LoadingScreenOverlay {
                target_rect,
                visible,
            } => set_loading_screen_overlay_hwnd(hwnd, *target_rect, *visible),
            Win32WindowAction::TabScoreboardSideOverlay {
                target_rect,
                side,
                has_augment_wings,
                visible,
            } => set_tab_scoreboard_side_overlay_hwnd(
                hwnd,
                *target_rect,
                *side,
                *has_augment_wings,
                *visible,
            ),
        }
    }

    /// Returns `true` when bare `Tab` is currently held down (without Alt/Ctrl/Win modifiers).
    pub fn is_tab_scoreboard_hotkey_pressed() -> bool {
        // SAFETY: `GetAsyncKeyState` is a stateless, hook-free read-only keyboard state query.
        unsafe {
            GetAsyncKeyState(VK_TAB.0 as i32) < 0
                && GetAsyncKeyState(VK_MENU.0 as i32) >= 0
                && GetAsyncKeyState(VK_CONTROL.0 as i32) >= 0
                && GetAsyncKeyState(VK_LWIN.0 as i32) >= 0
                && GetAsyncKeyState(VK_RWIN.0 as i32) >= 0
        }
    }

    /// Anchors one click-through hold-Tab scoreboard side panel adjacent to the native Tab board.
    fn set_tab_scoreboard_side_overlay_hwnd(
        hwnd: HWND,
        target_rect: Option<WindowRectInfo>,
        side: TabScoreboardSide,
        has_augment_wings: bool,
        visible: bool,
    ) -> Result<()> {
        // SAFETY: `hwnd` belongs to the active GPUI Tab scoreboard side window on the UI thread.
        unsafe {
            apply_transparent_ex_style(hwnd, false);

            let Some(target) = target_rect.filter(|_| visible) else {
                let _ = ShowWindow(hwnd, SW_HIDE);
                return Ok(());
            };

            let own_dpi = GetDpiForWindow(hwnd);
            let dpi = if own_dpi > 0 { own_dpi } else { target.dpi.max(96) };
            let scale = dpi as f32 / 96.0;

            let target_w = target.width().max(320);
            let target_h = target.height().max(200);
            let layout = compute_tab_scoreboard_layout_dip(
                target_w as f32 / scale,
                target_h as f32 / scale,
                has_augment_wings,
            );
            let x_dip = match side {
                TabScoreboardSide::Left => layout.left_x_dip,
                TabScoreboardSide::Right => layout.right_x_dip,
            };

            let width_px = ((layout.panel_w_dip * scale).round() as i32).clamp(1, target_w);
            let height_px = ((layout.window_h_dip * scale).round() as i32).clamp(1, target_h);
            let max_x = target.left + (target_w - width_px).max(0);
            let max_y = target.top + (target_h - height_px).max(0);
            let x = (target.left + (x_dip * scale).round() as i32).clamp(target.left, max_x);
            let y = (target.top + (layout.window_top_dip * scale).round() as i32)
                .clamp(target.top, max_y);

            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                width_px,
                height_px,
                SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
            .context("SetWindowPos(tab scoreboard side overlay) failed")?;

            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }

        Ok(())
    }

    /// Anchors the full-window click-through 10-player loading screen overlay over `League of Legends.exe`.
    fn set_loading_screen_overlay_hwnd(
        hwnd: HWND,
        target_rect: Option<WindowRectInfo>,
        visible: bool,
    ) -> Result<()> {
        // SAFETY: `hwnd` belongs to the active GPUI loading overlay window on the UI thread.
        unsafe {
            apply_transparent_ex_style(hwnd, false);

            let Some(target) = target_rect.filter(|_| visible) else {
                let _ = ShowWindow(hwnd, SW_HIDE);
                return Ok(());
            };

            let w = target.width().max(320);
            let h = target.height().max(200);

            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                target.left,
                target.top,
                w,
                h,
                SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
            .context("SetWindowPos(loading screen overlay) failed")?;

            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }

        Ok(())
    }

    /// Anchors the compact collapsible corner HUD onto `League of Legends.exe`.
    fn set_in_game_overlay_mode_hwnd(
        hwnd: HWND,
        target_rect: Option<WindowRectInfo>,
        corner: OverlayCorner,
        visible: bool,
        collapsed: bool,
    ) -> Result<()> {

        // SAFETY: `hwnd` belongs to the active GPUI window on the UI thread.
        unsafe {
            apply_transparent_ex_style(hwnd, visible);

            let Some(target) = target_rect.filter(|_| visible) else {
                let _ = ShowWindow(hwnd, SW_HIDE);
                return Ok(());
            };

            let own_dpi = GetDpiForWindow(hwnd);
            let dpi = if own_dpi > 0 { own_dpi } else { target.dpi.max(96) };
            let scale = dpi as f32 / 96.0;

            let target_w = target.width().max(160);
            let target_h = target.height().max(80);
            let hud_h_dip = if collapsed {
                IN_GAME_HUD_COLLAPSED_HEIGHT_DIP
            } else {
                IN_GAME_HUD_HEIGHT_DIP
            };
            let width_px = ((IN_GAME_HUD_WIDTH_DIP * scale).round() as i32).min(target_w);
            let height_px = ((hud_h_dip * scale).round() as i32).min(target_h);
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

    /// Anchors a permanently click-through horizontal strip strictly ABOVE the augment cards.
    fn set_in_game_augment_bar_mode_hwnd(
        hwnd: HWND,
        target_rect: Option<WindowRectInfo>,
        visible: bool,
        slot_count: usize,
        layout: Option<&AugmentOfferLayout>,
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
            let geom =
                compute_augment_bar_geometry(target, dpi as f32 / 96.0, slot_count, layout);

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

            let y = (target.top + (BENCH_STRIP_Y_FRAC * target_h as f32).round() as i32)
                .clamp(target.top, target.bottom.saturating_sub(1));
            let max_h = (target.bottom - y).max(1);
            let min_h = (16.0 * scale).round() as i32;
            let cap_h = (18.0 * scale).round() as i32;
            let raw_h = ((BENCH_SLOT_HEIGHT_FRAC * target_h as f32).round() as i32)
                .clamp(min_h, cap_h)
                + (6.0 * scale).round() as i32;
            let h = raw_h.min(max_h);

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

            let mut w = ((target_w as f32 * 0.076).round() as i32)
                .clamp((110.0 * scale).round() as i32, (125.0 * scale).round() as i32);
            let right_px = target.left + (TEAM_STRIP_RIGHT_FRAC * target_w as f32).round() as i32;
            let min_left_px = target.left + (0.1650 * target_w as f32).round() as i32;
            let x = (right_px - w)
                .max(min_left_px)
                .clamp(target.left, target.right.saturating_sub(1));
            w = w.min((target.right - x).max(1));

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

    /// Anchors the narrow horizontal pick-card badge strip strictly below the 2 or 3 center cards.
    fn set_card_badge_strip_hwnd(
        hwnd: HWND,
        target_rect: Option<WindowRectInfo>,
        card_count: usize,
    ) -> Result<()> {
        // SAFETY: `hwnd` belongs to the active GPUI window on the UI thread.
        unsafe {
            apply_transparent_ex_style(hwnd, false);

            let Some(target) = target_rect.filter(|_| (2..=3).contains(&card_count)) else {
                let _ = ShowWindow(hwnd, SW_HIDE);
                return Ok(());
            };

            let own_dpi = GetDpiForWindow(hwnd);
            let dpi = if own_dpi > 0 { own_dpi } else { target.dpi.max(96) };
            let scale = dpi as f32 / 96.0;

            let target_w = target.width().max(320);
            let target_h = target.height().max(200);
            let count = card_count.clamp(2, 3);

            let strip_w_frac =
                count as f32 * PICK_CARD_WIDTH_FRAC + (count - 1) as f32 * PICK_CARD_GAP_FRAC;
            let strip_left_frac = (0.4965 - strip_w_frac * 0.5).max(0.0);

            let x = (target.left + (strip_left_frac * target_w as f32).round() as i32)
                .clamp(target.left, target.right.saturating_sub(1));
            let max_w = (target.right - x).max(1);
            let w = ((strip_w_frac * target_w as f32).round() as i32 + (4.0 * scale).round() as i32)
                .max((120.0 * scale).round() as i32)
                .min(max_w);

            let y = (target.top + (PICK_CARD_STRIP_Y_FRAC * target_h as f32).round() as i32)
                .clamp(target.top, target.bottom.saturating_sub(1));
            let max_h = (target.bottom - y).max(1);
            let raw_h = ((PICK_CARD_BADGE_HEIGHT_FRAC * target_h as f32).round() as i32)
                .clamp((22.0 * scale).round() as i32, (38.0 * scale).round() as i32)
                + (12.0 * scale).round() as i32;
            let h = raw_h.min(max_h);

            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                w,
                h,
                SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
            .context("SetWindowPos(card badge strip) failed")?;

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

    /// Opens an `https://` URL in the user's default web browser via `ShellExecuteW`.
    pub fn open_url(url: &str) -> Result<()> {
        let trimmed = url.trim();
        if !trimmed.starts_with("https://") {
            anyhow::bail!("refusing to open non-HTTPS URL");
        }
        let url_w: Vec<u16> = trimmed.encode_utf16().chain(std::iter::once(0)).collect();

        // SAFETY: `url_w` is null-terminated UTF-16 and lives across the synchronous `ShellExecuteW` call.
        let res = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR::from_raw(url_w.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };

        if (res.0 as isize) <= 32 {
            anyhow::bail!(
                "failed to open URL in browser (ShellExecuteW code={})",
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
    pub fn apply_window_command(_cmd: &Win32WindowCommand) -> Result<()> {
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
            hwnd_raw: 0,
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
            hwnd_raw: 0,
        }
    }

    /// Non-Windows stub for `is_lobby_window_foreground`.
    pub fn is_lobby_window_foreground(
        _lcu_pid: Option<u32>,
        _lobby_hwnd_raw: isize,
        _own_hwnd_raw: isize,
    ) -> bool {
        false
    }

    /// Non-Windows stub for `is_game_window_foreground`.
    pub fn is_game_window_foreground(
        _game_pid: Option<u32>,
        _game_hwnd_raw: isize,
        _own_hwnd_raw: isize,
    ) -> bool {
        false
    }

    /// Non-Windows stub for `is_tab_scoreboard_hotkey_pressed`.
    pub fn is_tab_scoreboard_hotkey_pressed() -> bool {
        false
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

    /// Non-Windows stub for `open_url`.
    pub fn open_url(url: &str) -> Result<()> {
        let trimmed = url.trim();
        if !trimmed.starts_with("https://") {
            anyhow::bail!("refusing to open non-HTTPS URL");
        }
        tracing::info!(url = trimmed, "open_url requested on non-Windows");
        Ok(())
    }
}

pub use imp::{
    apply_window_command, default_preview_game_rect, default_preview_lobby_rect,
    find_league_game_window, find_league_lobby_window, is_game_window_foreground,
    is_lobby_window_foreground, is_tab_scoreboard_hotkey_pressed, open_data_directory,
    open_log_file, open_url, relaunch_as_administrator, window_hwnd_raw, window_scale_for_hwnd,
};

#[cfg(test)]
mod tests {
    use super::{
        compute_loading_layout_dip, compute_tab_scoreboard_layout_dip, open_url, OverlayCorner,
    };

    #[test]
    fn open_url_rejects_non_https_schemes() {
        assert!(open_url("http://example.com").is_err());
        assert!(open_url("javascript:alert(1)").is_err());
        assert!(open_url("").is_err());
    }

    #[test]
    fn overlay_corner_defaults_to_top_right() {
        assert_eq!(OverlayCorner::default(), OverlayCorner::TopRight);
    }

    #[test]
    fn loading_layout_fits_and_does_not_overlap_across_resolutions() {
        for (w, h) in [
            (1920.0_f32, 1080.0_f32),
            (2560.0_f32, 1440.0_f32),
            (2560.0_f32, 1080.0_f32),
            (1280.0_f32, 800.0_f32),
        ] {
            let g = compute_loading_layout_dip(w, h);
            assert!(
                (44.0..=62.0).contains(&g.card_strip_h_dip),
                "strip height {} out of 44..=62 at {w}x{h}",
                g.card_strip_h_dip
            );
            let row_w = 5.0 * g.card_w_dip + 4.0 * g.card_gap_dip;
            assert!(g.row_left_dip >= 0.0, "row_left < 0 at {w}x{h}");
            assert!(
                g.row_left_dip + row_w <= w + 0.1,
                "row overflows right edge at {w}x{h}"
            );
            assert!(
                g.center_bar_y_dip >= 0.0
                    && g.center_bar_y_dip + g.center_bar_h_dip <= g.top_row_y_dip,
                "top VS bar overlaps top card row at {w}x{h}: bar_end={} top_row={}",
                g.center_bar_y_dip + g.center_bar_h_dip,
                g.top_row_y_dip
            );
            assert!(
                g.bottom_row_y_dip > h * 0.54,
                "bottom row {} does not clear mid-screen lore tip (> {}) at {w}x{h}",
                g.bottom_row_y_dip,
                h * 0.54
            );
            assert!(
                g.top_row_y_dip + g.card_strip_h_dip < g.bottom_row_y_dip,
                "top row overlaps bottom row at {w}x{h}"
            );
            assert!(g.center_bar_left_dip >= 0.0, "center bar left < 0 at {w}x{h}");
            assert!(
                g.center_bar_left_dip + g.center_bar_w_dip <= w + 0.1,
                "center bar overflows right edge at {w}x{h}"
            );
        }
    }

    #[test]
    fn tab_scoreboard_layout_fits_and_avoids_board_hud_and_minimap() {
        for (w, h) in [
            (1920.0_f32, 1080.0_f32),
            (2560.0_f32, 1440.0_f32),
            (2560.0_f32, 1080.0_f32),
            (1920.0_f32, 1200.0_f32),
            (1280.0_f32, 720.0_f32),
        ] {
            for has_augment_wings in [true, false] {
                let g = compute_tab_scoreboard_layout_dip(w, h, has_augment_wings);
                let half_board_w = h * if has_augment_wings { 0.632 } else { 0.535 };
                let board_left = (w * 0.5 - half_board_w).max(0.0);
                let board_right = (w * 0.5 + half_board_w).min(w);

                assert!(g.left_x_dip >= 0.0, "left_x < 0 at {w}x{h} wings={has_augment_wings}");
                assert!(
                    g.left_x_dip + g.panel_w_dip <= board_left + 0.1,
                    "left panel overlaps native Tab board at {w}x{h} wings={has_augment_wings}"
                );
                assert!(
                    g.right_x_dip >= board_right - 0.1,
                    "right panel overlaps native Tab board at {w}x{h} wings={has_augment_wings}"
                );
                assert!(
                    g.right_x_dip + g.panel_w_dip <= w + 0.1,
                    "right panel overflows screen at {w}x{h} wings={has_augment_wings}"
                );

                let row0_top = g.window_top_dip + g.header_h_dip + g.header_gap_dip;
                let row4_bottom = row0_top + 4.0 * g.row_pitch_dip + g.row_h_dip;
                assert!(
                    row0_top >= h * 0.287 && row4_bottom <= h * 0.636,
                    "rows [{row0_top}, {row4_bottom}] out of [{}, {}] at {w}x{h}",
                    h * 0.287,
                    h * 0.636
                );

                if (w - 1920.0).abs() < 0.1 && (h - 1080.0).abs() < 0.1 {
                    assert!(
                        g.window_top_dip > 265.0,
                        "1080p top {} overlaps expanded corner HUD (> 265)",
                        g.window_top_dip
                    );
                    assert!(
                        g.window_top_dip + g.window_h_dip < 720.0,
                        "1080p bottom {} overlaps minimap (< 720)",
                        g.window_top_dip + g.window_h_dip
                    );
                }
            }
        }
    }
}
