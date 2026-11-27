//! In-game overlay views: compact `324x204` corner HUD and non-blocking 3-column S+~D augment strip.

use std::path::PathBuf;

use gpui::{
    div, img, linear_color_stop, linear_gradient, prelude::*, px, rgb, rgba, Div, FontWeight,
    ObjectFit, Rgba,
};
use hex_core::HeroTier;
use hex_data::{AugmentEvaluation, BuildItemInfo, DynamicBuildPlan};

use crate::app::HexrsApp;
use crate::theme;
use crate::win32::{AUGMENT_BAR_CARD_HEIGHT_DIP, AUGMENT_BAR_TOP_PAD_DIP};

/// Renders either the 3-card augment strip (when an offer is active) or the compact corner equipment HUD.
pub fn render_in_game_overlay(app: &HexrsApp) -> Div {
    if !app.overlay_visible {
        return div().size_full().bg(rgba(0x00000000));
    }

    if !app.offered_augments.is_empty() {
        return render_augment_offer_bar(app);
    }

    let hero_name = app
        .active_hero_name_zh
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            app.live_game
                .as_ref()
                .map(|l| l.active_champion_name.as_str())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("对局识别中...")
        .to_owned();

    let playstyle_tag = app
        .dynamic_build
        .as_ref()
        .map(|plan| plan.playstyle_tag.clone())
        .filter(|tag| !tag.is_empty());

    div()
        .size_full()
        .p_2()
        .rounded_xl()
        .bg(theme::overlay_card_bg())
        .border_1()
        .border_color(theme::border_gold())
        .flex()
        .flex_col()
        .justify_between()
        .gap_1()
        .text_color(theme::text_primary())
        .child(render_hero_header(
            app.active_hero_icon.clone(),
            hero_name,
            playstyle_tag,
        ))
        .child(render_item_grid(app, app.dynamic_build.as_ref()))
}

fn pick_recommended_augment_index(evals: &[AugmentEvaluation]) -> Option<usize> {
    let (best_idx, best_eval) = evals
        .iter()
        .take(4)
        .enumerate()
        .filter(|(_, eval)| eval.augment_id > 0 && !eval.low_sample)
        .min_by(|(idx_a, a), (idx_b, b)| {
            a.tier
                .sort_rank()
                .cmp(&b.tier.sort_rank())
                .then_with(|| b.win_rate.total_cmp(&a.win_rate))
                .then_with(|| b.pick_rate.total_cmp(&a.pick_rate))
                .then_with(|| idx_a.cmp(idx_b))
        })?;

    if best_eval.win_rate < 48.5 || matches!(best_eval.tier, HeroTier::C | HeroTier::D) {
        return None;
    }

    Some(best_idx)
}

fn augment_tier_badge_colors(tier: HeroTier) -> (Rgba, Rgba, Rgba) {
    match tier {
        HeroTier::SPlus => (rgb(0xFDE047), rgba(0xEAB30838), rgba(0xFACC15A6)),
        HeroTier::S => (rgb(0x2DD4BF), rgba(0x2DD4BF2E), rgba(0x2DD4BF8C)),
        HeroTier::A | HeroTier::B => (rgb(0x4ADE80), rgba(0x4ADE8024), rgba(0x4ADE8066)),
        HeroTier::C | HeroTier::D => (rgb(0x94A3B8), rgba(0x94A3B81F), rgba(0x94A3B84D)),
    }
}

fn augment_win_rate_colors(win_rate: f64) -> (Rgba, Rgba) {
    if win_rate >= 52.0 {
        (rgb(0x2DD4BF), rgba(0x2DD4BFBF))
    } else if win_rate >= 50.0 {
        (rgb(0xF1F5F9), rgba(0xF1F5F9BF))
    } else if win_rate >= 48.0 {
        (rgb(0x94A3B8), rgba(0x94A3B8BF))
    } else {
        (rgb(0xF87171), rgba(0xF87171BF))
    }
}

fn render_augment_offer_bar(app: &HexrsApp) -> Div {
    let recommended_idx = pick_recommended_augment_index(&app.offered_augments);
    let mut columns = div()
        .size_full()
        .pt(px(AUGMENT_BAR_TOP_PAD_DIP))
        .pb(px(2.0))
        .bg(rgba(0x00000000))
        .flex()
        .flex_row()
        .gap(px(app.augment_bar_gap_dip))
        .text_color(theme::text_primary());
    for (idx, eval) in app.offered_augments.iter().take(4).enumerate() {
        columns = columns.child(render_augment_slot_card(eval, recommended_idx == Some(idx)));
    }
    columns
}

fn render_augment_slot_card(eval: &AugmentEvaluation, is_recommended: bool) -> Div {
    if eval.augment_id <= 0 {
        return div()
            .flex_1()
            .min_w_0()
            .h(px(AUGMENT_BAR_CARD_HEIGHT_DIP))
            .relative()
            .child(
                div()
                    .size_full()
                    .overflow_hidden()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(4.0))
                    .bg(rgba(0x0B121EDB))
                    .border_1()
                    .border_color(rgba(0x47556973))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(0x94A3B8))
                            .child("海克斯未识别 · 请按效果选择"),
                    ),
            );
    }

    if eval.low_sample {
        return div()
            .flex_1()
            .min_w_0()
            .h(px(AUGMENT_BAR_CARD_HEIGHT_DIP))
            .relative()
            .child(
                div()
                    .size_full()
                    .overflow_hidden()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(4.0))
                    .bg(rgba(0x1C1014E0))
                    .border_1()
                    .border_color(rgba(0xEF444459))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(13.5))
                            .text_color(rgb(0xF87171))
                            .child("⚠"),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(0xFCA5A5))
                            .child("数据过少 · 建议刷新"),
                    ),
            );
    }

    let (tier_text, tier_bg, tier_border) = augment_tier_badge_colors(eval.tier);
    let (wr_color, wr_suffix_color) = augment_win_rate_colors(eval.win_rate);

    let inner_base = div()
        .size_full()
        .overflow_hidden()
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(4.0))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(6.0));

    let inner = if is_recommended {
        let rec_border = if eval.tier == HeroTier::S {
            rgb(0x2DD4BF)
        } else if eval.tier == HeroTier::SPlus {
            rgb(0xEAB308)
        } else {
            rgb(0xF0C46C)
        };
        inner_base
            .bg(linear_gradient(
                180.0,
                linear_color_stop(rgba(0x242A36F0), 0.0),
                linear_color_stop(rgba(0x0D192BF0), 1.0),
            ))
            .border_2()
            .border_color(rec_border)
    } else {
        inner_base
            .bg(rgba(0x0B121EDB))
            .border_1()
            .border_color(rgba(0x47556973))
    }
    .child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .min_w_0()
            .child(
                div()
                    .min_w(px(30.0))
                    .h(px(30.0))
                    .px(px(4.0))
                    .rounded(px(3.0))
                    .bg(tier_bg)
                    .border_1()
                    .border_color(tier_border)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(18.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(tier_text)
                    .child(eval.tier.label()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_size(px(17.5))
                            .font_weight(FontWeight::BOLD)
                            .font_family("Consolas")
                            .text_color(wr_color)
                            .child(format!("{:.1}%", eval.win_rate)),
                    )
                    .child(
                        div()
                            .pb(px(1.5))
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(wr_suffix_color)
                            .child("胜率"),
                    ),
            ),
    )
    .child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(3.0))
            .child(
                div()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(0x94A3B8))
                    .child("选取"),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .font_family("Consolas")
                    .text_color(rgb(0x94A3B8))
                    .child(format!("{:.1}%", eval.pick_rate)),
            ),
    );

    div()
        .flex_1()
        .min_w_0()
        .h(px(AUGMENT_BAR_CARD_HEIGHT_DIP))
        .relative()
        .child(inner)
        .when(is_recommended, |slot| {
            slot.child(
                div()
                    .absolute()
                    .top(px(-9.0))
                    .right(px(8.0))
                    .h(px(18.0))
                    .px(px(7.0))
                    .rounded_full()
                    .bg(linear_gradient(
                        90.0,
                        linear_color_stop(rgb(0xF0C46C), 0.0),
                        linear_color_stop(rgb(0xD97706), 1.0),
                    ))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0x0F172A))
                    .child("★ 推荐"),
            )
        })
}

#[allow(dead_code)]
fn sanitize_augment_desc(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;

    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("{{") {
            if let Some(end) = after.find("}}") {
                let inner = after[..end].trim();
                if inner.eq_ignore_ascii_case("Item_Keyword_OnHit") {
                    out.push_str("攻击特效");
                }
                rest = &after[end + 2..];
            } else {
                break;
            }
        } else if rest.starts_with("%i:") || rest.starts_with("%I:") {
            let after = &rest[3..];
            let token_end = after.find(|c: char| c == '%' || c.is_whitespace());
            if let Some(idx) = token_end {
                if after.as_bytes()[idx] == b'%' {
                    rest = &after[idx + 1..];
                    continue;
                }
            }
            out.push('%');
            rest = &rest[1..];
        } else if let Some(after) = rest.strip_prefix('<') {
            if let Some(end) = after.find('>') {
                let tag = after[..end].trim();
                if tag
                    .as_bytes()
                    .get(..2)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"br"))
                {
                    out.push(' ');
                }
                rest = &after[end + 1..];
            } else {
                break;
            }
        } else if let Some(after) = rest.strip_prefix('@') {
            if let Some(end) = after.find('@') {
                rest = &after[end + 1..];
            } else {
                break;
            }
        } else if let Some(ch) = rest.chars().next() {
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
        } else {
            break;
        }
    }

    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn render_hero_header(
    hero_icon: Option<PathBuf>,
    hero_name: String,
    playstyle_tag: Option<String>,
) -> Div {
    let portrait = match hero_icon {
        Some(path) => div()
            .w(px(36.0))
            .h(px(36.0))
            .rounded_lg()
            .overflow_hidden()
            .border_1()
            .border_color(theme::border_gold())
            .child(
                img(path)
                    .w(px(36.0))
                    .h(px(36.0))
                    .rounded_lg()
                    .object_fit(ObjectFit::Cover),
            ),
        None => div()
            .w(px(36.0))
            .h(px(36.0))
            .rounded_lg()
            .bg(theme::gold_dim())
            .border_1()
            .border_color(theme::border_gold())
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .font_weight(FontWeight::BOLD)
            .text_color(theme::gold())
            .child("HEX"),
    };

    let tag_badge = playstyle_tag.map(|tag| truncate_label(&tag, 7));

    div()
        .w_full()
        .h(px(38.0))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(portrait)
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::text_primary())
                        .child(hero_name),
                ),
        )
        .when_some(tag_badge, |row, tag| {
            row.child(
                div()
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded_md()
                    .bg(theme::cyan_dim())
                    .border_1()
                    .border_color(theme::border_cyan())
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::cyan())
                    .child(tag),
            )
        })
}

fn render_item_grid(app: &HexrsApp, build: Option<&DynamicBuildPlan>) -> Div {
    let Some(plan) = build else {
        return div()
            .w_full()
            .h(px(142.0))
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(theme::text_muted())
            .child("正在加载推荐出装...");
    };

    if plan.items.is_empty() {
        return div()
            .w_full()
            .h(px(142.0))
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(theme::text_muted())
            .child("暂无推荐装备");
    }

    let items: Vec<&BuildItemInfo> = plan.items.iter().take(6).collect();
    let mut grid = div()
        .w_full()
        .h(px(142.0))
        .flex()
        .flex_col()
        .justify_between()
        .gap(px(6.0));

    for chunk in items.chunks(3) {
        let mut row = div()
            .w_full()
            .h(px(68.0))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(6.0));
        for item in chunk {
            let icon_path = app.item_icons.get(&item.id).cloned();
            row = row.child(render_item_slot(item, icon_path));
        }
        for _ in chunk.len()..3 {
            row = row.child(div().w(px(96.0)).h(px(66.0)));
        }
        grid = grid.child(row);
    }

    grid
}

fn render_item_slot(item: &BuildItemInfo, icon_path: Option<PathBuf>) -> Div {
    let (bg, border, name_color) = if item.already_owned {
        (
            theme::owned_item_bg(),
            theme::owned_item_border(),
            theme::cyan(),
        )
    } else {
        (
            theme::card_bg(),
            theme::border_subtle(),
            theme::text_primary(),
        )
    };

    let icon = match icon_path {
        Some(path) => div()
            .w(px(30.0))
            .h(px(30.0))
            .rounded_md()
            .overflow_hidden()
            .border_1()
            .border_color(border)
            .child(
                img(path)
                    .w(px(30.0))
                    .h(px(30.0))
                    .rounded_md()
                    .object_fit(ObjectFit::Cover),
            ),
        None => div()
            .w(px(30.0))
            .h(px(30.0))
            .rounded_md()
            .bg(theme::card_elevated_bg())
            .border_1()
            .border_color(border)
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .font_weight(FontWeight::BOLD)
            .text_color(theme::gold())
            .child("装"),
    };

    div()
        .w(px(96.0))
        .h(px(66.0))
        .p(px(4.0))
        .rounded_lg()
        .bg(bg)
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(4.0))
        .child(icon)
        .child(
            div()
                .text_xs()
                .font_weight(if item.already_owned {
                    FontWeight::BOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(name_color)
                .child(truncate_label(&item.name_zh, 6)),
        )
}

fn truncate_label(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let collected: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{collected}…")
    } else {
        collected
    }
}

#[cfg(test)]
mod tests {
    use super::{pick_recommended_augment_index, sanitize_augment_desc};
    use hex_core::HeroTier;
    use hex_data::AugmentEvaluation;

    fn make_eval(
        augment_id: i32,
        tier: HeroTier,
        win_rate: f64,
        pick_rate: f64,
        low_sample: bool,
    ) -> AugmentEvaluation {
        AugmentEvaluation {
            augment_id,
            name_zh: format!("aug_{augment_id}"),
            rarity: "黄金".to_owned(),
            desc_zh: String::new(),
            tier,
            win_rate,
            pick_rate,
            sample_size: if low_sample { 12 } else { 200 },
            low_sample,
            synergy_alert: None,
        }
    }

    #[test]
    fn sanitizes_cdragon_mustache_inline_icons_and_html_tokens() {
        assert_eq!(
            sanitize_augment_desc("你的技能可施加{{ Item_Keyword_OnHit }}。"),
            "你的技能可施加攻击特效。"
        );
        assert_eq!(
            sanitize_augment_desc("{{Cherry_SpiritLink@TeamSize@_Summary}}"),
            ""
        );
        assert_eq!(
            sanitize_augment_desc(
                "获得<crit>%i:scaleCrit%@CritChance*100@%暴击几率</crit>。<br>50%有效"
            ),
            "获得%暴击几率。 50%有效"
        );
    }

    #[test]
    fn pick_recommended_augment_respects_tier_winrate_and_filters() {
        // 1. Picks highest tier (S > A > B), breaking same-tier ties by highest win_rate.
        let tier_and_tie = vec![
            make_eval(1, HeroTier::A, 56.0, 20.0, false),
            make_eval(2, HeroTier::S, 51.2, 12.0, false),
            make_eval(3, HeroTier::S, 54.0, 10.0, false),
        ];
        assert_eq!(pick_recommended_augment_index(&tier_and_tie), Some(2));

        // 2. Ignores low_sample = true and augment_id <= 0 even if their win_rate/tier is high.
        let filtered = vec![
            make_eval(0, HeroTier::SPlus, 60.0, 30.0, false),
            make_eval(10, HeroTier::SPlus, 59.0, 25.0, true),
            make_eval(11, HeroTier::A, 50.5, 14.0, false),
        ];
        assert_eq!(pick_recommended_augment_index(&filtered), Some(2));

        // 3. Returns None when best card has win_rate < 48.5 OR all valid are C/D OR all low_sample.
        let low_wr = vec![
            make_eval(20, HeroTier::B, 48.4, 15.0, false),
            make_eval(21, HeroTier::C, 53.0, 12.0, false),
        ];
        assert_eq!(pick_recommended_augment_index(&low_wr), None);

        let only_cd = vec![
            make_eval(30, HeroTier::C, 55.0, 18.0, false),
            make_eval(31, HeroTier::D, 52.0, 10.0, false),
        ];
        assert_eq!(pick_recommended_augment_index(&only_cd), None);

        let all_low_sample = vec![
            make_eval(40, HeroTier::SPlus, 58.0, 20.0, true),
            make_eval(41, HeroTier::S, 54.0, 15.0, true),
        ];
        assert_eq!(pick_recommended_augment_index(&all_low_sample), None);
    }
}
