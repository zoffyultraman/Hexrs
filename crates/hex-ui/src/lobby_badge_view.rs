//! Click-through, DirectComposition-safe razor-thin strip overlays anchored strictly to
//! non-interactive regions of `LeagueClientUx.exe` (`RCLIENT`) during Champ Select.

use gpui::{div, prelude::*, px, rgba, Context, Div, FontWeight, Render};
use hex_core::HeroTier;

use crate::theme;
use crate::win32::{
    BENCH_SLOT_HEIGHT_FRAC, BENCH_SLOT_PITCH_FRAC, BENCH_SLOT_WIDTH_FRAC,
    PICK_CARD_BADGE_HEIGHT_FRAC, PICK_CARD_PITCH_FRAC, PICK_CARD_WIDTH_FRAC,
};

const TEAM_ROW_PITCH_FRAC: f32 = 0.1129;
const TEAM_BADGE_HEIGHT_FRAC: f32 = 0.032;

/// Single champion strength badge rendered onto a Champ Select strip overlay.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotBadgeData {
    pub tier: HeroTier,
    pub win_rate: f64,
}

/// Immutable frame snapshot synced from `HexrsApp` to `BenchBadgeOverlay`.
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
        let raw_slot_w = w * BENCH_SLOT_WIDTH_FRAC;
        let slot_w = raw_slot_w.clamp(40.0, 140.0);
        let slot_h = (h * BENCH_SLOT_HEIGHT_FRAC).clamp(18.0, 40.0);
        let bench_font_px = (slot_w * 0.140).clamp(8.5, 11.2);

        let mut canvas: Div = div().size_full().relative().bg(rgba(0x00000000));

        for (idx, slot) in self.state.slots.iter().take(10).enumerate() {
            let Some(badge) = slot.as_ref() else {
                continue;
            };
            let (_bg, border_color, text_color) = theme::tier_colors(badge.tier);
            let left = idx as f32 * (w * BENCH_SLOT_PITCH_FRAC) + (raw_slot_w - slot_w) * 0.5;

            canvas = canvas.child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(0.0))
                    .w(px(slot_w))
                    .h(px(slot_h))
                    .overflow_hidden()
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

/// Immutable frame snapshot synced from `HexrsApp` to `TeamBadgeOverlay`.
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
        let badge_h = (h * TEAM_BADGE_HEIGHT_FRAC).clamp(18.0, 30.0);
        let team_font_px = (badge_h * 0.40).clamp(9.2, 11.8);

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
                    .px(px(5.0))
                    .rounded_md()
                    .bg(rgba(0x0B101BE8))
                    .border_1()
                    .border_color(theme::border_gold())
                    .overflow_hidden()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(3.0))
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

/// Single champion strength + name badge rendered below an ARAM pick card.
#[derive(Debug, Clone, PartialEq)]
pub struct CardSlotBadgeData {
    pub tier: HeroTier,
    pub win_rate: f64,
    pub name_zh: String,
}

/// Immutable frame snapshot synced from `HexrsApp` to `CardBadgeOverlay`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CardBadgeState {
    pub visible: bool,
    pub cards: Vec<Option<CardSlotBadgeData>>,
    pub client_width_dip: f32,
    pub client_height_dip: f32,
}

/// Narrow horizontal strip window strictly below the 2 or 3 center champion pick cards.
pub struct CardBadgeOverlay {
    state: CardBadgeState,
}

impl CardBadgeOverlay {
    /// Creates an initially hidden pick-card badge strip overlay.
    pub fn new() -> Self {
        Self {
            state: CardBadgeState::default(),
        }
    }

    /// Replaces `state` and notifies GPUI only when geometry, visibility, or card slots changed.
    pub fn update_state(&mut self, next: CardBadgeState, cx: &mut Context<Self>) {
        if self.state != next {
            self.state = next;
            cx.notify();
        }
    }
}

impl Render for CardBadgeOverlay {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if !self.state.visible
            || self.state.client_width_dip <= 100.0
            || self.state.client_height_dip <= 100.0
            || !(2..=3).contains(&self.state.cards.len())
            || self.state.cards.iter().all(Option::is_none)
        {
            return div().size_full().bg(rgba(0x00000000));
        }

        let w = self.state.client_width_dip;
        let h = self.state.client_height_dip;
        let col_w = w * PICK_CARD_WIDTH_FRAC;
        let badge_h = (h * PICK_CARD_BADGE_HEIGHT_FRAC).clamp(22.0, 44.0);
        let font_px = (badge_h * 0.42).clamp(10.0, 14.0);
        let show_name = col_w >= 140.0;

        let mut canvas: Div = div().size_full().relative().bg(rgba(0x00000000));

        for (idx, slot) in self.state.cards.iter().take(3).enumerate() {
            let Some(card) = slot.as_ref() else {
                continue;
            };
            let (bg, border, text) = theme::tier_colors(card.tier);
            let left = idx as f32 * (w * PICK_CARD_PITCH_FRAC);
            let compact_name: String = card.name_zh.chars().take(5).collect();

            let mut pill = div()
                .h(px(badge_h))
                .px(px(8.0))
                .rounded_md()
                .bg(rgba(0x0B101BE8))
                .border_1()
                .border_color(theme::border_gold())
                .overflow_hidden()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .text_size(px(font_px))
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
                        .child(card.tier.label()),
                );

            if show_name && !compact_name.is_empty() {
                pill = pill.child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::text_primary())
                        .child(compact_name),
                );
            }

            pill = pill.child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::cyan())
                    .child(format!("{:.1}%", card.win_rate)),
            );

            canvas = canvas.child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(0.0))
                    .w(px(col_w))
                    .h(px(badge_h))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .child(pill),
            );
        }

        canvas
    }
}
