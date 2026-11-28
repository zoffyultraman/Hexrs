//! Click-through, DirectComposition-safe 10-player loading screen HUD overlay (`hexrs_hud.md` Section 4.5).

use gpui::{div, prelude::*, px, Context, Div, FontWeight, Render};
use hex_core::{
    HeroTier, LoadingTeamSide, RatingTitlesConfig, SummonerRatingStatus, TeamRatingsSnapshot,
};

use crate::components::{self, AccentBar, TierBadgeSize};
use crate::lobby_badge_view::SummonerBadgeData;
use crate::theme;
use crate::win32::compute_loading_layout_dip;

/// Champion strength and short display name rendered on a loading screen card strip.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadingCardHeroData {
    pub tier: HeroTier,
    pub win_rate: f64,
    pub name_zh: String,
}

/// Combined champion meta and summoner rating data for one loading screen player card.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadingCardSlotData {
    pub hero: Option<LoadingCardHeroData>,
    pub summoner: SummonerBadgeData,
}

/// Aggregated team combat strength overview displayed on one side of the center VS bar.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TeamLoadingSummary {
    pub avg_score: Option<u8>,
    pub avg_tier_index: u8,
    pub premade_label: String,
    pub hot_streak_count: usize,
    pub loading_count: usize,
}

/// Immutable frame snapshot synced from `HexrsApp` to `LoadingScreenOverlay`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LoadingOverlayState {
    pub visible: bool,
    pub local_side: LoadingTeamSide,
    pub team_one: Vec<LoadingCardSlotData>,
    pub team_two: Vec<LoadingCardSlotData>,
    pub team_one_summary: TeamLoadingSummary,
    pub team_two_summary: TeamLoadingSummary,
    pub window_width_dip: f32,
    pub window_height_dip: f32,
}

/// Aggregates average ready rating, premade composition label, hot streaks, and pending loads.
pub fn compute_team_loading_summary(
    ratings: &TeamRatingsSnapshot,
    titles: &RatingTitlesConfig,
) -> TeamLoadingSummary {
    let mut score_sum: u32 = 0;
    let mut ready_count: u32 = 0;
    let mut hot_streak_count: usize = 0;
    let mut loading_count: usize = 0;

    for slot in &ratings.slots {
        if slot.status == SummonerRatingStatus::Ready {
            score_sum += u32::from(slot.score);
            ready_count += 1;
        } else if slot.status == SummonerRatingStatus::Loading {
            loading_count += 1;
        }
        if slot.streak >= 3 {
            hot_streak_count += 1;
        }
    }

    let (avg_score, avg_tier_index) = if ready_count > 0 {
        let avg = (score_sum as f64 / ready_count as f64)
            .round()
            .clamp(0.0, 100.0) as u8;
        let (tier_idx, _) = titles.resolve(SummonerRatingStatus::Ready, avg);
        (Some(avg), tier_idx)
    } else {
        (None, 3)
    };

    let group_sizes: Vec<usize> = ratings
        .premade_groups
        .iter()
        .map(|g| g.slot_indices.len())
        .filter(|&n| n >= 2)
        .collect();
    let premade_label = match group_sizes.as_slice() {
        [] => "单排".to_owned(),
        [single] => format!("{single}人黑"),
        many => {
            let joined = many
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join("+");
            format!("{joined}黑")
        }
    };

    TeamLoadingSummary {
        avg_score,
        avg_tier_index,
        premade_label,
        hot_streak_count,
        loading_count,
    }
}

/// Full-window click-through overlay anchoring 10 player strips + center VS bar over the loading screen.
pub struct LoadingScreenOverlay {
    state: LoadingOverlayState,
}

impl LoadingScreenOverlay {
    /// Creates an initially hidden 10-player loading screen overlay.
    pub fn new() -> Self {
        Self {
            state: LoadingOverlayState::default(),
        }
    }

    /// Replaces `state` and notifies GPUI only when geometry, visibility, or slot values changed.
    pub fn update_state(&mut self, next: LoadingOverlayState, cx: &mut Context<Self>) {
        if self.state != next {
            self.state = next;
            cx.notify();
        }
    }
}

fn render_loading_card_strip(
    slot: &LoadingCardSlotData,
    left_dip: f32,
    top_dip: f32,
    width_dip: f32,
    height_dip: f32,
) -> Div {
    let accent_color = slot
        .hero
        .as_ref()
        .map(|h| theme::tier_palette(h.tier).text)
        .unwrap_or_else(theme::border_subtle);

    let line1_left = match slot.hero.as_ref() {
        Some(hero) => div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.0))
            .overflow_hidden()
            .child(components::tier_badge(hero.tier, TierBadgeSize::Md))
            .child(
                div()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::BOLD)
                    .font_family(theme::MONO_FONT)
                    .text_color(theme::win_rate_color(hero.win_rate))
                    .child(format!("{:.1}%", hero.win_rate)),
            ),
        None => div()
            .text_size(px(11.0))
            .text_color(theme::text_muted())
            .child("未识别"),
    };

    let short_name: String = slot
        .hero
        .as_ref()
        .map(|h| h.name_zh.chars().take(5).collect())
        .unwrap_or_default();

    let sum = &slot.summoner;
    let has_streak = sum.streak >= 3 || sum.streak <= -3;
    let has_all_three = sum.premade_group_id.is_some() && sum.is_premade_last_mvp && has_streak;

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

    components::hud_container(
        theme::bg_surface(),
        Some(theme::border_subtle()),
        AccentBar::Top(2.0, accent_color),
    )
    .absolute()
    .left(px(left_dip))
    .top(px(top_dip))
    .w(px(width_dip))
    .h(px(height_dip))
    .rounded(px(3.0))
    .overflow_hidden()
    .child(
        div()
            .size_full()
            .px(px(7.0))
            .pt(px(4.0))
            .pb(px(3.0))
            .flex()
            .flex_col()
            .justify_between()
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(4.0))
                    .overflow_hidden()
                    .child(line1_left)
                    .child(components::player_score_line(sum)),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(4.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::text_muted())
                            .child(short_name),
                    )
                    .child(tags),
            ),
    )
}

fn render_team_summary_side(
    summary: &TeamLoadingSummary,
    side_label: &str,
    is_enemy: bool,
) -> Div {
    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .text_size(px(11.0))
        .overflow_hidden()
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .text_color(theme::text_secondary())
                .child(side_label.to_owned()),
        );

    if is_enemy && summary.avg_score.is_none() && summary.loading_count > 0 {
        return row.child(
            div()
                .font_family(theme::MONO_FONT)
                .text_color(theme::text_muted())
                .child(format!("拉取敌方中({})…", summary.loading_count)),
        );
    }

    match summary.avg_score {
        Some(avg) => {
            let (score_color, _) = theme::player_score_colors(summary.avg_tier_index);
            row = row.child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .font_family(theme::MONO_FONT)
                    .text_color(score_color)
                    .child(format!("均分 {avg}")),
            );
        }
        None => {
            row = row.child(div().text_color(theme::text_muted()).child("统计中"));
        }
    }

    row = row
        .child(div().text_color(theme::text_muted()).child("·"))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::text_secondary())
                .child(summary.premade_label.clone()),
        );

    if summary.hot_streak_count > 0 {
        row = row
            .child(div().text_color(theme::text_muted()).child("·"))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .font_family(theme::MONO_FONT)
                    .text_color(theme::win_streak_pill_palette().text)
                    .child(format!("{}人连胜", summary.hot_streak_count)),
            );
    }

    row
}

impl Render for LoadingScreenOverlay {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if !self.state.visible
            || self.state.window_width_dip <= 100.0
            || self.state.window_height_dip <= 100.0
            || (self.state.team_one.is_empty() && self.state.team_two.is_empty())
        {
            return div().size_full().bg(theme::transparent());
        }

        let layout =
            compute_loading_layout_dip(self.state.window_width_dip, self.state.window_height_dip);
        let mut canvas = div().size_full().relative().bg(theme::transparent());

        for (idx, slot) in self.state.team_one.iter().take(5).enumerate() {
            let left = layout.row_left_dip + idx as f32 * (layout.card_w_dip + layout.card_gap_dip);
            canvas = canvas.child(render_loading_card_strip(
                slot,
                left,
                layout.top_row_y_dip,
                layout.card_w_dip,
                layout.card_strip_h_dip,
            ));
        }

        for (idx, slot) in self.state.team_two.iter().take(5).enumerate() {
            let left = layout.row_left_dip + idx as f32 * (layout.card_w_dip + layout.card_gap_dip);
            canvas = canvas.child(render_loading_card_strip(
                slot,
                left,
                layout.bottom_row_y_dip,
                layout.card_w_dip,
                layout.card_strip_h_dip,
            ));
        }

        let (ally_summary, ally_label, enemy_summary, enemy_label) = match self.state.local_side {
            LoadingTeamSide::Blue => (
                &self.state.team_one_summary,
                "我方(蓝)",
                &self.state.team_two_summary,
                "敌方(红)",
            ),
            LoadingTeamSide::Red => (
                &self.state.team_two_summary,
                "我方(红)",
                &self.state.team_one_summary,
                "敌方(蓝)",
            ),
        };

        let center_bar = components::hud_container(
            theme::bg_surface_dense(),
            Some(theme::border_subtle()),
            AccentBar::None,
        )
        .absolute()
        .left(px(layout.center_bar_left_dip))
        .top(px(layout.center_bar_y_dip))
        .w(px(layout.center_bar_w_dip))
        .h(px(layout.center_bar_h_dip))
        .rounded(px(3.0))
        .overflow_hidden()
        .child(
            div()
                .size_full()
                .px(px(12.0))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(render_team_summary_side(ally_summary, ally_label, false))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .text_color(theme::gold())
                        .child("VS"),
                )
                .child(render_team_summary_side(enemy_summary, enemy_label, true)),
        );

        canvas.child(center_bar)
    }
}

#[cfg(test)]
mod tests {
    use super::compute_team_loading_summary;
    use hex_core::{
        PremadeGroupSummary, RatingTitlesConfig, SummonerRatingStatus, SummonerRatingSummary,
        TeamRatingsSnapshot,
    };

    fn make_slot(
        cell_id: i32,
        status: SummonerRatingStatus,
        score: u8,
        streak: i8,
    ) -> SummonerRatingSummary {
        SummonerRatingSummary {
            cell_id,
            status,
            score,
            tier_index: 2,
            title: "人上人".to_owned(),
            games_count: 10,
            win_rate: 52.0,
            avg_kda: 3.1,
            streak,
            premade_group_id: None,
            is_premade_last_mvp: false,
        }
    }

    #[test]
    fn computes_team_summary_avg_premade_streaks_and_loading_counts() {
        let titles = RatingTitlesConfig::default();

        // 2+3 premade with 4 ready + 1 private + 2 hot streaks
        let snap_2_3 = TeamRatingsSnapshot {
            slots: vec![
                make_slot(0, SummonerRatingStatus::Ready, 84, 5),
                make_slot(1, SummonerRatingStatus::Ready, 72, 3),
                make_slot(2, SummonerRatingStatus::Ready, 66, -3),
                make_slot(3, SummonerRatingStatus::Ready, 70, 0),
                make_slot(4, SummonerRatingStatus::Private, 0, 0),
            ],
            premade_groups: vec![
                PremadeGroupSummary {
                    group_id: 1,
                    slot_indices: vec![0, 1],
                    shared_games: 4,
                    last_mvp_slot_index: Some(0),
                },
                PremadeGroupSummary {
                    group_id: 2,
                    slot_indices: vec![2, 3, 4],
                    shared_games: 6,
                    last_mvp_slot_index: Some(3),
                },
            ],
        };
        let s1 = compute_team_loading_summary(&snap_2_3, &titles);
        assert_eq!(s1.avg_score, Some(73));
        assert_eq!(s1.avg_tier_index, 1);
        assert_eq!(s1.premade_label, "2+3黑");
        assert_eq!(s1.hot_streak_count, 2);
        assert_eq!(s1.loading_count, 0);

        // Single duo group
        let snap_duo = TeamRatingsSnapshot {
            slots: vec![
                make_slot(0, SummonerRatingStatus::Ready, 60, 0),
                make_slot(1, SummonerRatingStatus::Loading, 0, 0),
                make_slot(2, SummonerRatingStatus::Loading, 0, 0),
            ],
            premade_groups: vec![PremadeGroupSummary {
                group_id: 1,
                slot_indices: vec![0, 1],
                shared_games: 2,
                last_mvp_slot_index: None,
            }],
        };
        let s2 = compute_team_loading_summary(&snap_duo, &titles);
        assert_eq!(s2.avg_score, Some(60));
        assert_eq!(s2.avg_tier_index, 2);
        assert_eq!(s2.premade_label, "2人黑");
        assert_eq!(s2.hot_streak_count, 0);
        assert_eq!(s2.loading_count, 2);

        // All loading solo queue
        let snap_loading = TeamRatingsSnapshot {
            slots: vec![
                make_slot(0, SummonerRatingStatus::Loading, 0, 0),
                make_slot(1, SummonerRatingStatus::Loading, 0, 0),
                make_slot(2, SummonerRatingStatus::Loading, 0, 0),
            ],
            premade_groups: Vec::new(),
        };
        let s3 = compute_team_loading_summary(&snap_loading, &titles);
        assert_eq!(s3.avg_score, None);
        assert_eq!(s3.premade_label, "单排");
        assert_eq!(s3.loading_count, 3);
    }
}
