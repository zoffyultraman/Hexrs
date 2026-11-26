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
use crate::lobby_badge_view::{
    BenchBadgeOverlay, BenchBadgeState, TeamBadgeOverlay, TeamBadgeState,
};
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

        let mut bench_hwnd_raw: isize = 0;
        let bench_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::PopUp,
            window_background: WindowBackgroundAppearance::Transparent,
            focus: false,
            show: true,
            ..Default::default()
        };
        let bench_window_handle = match cx.open_window(bench_options, |window, cx| {
            if let Ok(raw) = win32::window_hwnd_raw(window) {
                bench_hwnd_raw = raw;
            }
            cx.new(|_| BenchBadgeOverlay::new())
        }) {
            Ok(handle) => Some(handle),
            Err(err) => {
                warn!(error = %err, "failed to open HexOverlay bench badge strip window");
                None
            }
        };

        let mut team_hwnd_raw: isize = 0;
        let team_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::PopUp,
            window_background: WindowBackgroundAppearance::Transparent,
            focus: false,
            show: true,
            ..Default::default()
        };
        let team_window_handle = match cx.open_window(team_options, |window, cx| {
            if let Ok(raw) = win32::window_hwnd_raw(window) {
                team_hwnd_raw = raw;
            }
            cx.new(|_| TeamBadgeOverlay::new())
        }) {
            Ok(handle) => Some(handle),
            Err(err) => {
                warn!(error = %err, "failed to open HexOverlay team badge strip window");
                None
            }
        };

        cx.spawn(async move |cx: &mut AsyncApp| loop {
            cx.background_executor().timer(EVENT_POLL_INTERVAL).await;
            let (main_cmd, bench_state, bench_cmd, team_state, team_cmd) =
                match window_handle.update(cx, |app, window, cx| {
                    let main_cmd = app.tick(window, cx);
                    let active_bench_hwnd = if bench_window_handle.is_some() {
                        bench_hwnd_raw
                    } else {
                        0
                    };
                    let active_team_hwnd = if team_window_handle.is_some() {
                        team_hwnd_raw
                    } else {
                        0
                    };
                    if active_bench_hwnd == 0 && active_team_hwnd == 0 {
                        (
                            main_cmd,
                            BenchBadgeState::default(),
                            None,
                            TeamBadgeState::default(),
                            None,
                        )
                    } else {
                        let (bench_state, bench_cmd, team_state, team_cmd) =
                            app.build_lobby_badge_sync(active_bench_hwnd, active_team_hwnd);
                        (main_cmd, bench_state, bench_cmd, team_state, team_cmd)
                    }
                }) {
                    Ok(tuple) => tuple,
                    Err(_) => break,
                };

            if let Some(ref bench_handle) = bench_window_handle {
                let _ = bench_handle.update(cx, |overlay, _window, cx| {
                    overlay.update_state(bench_state, cx);
                });
            }

            if let Some(ref team_handle) = team_window_handle {
                let _ = team_handle.update(cx, |overlay, _window, cx| {
                    overlay.update_state(team_state, cx);
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

            if let Some(cmd) = bench_cmd {
                if let Err(err) = win32::apply_window_command(cmd) {
                    warn!(error = %err, ?cmd.action, "failed to apply deferred bench badge strip command");
                }
                if let Some(ref bench_handle) = bench_window_handle {
                    let _ = bench_handle.update(cx, |_overlay, window, cx| {
                        window.refresh();
                        cx.notify();
                    });
                }
            }

            if let Some(cmd) = team_cmd {
                if let Err(err) = win32::apply_window_command(cmd) {
                    warn!(error = %err, ?cmd.action, "failed to apply deferred team badge strip command");
                }
                if let Some(ref team_handle) = team_window_handle {
                    let _ = team_handle.update(cx, |_overlay, window, cx| {
                        window.refresh();
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    });
}
