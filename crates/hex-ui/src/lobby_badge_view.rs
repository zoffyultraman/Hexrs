//! Click-through, DirectComposition-safe razor-thin strip overlays anchored strictly to
//! non-interactive regions of `LeagueClientUx.exe` (`RCLIENT`) during Champ Select.

use gpui::{div, prelude::*, px, rgba, Context, Div, FontWeight, Render};
use hex_core::HeroTier;

use crate::theme;

const BENCH_SLOT_PITCH_FRAC: f32 = 0.0464;
const BENCH_SLOT_WIDTH_FRAC: f32 = 0.0440;
const BENCH_SLOT_HEIGHT_FRAC: f32 = 0.0285;

const TEAM_ROW_PITCH_FRAC: f32 = 0.1129;
const TEAM_BADGE_HEIGHT_FRAC: f32 = 0.038;

/// Single champion strength badge rendered onto a Champ Select strip overlay.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotBadgeData {
    pub tier: HeroTier,
    pub win_rate: f64,
}

/// Immutable frame snapshot synced from `HexOverlayApp` to `BenchBadgeOverlay`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchBadgeState {
    pub visible: bool,
    pub slots: Vec<Option<SlotBadgeData>>,
    pub client_width_dip: f32,
    pub client_height_dip: f32,
}

/// Narrow horizontal strip window strictly below the top 10 bench portraits.
pub struct BenchBadgeOverlay {
    state: BenchBadgeState,
}

impl BenchBadgeOverlay {
    /// Creates an initially hidden bench badge strip overlay.
    pub fn new() -> Self {
        Self {
            state: BenchBadgeState::default(),
        }
    }

    /// Replaces `state` and notifies GPUI only when geometry, visibility, or slot values changed.
    pub fn update_state(&mut self, next: BenchBadgeState, cx: &mut Context<Self>) {
        if self.state != next {
            self.state = next;
            cx.notify();
        }
    }
}

impl Render for BenchBadgeOverlay {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if !self.state.visible
            || self.state.client_width_dip <= 100.0
            || self.state.client_height_dip <= 100.0
            || self.state.slots.iter().all(Option::is_none)
        {
            return div().size_full().bg(rgba(0x00000000));
        }

        let w = self.state.client_width_dip;
        let h = self.state.client_height_dip;
        let slot_w = (w * BENCH_SLOT_WIDTH_FRAC).clamp(44.0, 92.0);
        let slot_h = (h * BENCH_SLOT_HEIGHT_FRAC).clamp(18.0, 30.0);
        let bench_font_px = (slot_w * 0.175).clamp(9.0, 12.0);

        let mut canvas: Div = div().size_full().relative().bg(rgba(0x00000000));

        for (idx, slot) in self.state.slots.iter().take(10).enumerate() {
            let Some(badge) = slot.as_ref() else {
                continue;
            };
            let (_bg, border_color, text_color) = theme::tier_colors(badge.tier);
            let left = idx as f32 * (BENCH_SLOT_PITCH_FRAC * w);

            canvas = canvas.child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(0.0))
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

        canvas
    }
}

/// Immutable frame snapshot synced from `HexOverlayApp` to `TeamBadgeOverlay`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TeamBadgeState {
    pub visible: bool,
    pub slots: Vec<Option<SlotBadgeData>>,
    pub client_height_dip: f32,
}

/// Narrow vertical strip window strictly to the right of the 5 allied hero cards.
pub struct TeamBadgeOverlay {
    state: TeamBadgeState,
}

impl TeamBadgeOverlay {
    /// Creates an initially hidden allied team badge strip overlay.
    pub fn new() -> Self {
        Self {
            state: TeamBadgeState::default(),
        }
    }

    /// Replaces `state` and notifies GPUI only when geometry, visibility, or slot values changed.
    pub fn update_state(&mut self, next: TeamBadgeState, cx: &mut Context<Self>) {
        if self.state != next {
            self.state = next;
            cx.notify();
        }
    }
}

impl Render for TeamBadgeOverlay {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if !self.state.visible
            || self.state.client_height_dip <= 100.0
            || self.state.slots.iter().all(Option::is_none)
        {
            return div().size_full().bg(rgba(0x00000000));
        }

        let h = self.state.client_height_dip;
        let badge_h = (h * TEAM_BADGE_HEIGHT_FRAC).clamp(22.0, 34.0);
        let team_font_px = (badge_h * 0.46).clamp(11.0, 14.0);

        let mut canvas: Div = div().size_full().relative().bg(rgba(0x00000000));

        for (idx, slot) in self.state.slots.iter().take(5).enumerate() {
            let Some(badge) = slot.as_ref() else {
                continue;
            };
            let (bg, border, text) = theme::tier_colors(badge.tier);
            let top = ((idx as f32 + 0.5) * (TEAM_ROW_PITCH_FRAC * h)) - badge_h * 0.5;

            canvas = canvas.child(
                div()
                    .absolute()
                    .left(px(0.0))
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
