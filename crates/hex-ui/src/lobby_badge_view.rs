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
                    .rounded(px(2.0))
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

/// Returns `(main_text_and_bar, badge_bg, border)` for the Hextech team strip tier accents.
fn hextech_tier_colors(tier: HeroTier) -> (Rgba, Rgba, Rgba) {
    match tier {
        HeroTier::SPlus => (rgb(0xF0C46C), rgba(0xF0C46C29), rgba(0xF0C46C80)),
        HeroTier::S => (rgb(0x2DD4BF), rgba(0x2DD4BF29), rgba(0x2DD4BF73)),
        HeroTier::A | HeroTier::B => (rgb(0x4ADE80), rgba(0x4ADE8024), rgba(0x4ADE8066)),
        HeroTier::C | HeroTier::D => (rgb(0x94A3B8), rgba(0x94A3B824), rgba(0x94A3B859)),
    }
}

fn premade_ghost_colors(group_id: u8) -> (Rgba, Rgba, Rgba) {
    match group_id {
        1 => (rgba(0x14B8A62E), rgba(0x2DD4BF73), rgb(0x5EEAD4)),
        2 => (rgba(0xA855F72E), rgba(0xC084FC73), rgb(0xD8B4FE)),
        _ => (rgba(0x0EA5E92E), rgba(0x38BDF873), rgb(0x7DD3FC)),
    }
}

fn scrim_layer(width_dip: f32, bg: Rgba) -> Div {
    div()
        .absolute()
        .top(px(0.0))
        .right(px(2.0))
        .w(px(width_dip))
        .h_full()
        .bg(bg)
}

fn ghost_pill(bg: Rgba, border: Rgba, text_color: Rgba, label: String) -> Div {
    div()
        .h(px(17.0))
        .px(px(4.5))
        .rounded(px(2.0))
        .bg(bg)
        .border_1()
        .border_color(border)
        .flex()
        .items_center()
        .text_size(px(9.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(text_color)
        .child(label)
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
        let row_h = (row_pitch_h - 2.0).max(20.0);
        let sw = self.state.strip_width_dip.max(112.0);

        let mut canvas: Div = div().size_full().relative().bg(rgba(0x00000000));

        for (idx, slot) in self.state.slots.iter().take(5).enumerate() {
            if slot.hero.is_none() && slot.summoner.is_none() {
                continue;
            }

            let tier_accent = slot.hero.as_ref().map(|b| hextech_tier_colors(b.tier));
            let tier_bar_color = tier_accent
                .map(|(main_color, _, _)| main_color)
                .unwrap_or_else(|| rgba(0x94A3B866));

            let mut content_col = div()
                .relative()
                .size_full()
                .pl(px(14.0))
                .pr(px(10.0))
                .py(px(4.0))
                .flex()
                .flex_col()
                .justify_center()
                .items_end()
                .gap(px(3.5))
                .overflow_hidden();

            if let Some(badge) = slot.hero.as_ref() {
                let (tier_main, tier_bg, tier_border) = hextech_tier_colors(badge.tier);
                content_col = content_col.child(
                    div()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(3.0))
                        .bg(rgba(0x0F172AA6))
                        .border_1()
                        .border_color(tier_border)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(5.0))
                        .overflow_hidden()
                        .child(
                            div()
                                .px(px(3.0))
                                .rounded(px(2.0))
                                .bg(tier_bg)
                                .text_size(px(12.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(tier_main)
                                .child(badge.tier.label()),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(0xF1F5F9))
                                .font_family("Consolas")
                                .child(format!("{:.1}%", badge.win_rate)),
                        ),
                );
            }

            if let Some(sum) = slot.summoner.as_ref() {
                let mut line2 = div()
                    .px(px(5.0))
                    .py(px(1.0))
                    .rounded(px(2.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(3.0))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .overflow_hidden();

                line2 = match sum.status {
                    SummonerRatingStatus::Ready => {
                        let (bg, score_color, title_color) = match sum.tier_index {
                            0 => (rgba(0x0F172A73), rgb(0xFDE047), rgb(0xF0C46C)),
                            1 => (rgba(0x0F172A73), rgb(0xF0C46C), rgb(0xE2E8F0)),
                            2 => (rgba(0x0F172A73), rgb(0x4ADE80), rgb(0xE2E8F0)),
                            3 => (rgba(0x0F172A73), rgb(0xCBD5E1), rgb(0x94A3B8)),
                            _ => (rgba(0xEF444424), rgb(0xFDA4AF), rgb(0xF87171)),
                        };
                        let compact_title: String = sum.title.chars().take(4).collect();
                        line2
                            .bg(bg)
                            .child(div().text_color(score_color).child(sum.score.to_string()))
                            .child(div().text_color(rgb(0x64748B)).child("·"))
                            .child(div().text_color(title_color).child(compact_title))
                    }
                    SummonerRatingStatus::Private => line2
                        .bg(rgba(0x0F172A73))
                        .child(div().text_color(rgb(0x94A3B8)).child("隐藏")),
                    SummonerRatingStatus::InsufficientSamples => line2
                        .bg(rgba(0x0F172A73))
                        .child(div().text_color(rgb(0x94A3B8)).child("样本少")),
                    SummonerRatingStatus::Loading => line2
                        .bg(rgba(0x0F172A73))
                        .child(div().text_color(rgb(0x94A3B8)).child("战绩…")),
                };

                content_col = content_col.child(line2);

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
                        let (p_bg, p_border, p_text) = premade_ghost_colors(gid);
                        let label = if has_all_three {
                            format!("● {gid}")
                        } else {
                            format!("● 黑{gid}")
                        };
                        tags = tags.child(ghost_pill(p_bg, p_border, p_text, label));
                    }

                    if sum.is_premade_last_mvp {
                        tags = tags.child(ghost_pill(
                            rgba(0xEAB3082E),
                            rgba(0xFACC1580),
                            rgb(0xFDE047),
                            "★ MVP".to_owned(),
                        ));
                    }

                    if sum.streak >= 3 {
                        tags = tags.child(ghost_pill(
                            rgba(0x22C55E2E),
                            rgba(0x4ADE8073),
                            rgb(0x86EFAC),
                            format!("{}连胜", sum.streak),
                        ));
                    } else if sum.streak <= -3 {
                        tags = tags.child(ghost_pill(
                            rgba(0xEF44442E),
                            rgba(0xF8717180),
                            rgb(0xFCA5A5),
                            format!("{}连败", sum.streak.unsigned_abs()),
                        ));
                    }

                    content_col = content_col.child(tags);
                }
            }

            let row_inner = div()
                .relative()
                .size_full()
                .child(scrim_layer(sw * 0.96, rgba(0x09142818)))
                .child(scrim_layer(sw * 0.82, rgba(0x09142826)))
                .child(scrim_layer(sw * 0.68, rgba(0x09142838)))
                .child(scrim_layer(sw * 0.52, rgba(0x0914284D)))
                .child(scrim_layer(sw * 0.36, rgba(0x091428B8)))
                .child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .right(px(0.0))
                        .w(px(2.0))
                        .h_full()
                        .bg(tier_bar_color),
                )
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
            return div().size_full().bg(rgba(0x00000000));
        }

        let w = self.state.client_width_dip;
        let h = self.state.client_height_dip;
        let col_w = w * PICK_CARD_WIDTH_FRAC;
        let badge_h = (h * PICK_CARD_BADGE_HEIGHT_FRAC).clamp(22.0, 38.0);
        let font_px = (badge_h * 0.42).clamp(10.0, 14.0);
        let show_name = col_w >= 140.0;

        let mut canvas: Div = div().size_full().relative().bg(rgba(0x00000000));

        for (idx, slot) in self.state.cards.iter().take(3).enumerate() {
            let Some(card) = slot.as_ref() else {
                continue;
            };
            let (tier_main, tier_bg, tier_border) = hextech_tier_colors(card.tier);
            let left = idx as f32 * (w * PICK_CARD_PITCH_FRAC);
            let compact_name: String = card.name_zh.chars().take(5).collect();

            let mut pill = div()
                .h(px(badge_h))
                .px(px(8.0))
                .rounded(px(3.0))
                .bg(rgba(0x091428EB))
                .border_1()
                .border_color(tier_border)
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
                        .rounded(px(2.0))
                        .bg(tier_bg)
                        .border_1()
                        .border_color(tier_border)
                        .font_weight(FontWeight::BOLD)
                        .text_color(tier_main)
                        .child(card.tier.label()),
                );

            if show_name && !compact_name.is_empty() {
                pill = pill.child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(0xE2E8F0))
                        .child(compact_name),
                );
            }

            pill = pill.child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xF1F5F9))
                    .font_family("Consolas")
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
