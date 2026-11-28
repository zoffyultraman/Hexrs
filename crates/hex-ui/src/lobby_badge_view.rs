//! Click-through, DirectComposition-safe razor-thin strip overlays anchored strictly to
//! non-interactive regions of `LeagueClientUx.exe` (`RCLIENT`) during Champ Select.

use gpui::{
    div, linear_color_stop, linear_gradient, prelude::*, px, Context, Div, FontWeight, Render,
};
use hex_core::HeroTier;

use crate::components::{self, AccentBar, TierBadgeSize};
use crate::theme;
use crate::win32::{
    BENCH_SLOT_HEIGHT_FRAC, BENCH_SLOT_PITCH_FRAC, BENCH_SLOT_WIDTH_FRAC,
    PICK_CARD_BADGE_HEIGHT_FRAC, PICK_CARD_PITCH_FRAC, PICK_CARD_WIDTH_FRAC,
};

const TEAM_ROW_PITCH_FRAC: f32 = 0.1129;

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
            return div().size_full().bg(theme::transparent());
        }

        let w = self.state.client_width_dip;
        let h = self.state.client_height_dip;
        let raw_slot_w = w * BENCH_SLOT_WIDTH_FRAC;
        let slot_w = raw_slot_w.clamp(34.0, 140.0);
        let slot_h = (h * BENCH_SLOT_HEIGHT_FRAC).clamp(16.0, 18.0);
        let bench_font_px = (slot_w * 0.175).clamp(9.0, 11.0);
        let gap_px = if slot_w >= 48.0 { 4.0 } else { 2.5 };

        let mut canvas: Div = div().size_full().relative().bg(theme::transparent());

        for (idx, slot) in self.state.slots.iter().take(10).enumerate() {
            let Some(badge) = slot.as_ref() else {
                continue;
            };
            let pal = theme::tier_palette(badge.tier);
            let winrate_color = if matches!(badge.tier, HeroTier::SPlus | HeroTier::S) {
                theme::text_primary()
            } else {
                theme::text_secondary()
            };
            let left = idx as f32 * (w * BENCH_SLOT_PITCH_FRAC) + (raw_slot_w - slot_w) * 0.5;

            canvas = canvas.child(
                components::hud_container(
                    theme::bg_surface_dense(),
                    Some(theme::bench_frame_border(badge.tier)),
                    AccentBar::Bottom(2.0, pal.text),
                )
                .absolute()
                .left(px(left))
                .top(px(0.0))
                .w(px(slot_w))
                .h(px(slot_h))
                .overflow_hidden()
                .rounded_bl(px(2.0))
                .rounded_br(px(2.0))
                .child(
                    div()
                        .size_full()
                        .pb(px(2.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .gap(px(gap_px))
                        .text_size(px(bench_font_px))
                        .child(components::tier_badge(badge.tier, TierBadgeSize::Sm))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .font_family(theme::MONO_FONT)
                                .text_color(winrate_color)
                                .child(format!("{:.1}%", badge.win_rate)),
                        ),
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
    pub strip_width_dip: f32,
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
            || self
                .state
                .slots
                .iter()
                .all(|s| s.hero.is_none() && s.summoner.is_none())
        {
            return div().size_full().bg(theme::transparent());
        }

        let h = self.state.client_height_dip;
        let row_pitch_h = TEAM_ROW_PITCH_FRAC * h;
        let row_h = (row_pitch_h - 2.0).max(20.0);

        let mut canvas: Div = div().size_full().relative().bg(theme::transparent());

        for (idx, slot) in self.state.slots.iter().take(5).enumerate() {
            if slot.hero.is_none() && slot.summoner.is_none() {
                continue;
            }

            let tier_bar_color = slot
                .hero
                .as_ref()
                .map(|b| theme::tier_palette(b.tier).text)
                .unwrap_or_else(theme::border_cool);

            let mut content_col = div()
                .relative()
                .size_full()
                .pl(px(14.0))
                .pr(px(11.0))
                .py(px(4.0))
                .flex()
                .flex_col()
                .justify_center()
                .items_end()
                .gap(px(3.5))
                .overflow_hidden();

            if let Some(badge) = slot.hero.as_ref() {
                content_col = content_col.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_end()
                        .gap(px(6.0))
                        .child(components::tier_badge(badge.tier, TierBadgeSize::Md))
                        .child(
                            div()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme::text_primary())
                                .font_family(theme::MONO_FONT)
                                .child(format!("{:.1}%", badge.win_rate)),
                        ),
                );
            }

            if let Some(sum) = slot.summoner.as_ref() {
                content_col = content_col.child(components::player_score_line(sum));

                let has_streak = sum.streak >= 3 || sum.streak <= -3;
                if sum.premade_group_id.is_some() || has_streak {
                    let has_all_three =
                        sum.premade_group_id.is_some() && sum.is_premade_last_mvp && has_streak;
                    let mut tags = div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_end()
                        .gap(px(4.0))
                        .overflow_hidden();

                    if let Some(gid) = sum.premade_group_id {
                        tags = tags.child(components::premade_pill(gid, has_all_three));
                    }

                    if sum.is_premade_last_mvp {
                        tags = tags.child(components::mvp_pill());
                    }

                    if sum.streak >= 3 {
                        tags = tags.child(components::win_streak_pill(sum.streak));
                    } else if sum.streak <= -3 {
                        tags = tags.child(components::loss_streak_pill(sum.streak));
                    }

                    content_col = content_col.child(tags);
                }
            }

            let row_inner = components::hud_container(
                linear_gradient(
                    90.0,
                    linear_color_stop(theme::team_strip_gradient_start(), 0.0),
                    linear_color_stop(theme::team_strip_gradient_end(), 0.72),
                ),
                None,
                AccentBar::Right(3.0, tier_bar_color),
            )
            .size_full()
            .child(content_col);

            canvas = canvas.child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(idx as f32 * row_pitch_h + 1.0))
                    .w_full()
                    .h(px(row_h))
                    .overflow_hidden()
                    .child(row_inner),
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
            return div().size_full().bg(theme::transparent());
        }

        let w = self.state.client_width_dip;
        let h = self.state.client_height_dip;
        let col_w = w * PICK_CARD_WIDTH_FRAC;
        let badge_h = (h * PICK_CARD_BADGE_HEIGHT_FRAC).clamp(22.0, 38.0);
        let font_px = (badge_h * 0.42).clamp(10.0, 14.0);
        let show_name = col_w >= 140.0;

        let mut canvas: Div = div().size_full().relative().bg(theme::transparent());

        for (idx, slot) in self.state.cards.iter().take(3).enumerate() {
            let Some(card) = slot.as_ref() else {
                continue;
            };
            let pal = theme::tier_palette(card.tier);
            let left = idx as f32 * (w * PICK_CARD_PITCH_FRAC);
            let compact_name: String = card.name_zh.chars().take(5).collect();

            let mut pill =
                components::hud_container(theme::bg_surface_dense(), Some(pal.border), AccentBar::None)
                    .h(px(badge_h))
                    .px(px(8.0))
                    .rounded(px(3.0))
                    .overflow_hidden()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.0))
                    .text_size(px(font_px))
                    .child(components::tier_badge(card.tier, TierBadgeSize::Md));

            if show_name && !compact_name.is_empty() {
                pill = pill.child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::text_secondary())
                        .child(compact_name),
                );
            }

            pill = pill.child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_primary())
                    .font_family(theme::MONO_FONT)
                    .child(format!("{:.1}%", card.win_rate)),
            );

            canvas = canvas.child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(2.0))
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
