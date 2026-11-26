//! Click-through, DirectComposition-safe in-place Champ Select hero tier + win-rate badges
//! overlaid directly onto `LeagueClientUx.exe` (`RCLIENT`).

use gpui::{div, prelude::*, px, rgba, Context, Div, FontWeight, Render};
use hex_core::HeroTier;

use crate::theme;

const BENCH_SLOT0_CX: f32 = 0.2960;
const BENCH_SLOT_PITCH: f32 = 0.0464;
const BENCH_BADGE_TOP_Y: f32 = 0.0835;

const TEAM_BADGE_LEFT_X: f32 = 0.2980;
const TEAM_ROW0_CY: f32 = 0.1826;
const TEAM_ROW_PITCH: f32 = 0.1129;

/// Single champion strength badge rendered onto the League client window.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotBadgeData {
    pub tier: HeroTier,
    pub win_rate: f64,
}

/// Immutable frame snapshot synced from `HexOverlayApp` to `LobbyBadgeOverlay`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LobbyBadgeState {
    pub visible: bool,
    pub bench_slots: Vec<Option<SlotBadgeData>>,
    pub team_slots: Vec<Option<SlotBadgeData>>,
    pub client_width_dip: f32,
    pub client_height_dip: f32,
}

/// Transparent secondary GPUI window entity rendering in-place Champ Select badges.
pub struct LobbyBadgeOverlay {
    state: LobbyBadgeState,
}

impl LobbyBadgeOverlay {
    /// Creates an initially hidden badge overlay.
    pub fn new() -> Self {
        Self {
            state: LobbyBadgeState::default(),
        }
    }

    /// Replaces `state` and notifies GPUI only when geometry, visibility, or slot values changed.
    pub fn update_state(&mut self, next: LobbyBadgeState, cx: &mut Context<Self>) {
        if self.state != next {
            self.state = next;
            cx.notify();
        }
    }
}

impl Render for LobbyBadgeOverlay {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if !self.state.visible
            || self.state.client_width_dip <= 100.0
            || self.state.client_height_dip <= 100.0
        {
            return div().size_full().bg(rgba(0x00000000));
        }

        let w = self.state.client_width_dip;
        let h = self.state.client_height_dip;

        let mut canvas: Div = div().size_full().relative().bg(rgba(0x00000000));

        let slot_w = (w * 0.0440).clamp(44.0, 92.0);
        let slot_h = (h * 0.0285).clamp(18.0, 30.0);
        let bench_font_px = (slot_w * 0.175).clamp(9.0, 12.0);

        for (idx, slot) in self.state.bench_slots.iter().take(10).enumerate() {
            let Some(badge) = slot.as_ref() else {
                continue;
            };
            let (_bg, border_color, text_color) = theme::tier_colors(badge.tier);
            let left = w * (BENCH_SLOT0_CX + idx as f32 * BENCH_SLOT_PITCH) - slot_w * 0.5;
            let top = h * BENCH_BADGE_TOP_Y;

            canvas = canvas.child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(slot_w))
                    .h(px(slot_h))
                    .rounded_md()
                    .bg(rgba(0x0B101BE8))
                    .border_1()
                    .border_color(border_color)
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap(px(2.0))
                    .text_size(px(bench_font_px))
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_color)
                            .child(badge.tier.label()),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(format!("{:.1}%", badge.win_rate)),
                    ),
            );
        }

        let badge_h = (h * 0.038).clamp(22.0, 34.0);
        let team_font_px = (badge_h * 0.46).clamp(11.0, 14.0);

        for (idx, slot) in self.state.team_slots.iter().take(5).enumerate() {
            let Some(badge) = slot.as_ref() else {
                continue;
            };
            let (bg, border, text) = theme::tier_colors(badge.tier);
            let left = w * TEAM_BADGE_LEFT_X;
            let top = h * (TEAM_ROW0_CY + idx as f32 * TEAM_ROW_PITCH) - badge_h * 0.5;

            canvas = canvas.child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .h(px(badge_h))
                    .px(px(6.0))
                    .rounded_md()
                    .bg(rgba(0x0B101BE8))
                    .border_1()
                    .border_color(theme::border_gold())
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .text_size(px(team_font_px))
                    .child(
                        div()
                            .px(px(4.0))
                            .py(px(1.0))
                            .rounded_sm()
                            .bg(bg)
                            .border_1()
                            .border_color(border)
                            .font_weight(FontWeight::BOLD)
                            .text_color(text)
                            .child(badge.tier.label()),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::cyan())
                            .child(format!("{:.1}%", badge.win_rate)),
                    ),
            );
        }

        canvas
    }
}
