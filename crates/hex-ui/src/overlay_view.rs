//! Compact click-through corner HUD for in-game mode (`304x126` DIPs, hero portrait + 3 core items).

use std::path::PathBuf;

use gpui::{div, img, prelude::*, px, rgba, Div, FontWeight, ObjectFit};
use hex_data::{BuildItemInfo, DynamicBuildPlan};

use crate::app::HexOverlayApp;
use crate::theme;

/// Renders the compact `304x126` corner card showing ONLY hero portrait, hero name, and 3 recommended items.
pub fn render_in_game_overlay(app: &HexOverlayApp) -> Div {
    if !app.overlay_visible {
        return div().size_full().bg(rgba(0x00000000));
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
        .child(render_item_row(app, app.dynamic_build.as_ref()))
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

    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .flex()
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
        .when_some(playstyle_tag, |row, tag| {
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

fn render_item_row(app: &HexOverlayApp, build: Option<&DynamicBuildPlan>) -> Div {
    let Some(plan) = build else {
        return div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(theme::text_muted())
            .child("正在加载推荐出装...");
    };

    let mut row = div().flex().gap_1();
    for item in plan.items.iter().take(3) {
        let icon_path = app.item_icons.get(&item.id).cloned();
        row = row.child(render_item_slot(item, icon_path));
    }
    row
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
            .w(px(28.0))
            .h(px(28.0))
            .rounded_md()
            .overflow_hidden()
            .border_1()
            .border_color(border)
            .child(
                img(path)
                    .w(px(28.0))
                    .h(px(28.0))
                    .rounded_md()
                    .object_fit(ObjectFit::Cover),
            ),
        None => div()
            .w(px(28.0))
            .h(px(28.0))
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
        .flex_1()
        .p(px(4.0))
        .rounded_lg()
        .bg(bg)
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(3.0))
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
