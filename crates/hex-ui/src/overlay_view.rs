//! In-game transparent overlay rendering: floating augment badges, synergy alerts, and dynamic build strip.

use gpui::{div, prelude::*, px, rgba, Context, Div, FontWeight, Window};
use hex_core::{LiveGameState, RoiManifest};
use hex_data::{AugmentEvaluation, DynamicBuildPlan};

use crate::app::HexOverlayApp;
use crate::theme::{self, tier_badge};

const AUGMENT_BADGE_WIDTH_PX: f32 = 228.0;
const AUGMENT_BADGE_HEIGHT_OFFSET_PX: f32 = 148.0;

/// Renders the transparent in-game overlay anchored across `League of Legends.exe`.
pub fn render_in_game_overlay(
    app: &HexOverlayApp,
    window: &mut Window,
    cx: &mut Context<HexOverlayApp>,
) -> Div {
    if !app.overlay_visible {
        return div().size_full().bg(rgba(0x00000000));
    }

    let viewport = window.viewport_size();
    let vp_w = f32::from(viewport.width).max(1280.0);
    let vp_h = f32::from(viewport.height).max(720.0);

    let mut root = div()
        .size_full()
        .relative()
        .bg(rgba(0x00000000))
        .text_color(theme::text_primary())
        .child(render_hotkey_pill(app.hotkey_interactive));

    if !app.offered_augments.is_empty() {
        let manifest = app
            .roi_manifest
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| RoiManifest::default());
        let preset = manifest.preset_for_resolution(vp_w.round() as u32, vp_h.round() as u32);

        for (eval, roi) in app.offered_augments.iter().zip(preset.card_icons.iter()) {
            let center_x = (roi.x + roi.w * 0.5) * vp_w;
            let left_px =
                (center_x - AUGMENT_BADGE_WIDTH_PX * 0.5).clamp(12.0, (vp_w - AUGMENT_BADGE_WIDTH_PX - 12.0).max(12.0));
            let top_px = (roi.y * vp_h - AUGMENT_BADGE_HEIGHT_OFFSET_PX).clamp(52.0, (vp_h - 180.0).max(52.0));

            root = root.child(render_augment_floating_badge(eval, left_px, top_px));
        }

        if app.hotkey_interactive {
            root = root.child(render_offer_dismiss_button(app.last_offer_capture_ms, cx));
        }
    }

    if let Some(live) = app.live_game.as_ref() {
        root = root.child(render_dynamic_build_strip(
            live,
            app.active_hero_name_zh.as_deref(),
            app.dynamic_build.as_ref(),
        ));
    }

    root
}

fn render_hotkey_pill(interactive: bool) -> Div {
    let (bg, border, text_color, label) = if interactive {
        (
            theme::cyan_dim(),
            theme::border_cyan(),
            theme::cyan(),
            "🔓 交互模式已解锁 · 松开 Alt 恢复穿透",
        )
    } else {
        (
            theme::overlay_card_bg(),
            theme::border_subtle(),
            theme::text_muted(),
            "按住 Alt 解锁拖拽/详情 · 默认穿透",
        )
    };

    div()
        .absolute()
        .top(px(12.0))
        .left(px(0.0))
        .right(px(0.0))
        .flex()
        .justify_center()
        .child(
            div()
                .px_3()
                .py(px(4.0))
                .rounded_full()
                .bg(bg)
                .border_1()
                .border_color(border)
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_color)
                .child(label),
        )
}

fn render_augment_floating_badge(eval: &AugmentEvaluation, left_px: f32, top_px: f32) -> Div {
    let has_synergy = eval.synergy_alert.is_some();

    div()
        .absolute()
        .left(px(left_px))
        .top(px(top_px))
        .w(px(AUGMENT_BADGE_WIDTH_PX))
        .p_2()
        .rounded_xl()
        .bg(theme::overlay_card_bg())
        .border_1()
        .border_color(if has_synergy {
            theme::synergy_gold_border()
        } else {
            theme::border_gold()
        })
        .shadow_lg()
        .flex()
        .flex_col()
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
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme::text_primary())
                                .child(eval.name_zh.clone()),
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
                    .px_2()
                    .py(px(3.0))
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
        .child(
            div()
                .text_xs()
                .text_color(theme::text_muted())
                .child(truncate_desc(&eval.desc_zh, 34)),
        )
}

fn render_offer_dismiss_button(capture_ms: Option<u64>, cx: &mut Context<HexOverlayApp>) -> Div {
    let latency_note = capture_ms
        .map(|ms| format!("识别 {ms}ms · 收起面板 [×]"))
        .unwrap_or_else(|| "收起面板 [×]".to_owned());

    div()
        .absolute()
        .top(px(44.0))
        .left(px(0.0))
        .right(px(0.0))
        .flex()
        .justify_center()
        .child(
            div()
                .id("dismiss-augment-offer")
                .cursor_pointer()
                .px_3()
                .py(px(3.0))
                .rounded_md()
                .bg(theme::card_elevated_bg())
                .border_1()
                .border_color(theme::border_cyan())
                .hover(|s| s.bg(theme::cyan_dim()))
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::cyan())
                .child(latency_note)
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.dismiss_augment_offer(cx);
                })),
        )
}

fn render_dynamic_build_strip(
    live: &LiveGameState,
    hero_name_zh: Option<&str>,
    build: Option<&DynamicBuildPlan>,
) -> Div {
    let hero_label = hero_name_zh
        .filter(|s| !s.is_empty())
        .unwrap_or(live.active_champion_name.as_str())
        .to_owned();

    div()
        .absolute()
        .right(px(18.0))
        .bottom(px(22.0))
        .w(px(320.0))
        .p_2()
        .rounded_xl()
        .bg(theme::overlay_card_bg())
        .border_1()
        .border_color(theme::border_gold())
        .shadow_lg()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme::text_primary())
                                .child(hero_label),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child(format!("Lv.{}", live.level)),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::gold())
                        .child(format!("{:.0}G", live.current_gold)),
                ),
        )
        .when_some(build, |strip, plan| {
            let mut items_row = div().flex().flex_wrap().gap_1();
            for item in &plan.items {
                let (bg, border, text_color, prefix) = if item.already_owned {
                    (
                        theme::owned_item_bg(),
                        theme::owned_item_border(),
                        theme::cyan(),
                        "✓ ",
                    )
                } else {
                    (
                        theme::card_bg(),
                        theme::border_subtle(),
                        theme::text_primary(),
                        "• ",
                    )
                };

                items_row = items_row.child(
                    div()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded_md()
                        .bg(bg)
                        .border_1()
                        .border_color(border)
                        .text_xs()
                        .font_weight(if item.already_owned {
                            FontWeight::BOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(text_color)
                        .child(format!("{prefix}{}", item.name_zh)),
                );
            }

            strip
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::cyan())
                                .child(format!("推荐流派: {}", plan.playstyle_tag)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::gold())
                                .child(format!("胜率 {:.1}%", plan.win_rate)),
                        ),
                )
                .child(items_row)
        })
}

fn truncate_desc(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let collected: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{collected}…")
    } else {
        collected
    }
}
