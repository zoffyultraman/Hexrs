//! Click-through, DirectComposition-safe razor-thin strip overlays anchored strictly to
//! non-interactive regions of `LeagueClientUx.exe` (`RCLIENT`) during Champ Select.

use gpui::{div, prelude::*, px, rgb, rgba, Context, Div, FontWeight, Render, Rgba};
use hex_core::{HeroTier, SummonerRatingStatus};

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

/// Per-slot ARAM Mayhem summoner rating, streak, and premade summary badge data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummonerBadgeData {
    pub status: SummonerRatingStatus,
    pub score: u8,
    pub tier_index: u8,
    pub title: String,
    pub streak: i8,
    pub premade_group_id: Option<u8>,
    pub is_premade_last_mvp: bool,
}

/// Combined hero meta and summoner rating data for a single allied slot row.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TeamSlotBadgeEntry {
    pub hero: Option<SlotBadgeData>,
    pub summoner: Option<SummonerBadgeData>,
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
    pub slots: Vec<TeamSlotBadgeEntry>,
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

fn rating_colors(status: SummonerRatingStatus, tier_index: u8) -> (Rgba, Rgba, Rgba) {
    match status {
        SummonerRatingStatus::Ready => match tier_index {
            0 => (rgba(0xC8AA6E38), rgba(0xF0C75EBF), rgb(0xFFD97D)),
            1 => (rgba(0x0AC8B933), rgba(0x0AC8B9AA), rgb(0x4BE8DB)),
            2 => (rgba(0x94A3B830), rgba(0xCBD5E188), rgb(0xE2E8F0)),
            3 => (rgba(0xF59E0B2B), rgba(0xFBBF2488), rgb(0xFCD34D)),
            _ => (rgba(0xE840572E), rgba(0xE8405799), rgb(0xFDA4AF)),
        },
        SummonerRatingStatus::Private => (rgba(0x33415555), rgba(0x64748B77), rgb(0x94A3B8)),
        SummonerRatingStatus::InsufficientSamples => {
            (rgba(0x33415544), rgba(0x64748B66), rgb(0xCBD5E1))
        }
        SummonerRatingStatus::Loading => (rgba(0x1E293B88), rgba(0x47556966), rgb(0x94A3B8)),
    }
}

fn premade_group_colors(group_id: u8, is_mvp: bool) -> (Rgba, Rgba, Rgba) {
    let (bg, base_border, base_text) = match group_id {
        1 => (rgba(0x0AC8B933), rgba(0x0AC8B9AA), rgb(0x4BE8DB)),
        2 => (rgba(0x8B5CF638), rgba(0xA78BFA99), rgb(0xDDD6FE)),
        _ => (rgba(0x14B8A633), rgba(0x2DD4BF99), rgb(0x99F6E4)),
    };
    if is_mvp {
        (bg, rgba(0xF0C75EE6), rgb(0xFFD97D))
    } else {
        (bg, base_border, base_text)
    }
}

impl Render for TeamBadgeOverlay {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if !self.state.visible
            || self.state.client_height_dip <= 100.0
            || self
                .state
                .slots
                .iter()
                .all(|s| s.hero.is_none() && s.summoner.is_none())
        {
            return div().size_full().bg(rgba(0x00000000));
        }

        let h = self.state.client_height_dip;
        let row_pitch_h = TEAM_ROW_PITCH_FRAC * h;
        let line1_h = (h * 0.028).clamp(17.0, 25.0);
        let line1_font = (line1_h * 0.42).clamp(8.8, 11.0);
        let line2_h = (h * 0.025).clamp(16.0, 22.0);
        let line2_font = (line2_h * 0.43).clamp(8.5, 10.5);
        let line3_h = (h * 0.021).clamp(14.0, 18.0);
        let line3_font = (line3_h * 0.48).clamp(7.8, 9.5);

        let mut canvas: Div = div().size_full().relative().bg(rgba(0x00000000));

        for (idx, slot) in self.state.slots.iter().take(5).enumerate() {
            if slot.hero.is_none() && slot.summoner.is_none() {
                continue;
            }

            let mut row_col = div()
                .absolute()
                .left(px(0.0))
                .top(px(idx as f32 * row_pitch_h))
                .w_full()
                .h(px(row_pitch_h))
                .flex()
                .flex_col()
                .justify_center()
                .gap(px(2.0))
                .overflow_hidden();

            if let Some(badge) = slot.hero.as_ref() {
                let (bg, border, text) = theme::tier_colors(badge.tier);
                row_col = row_col.child(
                    div().flex().flex_row().child(
                        div()
                            .h(px(line1_h))
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
                            .text_size(px(line1_font))
                            .child(
                                div()
                                    .px(px(3.0))
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
                    ),
                );
            }

            if let Some(sum) = slot.summoner.as_ref() {
                let (r_bg, r_border, r_text) = rating_colors(sum.status, sum.tier_index);
                let mut rating_pill = div()
                    .h(px(line2_h))
                    .px(px(5.0))
                    .rounded_md()
                    .bg(rgba(0x0B101BE8))
                    .border_1()
                    .border_color(r_border)
                    .overflow_hidden()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(3.0))
                    .text_size(px(line2_font));

                match sum.status {
                    SummonerRatingStatus::Ready => {
                        let compact_title: String = sum.title.chars().take(4).collect();
                        rating_pill = rating_pill
                            .child(
                                div()
                                    .px(px(3.0))
                                    .rounded_sm()
                                    .bg(r_bg)
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(r_text)
                                    .child(sum.score.to_string()),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(r_text)
                                    .child(compact_title),
                            );
                    }
                    SummonerRatingStatus::Private => {
                        let label = if sum.title.trim().is_empty() {
                            "隐藏".to_owned()
                        } else {
                            sum.title.chars().take(4).collect()
                        };
                        rating_pill = rating_pill.child(div().text_color(r_text).child(label));
                    }
                    SummonerRatingStatus::InsufficientSamples => {
                        let label = if sum.title.trim().is_empty() {
                            "样本少".to_owned()
                        } else {
                            sum.title.chars().take(4).collect()
                        };
                        rating_pill = rating_pill.child(div().text_color(r_text).child(label));
                    }
                    SummonerRatingStatus::Loading => {
                        rating_pill = rating_pill.child(div().text_color(r_text).child("战绩…"));
                    }
                }

                row_col = row_col.child(div().flex().flex_row().child(rating_pill));

                let has_streak = sum.streak >= 3 || sum.streak <= -3;
                if sum.premade_group_id.is_some() || has_streak {
                    let mut tags = div()
                        .h(px(line3_h))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(3.0))
                        .text_size(px(line3_font))
                        .overflow_hidden();

                    if let Some(gid) = sum.premade_group_id {
                        let (p_bg, p_border, p_text) =
                            premade_group_colors(gid, sum.is_premade_last_mvp);
                        let label = if sum.is_premade_last_mvp {
                            format!("黑{gid}·MVP")
                        } else {
                            format!("黑{gid}")
                        };
                        tags = tags.child(
                            div()
                                .h(px(line3_h))
                                .px(px(4.0))
                                .rounded_sm()
                                .bg(p_bg)
                                .border_1()
                                .border_color(p_border)
                                .flex()
                                .items_center()
                                .font_weight(FontWeight::BOLD)
                                .text_color(p_text)
                                .child(label),
                        );
                    }

                    if sum.streak >= 3 {
                        tags = tags.child(
                            div()
                                .h(px(line3_h))
                                .px(px(4.0))
                                .rounded_sm()
                                .bg(rgba(0x10B98133))
                                .border_1()
                                .border_color(rgba(0x34D399AA))
                                .flex()
                                .items_center()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(0x6EE7B7))
                                .child(format!("{}连胜", sum.streak)),
                        );
                    } else if sum.streak <= -3 {
                        tags = tags.child(
                            div()
                                .h(px(line3_h))
                                .px(px(4.0))
                                .rounded_sm()
                                .bg(rgba(0xE840572E))
                                .border_1()
                                .border_color(rgba(0xE8405788))
                                .flex()
                                .items_center()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(0xFDA4AF))
                                .child(format!("{}连败", sum.streak.unsigned_abs())),
                        );
                    }

                    row_col = row_col.child(tags);
                }
            }

            canvas = canvas.child(row_col);
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
