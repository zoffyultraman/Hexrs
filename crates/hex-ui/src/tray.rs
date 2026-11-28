//! System tray icon and non-blocking menu action controller (`tray-icon 0.21`).

use anyhow::{Context, Result};
use hex_core::AppMode;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::win32::OverlayCorner;

/// User action triggered from the system tray context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ToggleOverlay,
    ToggleOverlayCorner,
    ToggleDiagnostics,
    CheckDataUpdate,
    ToggleTelemetry,
    OpenDataDir,
    OpenLogFile,
    RelaunchAsAdmin,
    Quit,
}

/// Owns the Windows/macOS system tray icon and menu items on the main UI thread.
pub struct TrayController {
    tray_icon: TrayIcon,
    status_item: MenuItem,
    toggle_overlay_item: CheckMenuItem,
    toggle_corner_item: MenuItem,
    toggle_diagnostics_item: CheckMenuItem,
    toggle_telemetry_item: CheckMenuItem,
    toggle_overlay_id: MenuId,
    toggle_corner_id: MenuId,
    toggle_diagnostics_id: MenuId,
    toggle_telemetry_id: MenuId,
    check_update_id: MenuId,
    open_dir_id: MenuId,
    open_log_id: MenuId,
    relaunch_admin_id: MenuId,
    quit_id: MenuId,
}

impl TrayController {
    /// Creates the system tray icon and context menu; must be called on the main UI thread.
    pub fn new() -> Result<Self> {
        let menu = Menu::new();

        let status_item = MenuItem::new("状态: 休眠中", false, None);
        let toggle_overlay_item = CheckMenuItem::new("显示悬浮窗", true, true, None);
        let toggle_corner_item = MenuItem::new("局内位置: 右上角 (点击切左上角)", true, None);
        let toggle_diagnostics_item = CheckMenuItem::new("显示诊断面板", true, false, None);
        let toggle_telemetry_item = CheckMenuItem::new("匿名众包战绩上报", true, true, None);
        let check_update_item = MenuItem::new("检查数据热更新", true, None);
        let open_dir_item = MenuItem::new("打开数据目录", true, None);
        let open_log_item = MenuItem::new("打开运行日志", true, None);
        let relaunch_admin_item = MenuItem::new(
            "以管理员身份重启 (UAC)",
            !hex_core::is_current_process_elevated(),
            None,
        );
        let quit_item = MenuItem::new("退出 Hexrs", true, None);

        let sep1 = PredefinedMenuItem::separator();
        let sep2 = PredefinedMenuItem::separator();

        menu.append(&status_item)
            .context("failed to append status tray item")?;
        menu.append(&sep1)
            .context("failed to append separator")?;
        menu.append(&toggle_overlay_item)
            .context("failed to append overlay toggle item")?;
        menu.append(&toggle_corner_item)
            .context("failed to append corner toggle item")?;
        menu.append(&toggle_diagnostics_item)
            .context("failed to append diagnostics toggle item")?;
        menu.append(&toggle_telemetry_item)
            .context("failed to append telemetry toggle item")?;
        menu.append(&check_update_item)
            .context("failed to append check-update item")?;
        menu.append(&open_dir_item)
            .context("failed to append open-dir item")?;
        menu.append(&open_log_item)
            .context("failed to append open-log item")?;
        menu.append(&relaunch_admin_item)
            .context("failed to append relaunch-admin item")?;
        menu.append(&sep2)
            .context("failed to append separator")?;
        menu.append(&quit_item)
            .context("failed to append quit item")?;

        let toggle_overlay_id = toggle_overlay_item.id().clone();
        let toggle_corner_id = toggle_corner_item.id().clone();
        let toggle_diagnostics_id = toggle_diagnostics_item.id().clone();
        let toggle_telemetry_id = toggle_telemetry_item.id().clone();
        let check_update_id = check_update_item.id().clone();
        let open_dir_id = open_dir_item.id().clone();
        let open_log_id = open_log_item.id().clone();
        let relaunch_admin_id = relaunch_admin_item.id().clone();
        let quit_id = quit_item.id().clone();

        let icon = build_hextech_icon()?;
        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Hexrs · 休眠中")
            .with_icon(icon)
            .build()
            .context("failed to build TrayIcon")?;

        Ok(Self {
            tray_icon,
            status_item,
            toggle_overlay_item,
            toggle_corner_item,
            toggle_diagnostics_item,
            toggle_telemetry_item,
            toggle_overlay_id,
            toggle_corner_id,
            toggle_diagnostics_id,
            toggle_telemetry_id,
            check_update_id,
            open_dir_id,
            open_log_id,
            relaunch_admin_id,
            quit_id,
        })
    }

    /// Drains any pending tray menu clicks without blocking.
    pub fn poll_actions(&self) -> Vec<TrayAction> {
        let mut actions = Vec::new();
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = event.id;
            if id == self.toggle_overlay_id {
                actions.push(TrayAction::ToggleOverlay);
            } else if id == self.toggle_corner_id {
                actions.push(TrayAction::ToggleOverlayCorner);
            } else if id == self.toggle_diagnostics_id {
                actions.push(TrayAction::ToggleDiagnostics);
            } else if id == self.toggle_telemetry_id {
                actions.push(TrayAction::ToggleTelemetry);
            } else if id == self.check_update_id {
                actions.push(TrayAction::CheckDataUpdate);
            } else if id == self.open_dir_id {
                actions.push(TrayAction::OpenDataDir);
            } else if id == self.open_log_id {
                actions.push(TrayAction::OpenLogFile);
            } else if id == self.relaunch_admin_id {
                actions.push(TrayAction::RelaunchAsAdmin);
            } else if id == self.quit_id {
                actions.push(TrayAction::Quit);
            }
        }
        actions
    }

    /// Updates the disabled status line and tooltip to reflect the active mode.
    pub fn update_status(&self, mode: AppMode) {
        let (status_label, tooltip) = match mode {
            AppMode::Dormant => ("状态: 休眠中", "Hexrs · 休眠中"),
            AppMode::LobbyDocked => ("状态: 选人侧边栏", "Hexrs · 选人侧边栏"),
            AppMode::InGameOverlay => ("状态: 局内悬浮窗", "Hexrs · 局内悬浮窗"),
        };
        self.status_item.set_text(status_label);
        let _ = self.tray_icon.set_tooltip(Some(tooltip));
    }

    /// Updates the in-game corner menu label cleanly when flipped between TopLeft and TopRight.
    pub fn update_corner_label(&self, corner: OverlayCorner) {
        let label = match corner {
            OverlayCorner::TopLeft => "局内位置: 左上角 (点击切右上角)",
            OverlayCorner::TopRight => "局内位置: 右上角 (点击切左上角)",
        };
        self.toggle_corner_item.set_text(label);
    }

    /// Returns whether the overlay visibility check item is currently checked.
    pub fn is_overlay_checked(&self) -> bool {
        self.toggle_overlay_item.is_checked()
    }

    /// Returns whether the diagnostics panel check item is currently checked.
    pub fn is_diagnostics_checked(&self) -> bool {
        self.toggle_diagnostics_item.is_checked()
    }

    /// Programmatically syncs the diagnostics panel check item state.
    pub fn set_diagnostics_checked(&self, checked: bool) {
        self.toggle_diagnostics_item.set_checked(checked);
    }

    /// Returns whether anonymized EOG telemetry is currently checked.
    pub fn is_telemetry_checked(&self) -> bool {
        self.toggle_telemetry_item.is_checked()
    }
}

/// Generates a 32x32 RGBA hextech diamond emblem in memory (zero external `.ico` asset required).
fn build_hextech_icon() -> Result<Icon> {
    const SIZE: usize = 32;
    let mut rgba = vec![0_u8; SIZE * SIZE * 4];
    let center = (SIZE as f32 - 1.0) * 0.5;

    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = (x as f32 - center).abs();
            let dy = (y as f32 - center).abs();
            let manhattan = dx + dy;
            let idx = (y * SIZE + x) * 4;

            if manhattan <= 5.5 {
                // Hextech cyan crystal core (#0AC8B9)
                rgba[idx] = 0x0A;
                rgba[idx + 1] = 0xC8;
                rgba[idx + 2] = 0xB9;
                rgba[idx + 3] = 0xFF;
            } else if manhattan <= 8.5 {
                // Obsidian inner ring (#0B101B)
                rgba[idx] = 0x0B;
                rgba[idx + 1] = 0x10;
                rgba[idx + 2] = 0x1B;
                rgba[idx + 3] = 0xFF;
            } else if manhattan <= 12.0 {
                // Hextech gold diamond frame (#C8AA6E)
                rgba[idx] = 0xC8;
                rgba[idx + 1] = 0xAA;
                rgba[idx + 2] = 0x6E;
                rgba[idx + 3] = 0xFF;
            } else if manhattan <= 14.5 {
                // Dark border backing (#0B101B)
                rgba[idx] = 0x0B;
                rgba[idx + 1] = 0x10;
                rgba[idx + 2] = 0x1B;
                rgba[idx + 3] = 0xE6;
            }
        }
    }

    Icon::from_rgba(rgba, SIZE as u32, SIZE as u32).context("failed to create RGBA tray icon")
}
