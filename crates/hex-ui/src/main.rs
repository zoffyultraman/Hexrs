#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod lobby_badge_view;
mod lobby_view;
mod overlay_view;
mod theme;
mod tray;
mod win32;

use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use gpui::{
    prelude::*, px, size, App, Application, AsyncApp, Bounds, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions,
};
use tracing::{error, info, warn};
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::EnvFilter;

use crate::app::HexOverlayApp;
use crate::lobby_badge_view::{LobbyBadgeOverlay, LobbyBadgeState};
use crate::tray::TrayController;

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(60);

/// Returns `%LOCALAPPDATA%/Hexrs/hexoverlay.log` (or platform equivalent).
pub fn log_file_path() -> PathBuf {
    hex_data::data_dir().join("hexoverlay.log")
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let data_dir = hex_data::data_dir();
    let _ = fs::create_dir_all(&data_dir);
    let log_path = log_file_path();

    match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(file) => {
            let writer = std::io::stderr.and(Mutex::new(file));
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(writer)
                .try_init();
        }
        Err(_) => {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .try_init();
        }
    }
}

fn main() {
    init_tracing();

    let (event_rx, core_handle) = match hex_core::spawn_core_service() {
        Ok(pair) => pair,
        Err(err) => {
            error!(error = %err, "failed to start hex-core background service");
            return;
        }
    };

    info!(
        log_path = %log_file_path().display(),
        "HexOverlay starting GPUI event loop"
    );

    Application::new().run(move |cx: &mut App| {
        let tray = match TrayController::new() {
            Ok(controller) => Some(controller),
            Err(err) => {
                warn!(error = %err, "failed to initialize system tray; continuing without tray icon");
                None
            }
        };

        let bounds = Bounds::centered(None, size(px(win32::SIDEBAR_WIDTH_DIP), px(680.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::PopUp,
            window_background: WindowBackgroundAppearance::Transparent,
            focus: false,
            show: true,
            ..Default::default()
        };

        let window_handle = match cx.open_window(options, |_window, cx| {
            cx.new(|_| HexOverlayApp::new(event_rx, core_handle, tray))
        }) {
            Ok(handle) => handle,
            Err(err) => {
                error!(error = %err, "failed to open HexOverlay window");
                return;
            }
        };

        let mut badge_hwnd_raw: isize = 0;
        let badge_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::PopUp,
            window_background: WindowBackgroundAppearance::Transparent,
            focus: false,
            show: true,
            ..Default::default()
        };
        let badge_window_handle = match cx.open_window(badge_options, |window, cx| {
            if let Ok(raw) = win32::window_hwnd_raw(window) {
                badge_hwnd_raw = raw;
            }
            cx.new(|_| LobbyBadgeOverlay::new())
        }) {
            Ok(handle) => Some(handle),
            Err(err) => {
                warn!(error = %err, "failed to open HexOverlay client badge window");
                None
            }
        };

        cx.spawn(async move |cx: &mut AsyncApp| loop {
            cx.background_executor().timer(EVENT_POLL_INTERVAL).await;
            let (main_cmd, badge_state, badge_cmd) =
                match window_handle.update(cx, |app, window, cx| {
                    let main_cmd = app.tick(window, cx);
                    let (badge_state, badge_cmd) = if badge_window_handle.is_some() {
                        app.build_lobby_badge_sync(badge_hwnd_raw)
                    } else {
                        (LobbyBadgeState::default(), None)
                    };
                    (main_cmd, badge_state, badge_cmd)
                }) {
                    Ok(tuple) => tuple,
                    Err(_) => break,
                };

            if let Some(ref badge_handle) = badge_window_handle {
                let _ = badge_handle.update(cx, |overlay, _window, cx| {
                    overlay.update_state(badge_state, cx);
                });
            }

            if let Some(cmd) = main_cmd {
                if let Err(err) = win32::apply_window_command(cmd) {
                    warn!(error = %err, ?cmd.action, "failed to apply deferred Win32 window command");
                }
                if window_handle
                    .update(cx, |_app, window, cx| {
                        window.refresh();
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }

            if let Some(cmd) = badge_cmd {
                if let Err(err) = win32::apply_window_command(cmd) {
                    warn!(error = %err, ?cmd.action, "failed to apply deferred client badge window command");
                }
                if let Some(ref badge_handle) = badge_window_handle {
                    let _ = badge_handle.update(cx, |_overlay, window, cx| {
                        window.refresh();
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    });
}
