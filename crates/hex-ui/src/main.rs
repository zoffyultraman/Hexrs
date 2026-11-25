#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod win32;

use gpui::{
    div, prelude::*, px, rgba, size, App, Application, Bounds, Context, FontWeight, Render,
    Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
};
use tracing_subscriber::EnvFilter;

/// Minimal hextech transparent overlay card rendered in Phase 1 to verify Win32 overlay behavior.
struct HexOverlayDemo;

impl Render for HexOverlayDemo {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_3()
            .flex()
            .flex_col()
            .justify_between()
            .rounded_xl()
            .border_1()
            .border_color(rgba(0xC8AA6E99))
            .bg(rgba(0x0A1428D9))
            .shadow_lg()
            .text_color(rgba(0xF0E6D2FF))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgba(0xC8AA6EFF))
                            .child("HEXOVERLAY // PHASE 1"),
                    )
                    .child(
                        div()
                            .px_2()
                            .py(px(2.0))
                            .rounded_md()
                            .border_1()
                            .border_color(rgba(0x0AC8B999))
                            .bg(rgba(0x0AC8B933))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgba(0x0AC8B9FF))
                            .child("TOP-MOST"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgba(0xA09B8CFF))
                            .child("Transparent click-through GPUI overlay initialized"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgba(0x0AC8B9FF))
                            .child("WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TRANSPARENT"),
                    ),
            )
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

fn main() {
    init_tracing();

    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(360.0), px(120.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::PopUp,
            window_background: WindowBackgroundAppearance::Transparent,
            focus: false,
            show: true,
            ..Default::default()
        };

        if let Err(err) = cx.open_window(options, |window, cx| {
            if let Err(style_err) = win32::apply_overlay_styles(window) {
                tracing::warn!(error = %style_err, "failed to apply Win32 overlay styles");
            }
            cx.new(|_| HexOverlayDemo)
        }) {
            tracing::error!(error = %err, "failed to open HexOverlay window");
        }
    });
}
