//! Shared atomic Hextech HUD components (hexrs_hud.md Section 3).

use gpui::{
    div, linear_color_stop, linear_gradient, prelude::*, px, Background, Div, FontWeight, Rgba,
    SharedString,
};
use hex_core::{HeroTier, SummonerRatingStatus};

use crate::lobby_badge_view::SummonerBadgeData;
use crate::theme::{self, PillPalette};

/// Visual sizing presets for `tier_badge`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierBadgeSize {
    /// Bench strip nameplate: frameless bold tier letter inheriting parent font size.
    Sm,
    /// Allied team strip / sidebar / pick card strip: compact `12px` bordered chip.
    Md,
    /// In-game 3-choice augment strip: `30x30` `18px ExtraBold` square block.
    Lg,
}

/// Renders a unified strength tier indicator (`S+` .. `D`).
pub fn tier_badge(tier: HeroTier, size: TierBadgeSize) -> Div {
    let pal = theme::tier_palette(tier);
    match size {
        TierBadgeSize::Sm => div()
            .font_weight(FontWeight::BOLD)
            .text_color(pal.text)
            .child(tier.label()),
        TierBadgeSize::Md => div()
            .px(px(5.0))
            .py(px(1.0))
            .rounded(px(2.0))
            .bg(pal.bg)
            .border_1()
            .border_color(pal.border)
            .text_size(px(12.0))
            .font_weight(FontWeight::BOLD)
            .text_color(pal.text)
            .child(tier.label()),
        TierBadgeSize::Lg => div()
            .min_w(px(30.0))
            .h(px(30.0))
            .px(px(2.0))
            .rounded(px(3.0))
            .bg(theme::tier_lg_bg(tier))
            .border_1()
            .border_color(pal.border)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(18.0))
            .font_weight(FontWeight::EXTRA_BOLD)
            .text_color(pal.text)
            .child(tier.label()),
    }
}

/// Renders a compact Ghost Pill micro-tag (`16px` tall, `2px` radius, `1px` translucent border).
pub fn ghost_pill(pal: PillPalette, label: impl Into<SharedString>) -> Div {
    div()
        .h(px(16.0))
        .px(px(5.0))
        .rounded(px(2.0))
        .bg(pal.bg)
        .border_1()
        .border_color(pal.border)
        .flex()
        .items_center()
        .text_size(px(10.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(pal.text)
        .child(label.into())
}

/// Renders a premade party Ghost Pill (`● 黑1` or compact `● 1` when 3 tags share one line).
pub fn premade_pill(group_id: u8, compact: bool) -> Div {
    let label = if compact {
        format!("● {group_id}")
    } else {
        format!("● 黑{group_id}")
    };
    ghost_pill(theme::premade_pill_palette(group_id), label)
}

/// Renders the premade last-game MVP Ghost Pill (`★ MVP`, crisp non-emoji glyph).
pub fn mvp_pill() -> Div {
    ghost_pill(theme::mvp_pill_palette(), "★ MVP")
}

/// Renders a warm-orange win-streak Ghost Pill (`{streak}连胜`).
pub fn win_streak_pill(streak: i8) -> Div {
    ghost_pill(theme::win_streak_pill_palette(), format!("{streak}连胜"))
}

/// Renders a coral-red loss-streak Ghost Pill (`{n}连败`).
pub fn loss_streak_pill(streak: i8) -> Div {
    ghost_pill(
        theme::loss_streak_pill_palette(),
        format!("{}连败", streak.unsigned_abs()),
    )
}

/// Renders the straddling `★ 推荐` gold corner badge for recommended augment cards.
pub fn recommended_badge() -> Div {
    div()
        .h(px(18.0))
        .px(px(7.0))
        .rounded(px(3.0))
        .bg(linear_gradient(
            180.0,
            linear_color_stop(theme::recommended_gold_start(), 0.0),
            linear_color_stop(theme::recommended_gold_end(), 1.0),
        ))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.5))
        .font_weight(FontWeight::BOLD)
        .text_color(theme::recommended_text())
        .child("★ 推荐")
}

/// Draws a crisp `10x10` geometric player silhouette icon without relying on emoji fonts.
pub fn player_silhouette_icon() -> Div {
    let fg = theme::text_muted();
    div()
        .w(px(10.0))
        .h(px(10.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(1.0))
        .child(div().w(px(4.0)).h(px(4.0)).rounded_full().bg(fg))
        .child(
            div()
                .w(px(8.0))
                .h(px(4.0))
                .rounded_tl(px(4.0))
                .rounded_tr(px(4.0))
                .bg(fg),
        )
}

/// Renders the frameless Line 2 summoner rating / status row with `Consolas` score alignment.
pub fn player_score_line(sum: &SummonerBadgeData) -> Div {
    let line = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_end()
        .gap(px(3.5))
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .overflow_hidden()
        .child(player_silhouette_icon());

    match sum.status {
        SummonerRatingStatus::Ready => {
            let (score_color, title_color) = theme::player_score_colors(sum.tier_index);
            let compact_title: String = sum.title.chars().take(4).collect();
            line.child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .font_family(theme::MONO_FONT)
                    .text_color(score_color)
                    .child(sum.score.to_string()),
            )
            .child(div().text_color(theme::text_muted()).child("·"))
            .child(div().text_color(title_color).child(compact_title))
        }
        SummonerRatingStatus::Private => {
            line.child(div().text_color(theme::text_muted()).child("隐藏"))
        }
        SummonerRatingStatus::InsufficientSamples => {
            line.child(div().text_color(theme::text_muted()).child("样本少"))
        }
        SummonerRatingStatus::Loading => {
            line.child(div().text_color(theme::text_muted()).child("战绩…"))
        }
    }
}

/// Optional edge accent bar attached inside `hud_container`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AccentBar {
    None,
    Top(f32, Rgba),
    Bottom(f32, Rgba),
    Left(f32, Rgba),
    Right(f32, Rgba),
}

/// Standardized translucent Hextech HUD container with optional border and edge accent bar.
pub fn hud_container(bg: impl Into<Background>, border: Option<Rgba>, accent: AccentBar) -> Div {
    let mut container = div().relative().bg(bg.into());
    let is_bottom = matches!(accent, AccentBar::Bottom(_, _));

    if let Some(border_color) = border {
        container = if is_bottom {
            container
                .border_l_1()
                .border_r_1()
                .border_t_1()
                .border_color(border_color)
        } else {
            container.border_1().border_color(border_color)
        };
    }

    match accent {
        AccentBar::None => container,
        AccentBar::Top(thickness, color) => container.child(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .h(px(thickness))
                .bg(color),
        ),
        AccentBar::Bottom(thickness, color) => container.child(
            div()
                .absolute()
                .bottom(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .h(px(thickness))
                .bg(color),
        ),
        AccentBar::Left(thickness, color) => container.child(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .bottom(px(0.0))
                .w(px(thickness))
                .bg(color),
        ),
        AccentBar::Right(thickness, color) => container.child(
            div()
                .absolute()
                .top(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .w(px(thickness))
                .bg(color),
        ),
    }
}
