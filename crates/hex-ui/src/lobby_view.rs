//! Champ Select GPUI sidebar rendering (`code_artifact.md` Section 3.1).

use gpui::{div, prelude::*, px, AnyElement, Context, Div, FontWeight, SharedString};
use hex_core::{AppMode, ChampSelectState, PremadeGroupSummary};
use hex_data::{
    AugmentEvaluation, BenchHighlight, DynamicBuildPlan, HeroPreview, HeroStats,
    SynergyRecommendation, TeamDiagnosis, TeamRecommendations,
};

use crate::app::{AppUpdateUiState, HexrsApp, APP_VERSION};
use crate::components::{self, TierBadgeSize};
use crate::theme;

const SPLIT_BAR_WIDTH_PX: f32 = 268.0;

/// Renders the interactive 320px-wide Champ Select sidebar docked beside `LeagueClientUx.exe`.
pub fn render_lobby_sidebar(app: &HexrsApp, cx: &mut Context<HexrsApp>) -> Div {
    let champ_select = app.champ_select.as_ref();
    let lcu_connected = app.lcu_connected;
    let current_hero = app.current_hero_stats.as_ref();
    let active_hero_id = app.active_champ_select_hero_id;
    let team_heroes = &app.team_hero_stats;
    let bench = &app.bench_highlights;
    let recs = &app.team_recs;
    let expanded_hero_id = app.expanded_hero_id;
    let selected_preview = app.selected_preview.as_ref();
    let is_preview = app.preview_mode == Some(AppMode::LobbyDocked);
    let preview_ratings = (is_preview && champ_select.is_none()).then(|| app.preview_team_ratings());
    let premade_groups: &[PremadeGroupSummary] = app
        .team_ratings
        .as_ref()
        .or(preview_ratings.as_ref())
        .map(|r| r.premade_groups.as_slice())
        .unwrap_or(&[]);

    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(theme::canvas_bg())
        .border_1()
        .border_color(theme::border_gold())
        .text_color(theme::text_primary())
        .when(is_preview, |root| {
            root.child(render_preview_banner(cx))
        })
        .child(render_header(
            lcu_connected,
            champ_select,
            &app.app_update_state,
            cx,
        ))
        .child(
            div()
                .id("lobby-sidebar-scroll")
                .flex_1()
                .overflow_y_scroll()
                .p_3()
                .flex()
                .flex_col()
                .gap_3()
                .child(render_active_hero_section(
                    current_hero,
                    active_hero_id,
                    expanded_hero_id,
                    selected_preview,
                    cx,
                ))
                .when(!premade_groups.is_empty(), |parent| {
                    parent.child(render_premade_summary_section(premade_groups))
                })
                .when(!team_heroes.is_empty(), |parent| {
                    parent.child(render_team_heroes_section(
                        team_heroes,
                        active_hero_id,
                        expanded_hero_id,
                        selected_preview,
                        cx,
                    ))
                })
                .when(!bench.is_empty(), |parent| {
                    parent.child(render_bench_section(
                        bench,
                        expanded_hero_id,
                        selected_preview,
                        cx,
                    ))
                })
                .child(render_diagnosis_section(&recs.diagnosis))
                .child(render_recommendations_section(
                    recs,
                    expanded_hero_id,
                    selected_preview,
                    cx,
                )),
        )
}

fn render_preview_banner(cx: &mut Context<HexrsApp>) -> Div {
    div()
        .px_3()
        .py_1()
        .bg(theme::cyan_dim())
        .border_b_1()
        .border_color(theme::border_cyan())
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme::cyan())
                .child("UI 预览模式 · 选人侧边栏"),
        )
        .child(
            div()
                .id("exit-lobby-preview")
                .cursor_pointer()
                .px_2()
                .py(px(1.0))
                .rounded(px(3.0))
                .bg(theme::card_bg())
                .border_1()
                .border_color(theme::border_cyan())
                .hover(|s| s.bg(theme::card_elevated_bg()))
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text_primary())
                .child("退出预览")
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.set_preview_mode(None, cx);
                })),
        )
}

fn render_header(
    lcu_connected: bool,
    champ_select: Option<&ChampSelectState>,
    app_update_state: &AppUpdateUiState,
    cx: &mut Context<HexrsApp>,
) -> Div {
    let status_dot_color = if lcu_connected {
        theme::cyan()
    } else {
        theme::warning_text()
    };
    let mode_label = format_game_mode(
        champ_select.map(|s| s.game_mode.as_str()).unwrap_or(""),
        champ_select.and_then(|s| s.queue_id),
    );
    let phase_label = champ_select
        .map(|s| format_phase(&s.phase))
        .unwrap_or("等待对局");
    let has_update = matches!(app_update_state, AppUpdateUiState::UpdateAvailable { .. });

    div()
        .px_3()
        .py_2()
        .bg(theme::card_bg())
        .border_b_1()
        .border_color(theme::border_subtle())
        .flex()
        .items_center()
        .justify_between()
        .gap_1()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded_full()
                        .bg(status_dot_color),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::gold())
                        .child("HEXRS"),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_size(px(10.0))
                        .font_family(theme::MONO_FONT)
                        .text_color(theme::text_muted())
                        .child(APP_VERSION),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .when(has_update, |row| {
                    row.child(
                        div()
                            .id("lobby-app-update-pill")
                            .cursor_pointer()
                            .px(px(5.0))
                            .py(px(1.0))
                            .rounded(px(3.0))
                            .bg(theme::gold_dim())
                            .border_1()
                            .border_color(theme::border_gold())
                            .hover(|s| s.bg(theme::bg_highlight()))
                            .text_size(px(10.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::gold())
                            .child("新版本 ↗")
                            .on_click(cx.listener(|this, _event, _window, _cx| {
                                this.open_app_release_page();
                            })),
                    )
                })
                .child(
                    div()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(2.0))
                        .bg(theme::cyan_dim())
                        .border_1()
                        .border_color(theme::border_cyan())
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::cyan())
                        .child(mode_label),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(phase_label),
                ),
        )
}

fn render_active_hero_section(
    current_hero: Option<&HeroStats>,
    active_hero_id: Option<i32>,
    expanded_hero_id: Option<i32>,
    selected_preview: Option<&HeroPreview>,
    cx: &mut Context<HexrsApp>,
) -> Div {
    let Some(hero) = current_hero else {
        if let Some(hero_id) = active_hero_id.filter(|&id| id > 0) {
            return div()
                .p_3()
                .rounded(px(4.0))
                .bg(theme::card_bg())
                .border_1()
                .border_color(theme::border_gold())
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
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme::text_primary())
                                .child(format!("英雄 #{hero_id}")),
                        )
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(2.0))
                                .bg(theme::gold_dim())
                                .border_1()
                                .border_color(theme::border_subtle())
                                .text_xs()
                                .text_color(theme::gold())
                                .child("冷启动库未收录"),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child("仍为您实时计算阵容短板并推荐高胜搭档，可点击托盘“检查数据热更新”同步全量英雄。"),
                );
        }

        return div()
            .p_3()
            .rounded(px(4.0))
            .bg(theme::card_bg())
            .border_1()
            .border_color(theme::border_subtle())
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::gold())
                    .child("当前英雄"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child("等待锁定或预选英雄以加载强度与出装建议..."),
            );
    };

    let hero_id = hero.hero_id;
    let is_expanded = expanded_hero_id == Some(hero_id);
    let preview_matches = selected_preview.filter(|p| p.hero_id == hero_id);

    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .id(SharedString::from(format!("active-hero-{hero_id}")))
                .cursor_pointer()
                .p_3()
                .rounded(px(4.0))
                .bg(if is_expanded {
                    theme::bg_highlight()
                } else {
                    theme::card_bg()
                })
                .border_1()
                .border_color(if is_expanded {
                    theme::border_cyan()
                } else {
                    theme::border_gold()
                })
                .hover(|s| s.bg(theme::card_elevated_bg()))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.toggle_hero_preview(hero_id, cx);
                }))
                .flex()
                .flex_col()
                .gap_2()
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
                                .child(components::tier_badge(hero.tier, TierBadgeSize::Md))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::BOLD)
                                        .child(format!("{} · {}", hero.name_zh, hero.title_zh)),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child(if is_expanded { "收起预览 ▲" } else { "战前预览 ▼" }),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .font_family(theme::MONO_FONT)
                                .text_color(theme::win_rate_color(hero.win_rate))
                                .child(format!("胜率 {:.1}%", hero.win_rate)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family(theme::MONO_FONT)
                                .text_color(theme::text_muted())
                                .child(format!("登场 {:.1}%", hero.pick_rate)),
                        ),
                )
                .when(!hero.tags.is_empty(), |card| {
                    card.child(render_tag_row(&hero.tags))
                }),
        )
        .when_some(preview_matches, |container, preview| {
            container.child(render_hero_preview_drawer(preview))
        })
}

fn render_premade_summary_section(groups: &[PremadeGroupSummary]) -> Div {
    let mut rows = div().flex().flex_col().gap_1();
    for group in groups {
        let pal = theme::premade_pill_palette(group.group_id);

        let members: Vec<String> = group
            .slot_indices
            .iter()
            .map(|&slot_idx| {
                let seat = slot_idx + 1;
                if group.last_mvp_slot_index == Some(slot_idx) {
                    format!("{seat}号位(MVP)")
                } else {
                    format!("{seat}号位")
                }
            })
            .collect();

        rows = rows.child(
            div()
                .px_2()
                .py(px(4.0))
                .rounded(px(3.0))
                .bg(pal.bg)
                .border_1()
                .border_color(pal.border)
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .min_w_0()
                        .overflow_hidden()
                        .child(components::premade_pill(group.group_id, false))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(pal.text)
                                .child(members.join(" + ")),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .when(group.last_mvp_slot_index.is_some(), |right| {
                            right.child(components::mvp_pill())
                        })
                        .child(
                            div()
                                .text_xs()
                                .font_family(theme::MONO_FONT)
                                .text_color(theme::text_secondary())
                                .child(format!("{}局", group.shared_games)),
                        ),
                ),
        );
    }

    div()
        .p_3()
        .rounded(px(4.0))
        .bg(theme::card_bg())
        .border_1()
        .border_color(theme::border_subtle())
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::gold())
                        .child("开黑车队识别"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child("近期同队 >= 2 局"),
                ),
        )
        .child(rows)
}

fn render_team_heroes_section(
    team_heroes: &[HeroStats],
    active_hero_id: Option<i32>,
    expanded_hero_id: Option<i32>,
    selected_preview: Option<&HeroPreview>,
    cx: &mut Context<HexrsApp>,
) -> Div {
    let mut pills = div().flex().flex_wrap().gap_1();
    for hero in team_heroes {
        let hero_id = hero.hero_id;
        let is_local = active_hero_id == Some(hero_id);
        let is_expanded = expanded_hero_id == Some(hero_id);
        pills = pills.child(
            div()
                .id(SharedString::from(format!("team-hero-{hero_id}")))
                .cursor_pointer()
                .px_2()
                .py(px(4.0))
                .rounded(px(3.0))
                .bg(if is_local || is_expanded {
                    theme::bg_highlight()
                } else {
                    theme::card_elevated_bg()
                })
                .border_1()
                .border_color(if is_expanded || is_local {
                    theme::border_cyan()
                } else {
                    theme::border_subtle()
                })
                .hover(|s| s.bg(theme::card_elevated_bg()))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.toggle_hero_preview(hero_id, cx);
                }))
                .flex()
                .items_center()
                .gap_1()
                .child(components::tier_badge(hero.tier, TierBadgeSize::Md))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(hero.name_zh.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .font_family(theme::MONO_FONT)
                        .text_color(theme::win_rate_color(hero.win_rate))
                        .child(format!("{:.1}%", hero.win_rate)),
                ),
        );
    }

    let preview_for_team = selected_preview.filter(|p| {
        expanded_hero_id == Some(p.hero_id)
            && active_hero_id != Some(p.hero_id)
            && team_heroes.iter().any(|h| h.hero_id == p.hero_id)
    });

    div()
        .p_3()
        .rounded(px(4.0))
        .bg(theme::card_bg())
        .border_1()
        .border_color(theme::border_subtle())
        .flex()
        .flex_col()
        .gap_2()
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
                        .child("我方阵容胜率"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child("点击切换预览"),
                ),
        )
        .child(pills)
        .when_some(preview_for_team, |container, preview| {
            container.child(render_hero_preview_drawer(preview))
        })
}

fn render_bench_section(
    bench: &[BenchHighlight],
    expanded_hero_id: Option<i32>,
    selected_preview: Option<&HeroPreview>,
    cx: &mut Context<HexrsApp>,
) -> Div {
    let mut pills = div().flex().flex_wrap().gap_1();
    for item in bench {
        let hero_id = item.hero_id;
        let is_expanded = expanded_hero_id == Some(hero_id);
        pills = pills.child(
            div()
                .id(SharedString::from(format!("bench-hero-{hero_id}")))
                .cursor_pointer()
                .px_2()
                .py(px(4.0))
                .rounded(px(3.0))
                .bg(if is_expanded {
                    theme::bg_highlight()
                } else {
                    theme::gold_dim()
                })
                .border_1()
                .border_color(if is_expanded {
                    theme::border_cyan()
                } else {
                    theme::border_gold()
                })
                .hover(|s| s.bg(theme::card_elevated_bg()))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.toggle_hero_preview(hero_id, cx);
                }))
                .flex()
                .items_center()
                .gap_1()
                .child(components::tier_badge(item.tier, TierBadgeSize::Md))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(item.name_zh.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .font_family(theme::MONO_FONT)
                        .text_color(theme::win_rate_color(item.win_rate))
                        .child(format!("{:.1}%", item.win_rate)),
                ),
        );
    }

    let preview_for_bench = selected_preview.filter(|p| {
        expanded_hero_id == Some(p.hero_id) && bench.iter().any(|b| b.hero_id == p.hero_id)
    });

    div()
        .p_3()
        .rounded(px(4.0))
        .bg(theme::card_bg())
        .border_1()
        .border_color(theme::border_subtle())
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::gold())
                        .child("备选池全部胜率"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child("按胜率排序"),
                ),
        )
        .child(pills)
        .when_some(preview_for_bench, |container, preview| {
            container.child(render_hero_preview_drawer(preview))
        })
}

fn render_diagnosis_section(diagnosis: &TeamDiagnosis) -> Div {
    let ad_pct = (diagnosis.ad_ratio * 100.0).clamp(0.0, 100.0);
    let ap_pct = (diagnosis.ap_ratio * 100.0).clamp(0.0, 100.0);
    let ad_px = ((SPLIT_BAR_WIDTH_PX * diagnosis.ad_ratio as f32).round()).clamp(2.0, SPLIT_BAR_WIDTH_PX - 2.0);
    let ap_px = (SPLIT_BAR_WIDTH_PX - ad_px).max(2.0);

    let mut warnings_row = div().flex().flex_wrap().gap_1();
    if diagnosis.warnings.is_empty() {
        warnings_row = warnings_row.child(
            div()
                .px(px(6.0))
                .py(px(2.0))
                .rounded(px(2.0))
                .bg(theme::cyan_dim())
                .border_1()
                .border_color(theme::border_cyan())
                .text_xs()
                .text_color(theme::cyan())
                .child("阵容结构均衡"),
        );
    } else {
        for warn in &diagnosis.warnings {
            warnings_row = warnings_row.child(
                div()
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(2.0))
                    .bg(theme::warning_bg())
                    .border_1()
                    .border_color(theme::warning_border())
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::warning_text())
                    .child(warn.clone()),
            );
        }
    }

    div()
        .p_3()
        .rounded(px(4.0))
        .bg(theme::card_bg())
        .border_1()
        .border_color(theme::border_subtle())
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::gold())
                        .child("阵容短板诊断"),
                )
                .child(
                    div()
                        .text_xs()
                        .font_family(theme::MONO_FONT)
                        .text_color(theme::text_muted())
                        .child(format!(
                            "前排 {:.1} · 硬控 {:.1}",
                            diagnosis.tank_total, diagnosis.cc_total
                        )),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .text_xs()
                        .font_family(theme::MONO_FONT)
                        .child(
                            div()
                                .text_color(theme::ad_bar())
                                .child(format!("物理 {:.0}%", ad_pct)),
                        )
                        .child(
                            div()
                                .text_color(theme::ap_bar())
                                .child(format!("魔法 {:.0}%", ap_pct)),
                        ),
                )
                .child(
                    div()
                        .w(px(SPLIT_BAR_WIDTH_PX))
                        .h(px(6.0))
                        .rounded(px(2.0))
                        .overflow_hidden()
                        .flex()
                        .child(div().w(px(ad_px)).h_full().bg(theme::ad_bar()))
                        .child(div().w(px(ap_px)).h_full().bg(theme::ap_bar())),
                ),
        )
        .child(warnings_row)
}

fn render_recommendations_section(
    recs: &TeamRecommendations,
    expanded_hero_id: Option<i32>,
    selected_preview: Option<&HeroPreview>,
    cx: &mut Context<HexrsApp>,
) -> Div {
    let mut list = div().flex().flex_col().gap_2();

    if recs.top_picks.is_empty() {
        list = list.child(
            div()
                .text_xs()
                .text_color(theme::text_muted())
                .child("暂无可选搭档推荐"),
        );
    } else {
        for rec in &recs.top_picks {
            list = list.child(render_recommendation_row(
                rec,
                expanded_hero_id,
                selected_preview,
                cx,
            ));
        }
    }

    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::gold())
                        .child("智能搭档推荐 TOP 5"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child("点击查看核心海克斯与六神装"),
                ),
        )
        .child(list)
}

fn render_recommendation_row(
    rec: &SynergyRecommendation,
    expanded_hero_id: Option<i32>,
    selected_preview: Option<&HeroPreview>,
    cx: &mut Context<HexrsApp>,
) -> AnyElement {
    let hero_id = rec.hero_id;
    let is_expanded = expanded_hero_id == Some(hero_id);
    let preview_matches = selected_preview.filter(|p| p.hero_id == hero_id);

    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .id(SharedString::from(format!("rec-hero-{hero_id}")))
                .cursor_pointer()
                .p_2()
                .rounded(px(4.0))
                .bg(if is_expanded {
                    theme::bg_highlight()
                } else {
                    theme::card_bg()
                })
                .border_1()
                .border_color(if is_expanded {
                    theme::border_cyan()
                } else {
                    theme::border_subtle()
                })
                .hover(|s| s.bg(theme::card_elevated_bg()))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.toggle_hero_preview(hero_id, cx);
                }))
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
                                .child(components::tier_badge(rec.tier, TierBadgeSize::Md))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .child(rec.name_zh.clone()),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .font_family(theme::MONO_FONT)
                                .text_color(theme::win_rate_color(rec.win_rate))
                                .child(format!("{:.1}% 胜率", rec.win_rate)),
                        ),
                )
                .when_some(rec.synergy_reason.as_ref(), |row, reason| {
                    row.child(
                        div()
                            .text_xs()
                            .text_color(theme::gold())
                            .child(reason.clone()),
                    )
                })
                .when_some(rec.comp_reason.as_ref(), |row, comp| {
                    row.child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .px(px(5.0))
                                    .py(px(1.0))
                                    .rounded(px(2.0))
                                    .bg(theme::cyan_dim())
                                    .border_1()
                                    .border_color(theme::border_cyan())
                                    .text_xs()
                                    .text_color(theme::cyan())
                                    .child(comp.clone()),
                            ),
                    )
                })
                .when(!rec.tags.is_empty(), |row| {
                    row.child(render_tag_row(&rec.tags))
                }),
        )
        .when_some(preview_matches, |container, preview| {
            container.child(render_hero_preview_drawer(preview))
        })
        .into_any_element()
}

fn render_hero_preview_drawer(preview: &HeroPreview) -> Div {
    let mut aug_list = div().flex().flex_col().gap_1();
    if preview.top_augments.is_empty() {
        aug_list = aug_list.child(
            div()
                .text_xs()
                .text_color(theme::text_muted())
                .child("暂无海克斯统计"),
        );
    } else {
        for aug in &preview.top_augments {
            aug_list = aug_list.child(render_preview_augment_row(aug));
        }
    }

    let mut build_list = div().flex().flex_col().gap_1();
    if preview.core_builds.is_empty() {
        build_list = build_list.child(
            div()
                .text_xs()
                .text_color(theme::text_muted())
                .child("暂无出装数据"),
        );
    } else {
        for build in preview.core_builds.iter().take(2) {
            build_list = build_list.child(render_preview_build_row(build));
        }
    }

    div()
        .p_2()
        .rounded(px(4.0))
        .bg(theme::bg_highlight())
        .border_1()
        .border_color(theme::border_cyan())
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme::cyan())
                .child(format!("{} · 核心海克斯 TOP 3", preview.name_zh)),
        )
        .child(aug_list)
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme::gold())
                .child("推荐六神装"),
        )
        .child(build_list)
}

fn render_preview_augment_row(aug: &AugmentEvaluation) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(components::tier_badge(aug.tier, TierBadgeSize::Md))
                .child(div().text_xs().child(aug.name_zh.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(aug.rarity.clone()),
                ),
        )
        .child(
            div()
                .text_xs()
                .font_family(theme::MONO_FONT)
                .text_color(theme::win_rate_color(aug.win_rate))
                .child(format!("{:.1}%", aug.win_rate)),
        )
}

fn render_preview_build_row(build: &DynamicBuildPlan) -> Div {
    let mut item_chips = div().flex().flex_wrap().gap_1();
    for item in &build.items {
        item_chips = item_chips.child(
            div()
                .px(px(5.0))
                .py(px(1.0))
                .rounded(px(2.0))
                .bg(theme::card_bg())
                .border_1()
                .border_color(theme::border_subtle())
                .text_xs()
                .child(item.name_zh.clone()),
        );
    }

    div()
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
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::gold())
                        .child(build.playstyle_tag.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .font_family(theme::MONO_FONT)
                        .text_color(theme::win_rate_color(build.win_rate))
                        .child(format!("{:.1}%", build.win_rate)),
                ),
        )
        .child(item_chips)
}

fn render_tag_row(tags: &[String]) -> Div {
    let mut row = div().flex().flex_wrap().gap_1();
    for tag in tags.iter().take(3) {
        row = row.child(
            div()
                .px(px(5.0))
                .py(px(1.0))
                .rounded(px(2.0))
                .bg(theme::canvas_bg())
                .border_1()
                .border_color(theme::border_subtle())
                .text_xs()
                .text_color(theme::text_muted())
                .child(tag.clone()),
        );
    }
    row
}

fn format_game_mode(raw: &str, queue_id: Option<i32>) -> &'static str {
    if matches!(queue_id, Some(1700)) || raw.eq_ignore_ascii_case("CHERRY") {
        "斗魂竞技场"
    } else if matches!(queue_id, Some(450) | Some(2400))
        || raw.eq_ignore_ascii_case("ARAM")
        || raw.eq_ignore_ascii_case("KIWI")
    {
        "海克斯大乱斗"
    } else if raw.is_empty() {
        "娱乐模式"
    } else {
        "选人阶段"
    }
}

fn format_phase(raw: &str) -> &'static str {
    match raw {
        "PLANNING" => "预选阶段",
        "BAN_PICK" => "禁用/选人",
        "FINALIZATION" => "最终备战",
        "GAME_STARTING" => "即将载入",
        _ => "选人中",
    }
}
