//! In-game overlay views: compact `324x204` corner HUD and non-blocking 3-column S+~D augment strip.

use std::path::PathBuf;

use gpui::{div, img, prelude::*, px, rgba, Div, FontWeight, ObjectFit};
use hex_data::{AugmentEvaluation, BuildItemInfo, DynamicBuildPlan};

use crate::app::HexrsApp;
use crate::theme::{self, tier_badge};

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

fn render_augment_offer_bar(app: &HexrsApp) -> Div {
    let mut columns = div()
        .size_full()
        .flex()
        .flex_row()
        .gap(px(app.augment_bar_gap_dip))
        .text_color(theme::text_primary());
    for eval in app.offered_augments.iter().take(3) {
        columns = columns.child(render_augment_slot_card(eval));
    }
    columns
}

fn render_augment_slot_card(eval: &AugmentEvaluation) -> Div {
    if eval.augment_id <= 0 {
        return div()
            .flex_1()
            .h_full()
            .p_2()
            .rounded_xl()
            .bg(theme::overlay_card_bg())
            .border_1()
            .border_color(theme::border_subtle())
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_muted())
                    .child("未匹配卡槽"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child("请按实际效果选择"),
            );
    }

    let has_synergy = eval.synergy_alert.is_some();

    div()
        .flex_1()
        .h_full()
        .p_2()
        .rounded_xl()
        .bg(theme::overlay_card_bg())
        .border_1()
        .border_color(if has_synergy {
            theme::synergy_gold_border()
        } else {
            theme::border_gold()
        })
        .flex()
        .flex_col()
        .justify_between()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(tier_badge(eval.tier))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme::text_primary())
                                .child(truncate_label(&eval.name_zh, 9)),
                        ),
                )
                .child(
                    div()
                        .px(px(5.0))
                        .py(px(1.0))
                        .rounded_md()
                        .bg(theme::card_bg())
                        .border_1()
                        .border_color(theme::border_subtle())
                        .text_xs()
                        .text_color(theme::gold())
                        .child(eval.rarity.clone()),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::cyan())
                        .child(format!("胜率 {:.1}%", eval.win_rate)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(format!("选取 {:.1}%", eval.pick_rate)),
                ),
        )
        .when_some(eval.synergy_alert.as_ref(), |card, alert| {
            card.child(
                div()
                    .px(px(5.0))
                    .py(px(2.0))
                    .rounded_md()
                    .bg(theme::synergy_gold_bg())
                    .border_1()
                    .border_color(theme::synergy_gold_border())
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::synergy_gold_text())
                    .child(format!(
                        "{} · 与【{}】Lift {:.2}",
                        alert.badge, alert.partner_name_zh, alert.lift_score
                    )),
            )
        })
        .when(eval.synergy_alert.is_none(), |card| {
            let clean_desc = sanitize_augment_desc(&eval.desc_zh);
            if clean_desc.is_empty() {
                card
            } else {
                card.child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(truncate_label(&clean_desc, 22)),
                )
            }
        })
}

fn sanitize_augment_desc(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();

    while let Some(ch) = chars.next() {
        if ch == '<' {
            let mut tag = String::new();
            for next in chars.by_ref() {
                if next == '>' {
                    break;
                }
                tag.push(next);
            }
            let lower = tag.trim().to_ascii_lowercase();
            if lower.starts_with("br") {
                out.push(' ');
            }
        } else if ch == '@' {
            for next in chars.by_ref() {
                if next == '@' {
                    break;
                }
            }
        } else {
            out.push(ch);
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
