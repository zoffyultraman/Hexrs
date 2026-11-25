#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod lobby_view;
mod theme;
mod win32;

use std::time::Duration;

use gpui::{
    prelude::*, px, size, App, Application, AsyncApp, Bounds, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions,
};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::app::HexOverlayApp;

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(80);

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
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

    info!("HexOverlay starting GPUI event loop (Dormant until League client enters Champ Select or Match)");

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(win32::SIDEBAR_WIDTH_DIP), px(680.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::PopUp,
            window_background: WindowBackgroundAppearance::Transparent,
            focus: false,
            show: false,
            ..Default::default()
        };

        let window_handle = match cx.open_window(options, |_window, cx| {
            cx.new(|_| HexOverlayApp::new(event_rx, core_handle))
        }) {
            Ok(handle) => handle,
            Err(err) => {
                error!(error = %err, "failed to open HexOverlay window");
                return;
            }
        };

        cx.spawn(async move |cx: &mut AsyncApp| loop {
            cx.background_executor().timer(EVENT_POLL_INTERVAL).await;
            let alive = window_handle
                .update(cx, |app, window, cx| {
                    app.tick(window, cx);
                })
                .is_ok();
            if !alive {
                break;
            }
        })
        .detach();
    });
}
