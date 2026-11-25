//! Root GPUI application entity, event-loop polling, and Win32 window state orchestration.

use crossbeam_channel::Receiver;
use gpui::{div, prelude::*, rgba, Context, Render, Window};
use hex_core::{AppEvent, AppMode, ChampSelectState, CoreHandle, LiveGameState};
use hex_data::{
    BenchHighlight, HeroPreview, HeroStats, MetaEngine, TeamRecommendations,
};
use tracing::{debug, info, warn};

use crate::lobby_view;
use crate::theme;
use crate::win32::{self, WindowRectInfo};

const BENCH_HIGHLIGHT_LIMIT: usize = 3;
const TEAM_RECOMMENDATION_LIMIT: usize = 5;

/// Root GPUI state and controller bridging `hex-core` events, `hex-data` queries, and Win32 windowing.
pub struct HexOverlayApp {
    pub mode: AppMode,
    pub lcu_connected: bool,
    pub champ_select: Option<ChampSelectState>,
    pub live_game: Option<LiveGameState>,
    pub bench_highlights: Vec<BenchHighlight>,
    pub team_recs: TeamRecommendations,
    pub current_hero_stats: Option<HeroStats>,
    pub expanded_hero_id: Option<i32>,
    pub selected_preview: Option<HeroPreview>,
    meta: Option<MetaEngine>,
    event_rx: Receiver<AppEvent>,
    core_handle: CoreHandle,
    last_window_sync: Option<(AppMode, Option<WindowRectInfo>)>,
    in_game_styles_applied: bool,
}

impl HexOverlayApp {
    /// Constructs the GPUI root entity, opening `MetaEngine::open_default()` with graceful fallback.
    pub fn new(event_rx: Receiver<AppEvent>, core_handle: CoreHandle) -> Self {
        let meta = match MetaEngine::open_default() {
            Ok(engine) => {
                info!("MetaEngine initialized successfully");
                Some(engine)
            }
            Err(err) => {
                warn!(error = %err, "failed to open MetaEngine; recommendations will be empty");
                None
            }
        };

        let mode = core_handle.current_mode();
        let lcu_connected = core_handle.is_lcu_connected();

        Self {
            mode,
            lcu_connected,
            champ_select: None,
            live_game: None,
            bench_highlights: Vec::new(),
            team_recs: TeamRecommendations::default(),
            current_hero_stats: None,
            expanded_hero_id: None,
            selected_preview: None,
            meta,
            event_rx,
            core_handle,
            last_window_sync: None,
            in_game_styles_applied: false,
        }
    }

    /// Drains pending `AppEvent`s from `hex-core` and synchronizes Win32 window docking/visibility.
    pub fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut dirty = false;

        while let Ok(event) = self.event_rx.try_recv() {
            dirty = true;
            match event {
                AppEvent::ModeChanged(next_mode) => {
                    info!(?next_mode, "overlay mode transitioned");
                    if next_mode != AppMode::InGameOverlay {
                        self.in_game_styles_applied = false;
                    }
                    if next_mode != AppMode::LobbyDocked {
                        self.expanded_hero_id = None;
                        self.selected_preview = None;
                    }
                    self.mode = next_mode;
                }
                AppEvent::LcuConnectionChanged(connected) => {
                    self.lcu_connected = connected;
                    if !connected {
                        self.champ_select = None;
                        self.bench_highlights.clear();
                        self.team_recs = TeamRecommendations::default();
                        self.current_hero_stats = None;
                    }
                }
                AppEvent::ChampSelectUpdated(state) => {
                    self.refresh_champ_select_data(&state);
                    self.champ_select = Some(state);
                }
                AppEvent::LiveGameUpdated(live) => {
                    self.live_game = Some(live);
                }
                AppEvent::AugmentOfferDetected {
                    candidates,
                    capture_ms,
                } => {
                    debug!(?candidates, capture_ms, "augment offer event received");
                }
                AppEvent::EndOfGameCaptured(eog) => {
                    info!(
                        game_mode = %eog.game_mode,
                        participants = eog.participants.len(),
                        "anonymized end-of-game stats captured"
                    );
                }
            }
        }

        self.sync_window_mode(window);

        if dirty {
            cx.notify();
        }
    }

    /// Toggles the inline pre-match preview drawer for `hero_id`.
    pub fn toggle_hero_preview(&mut self, hero_id: i32, cx: &mut Context<Self>) {
        if self.expanded_hero_id == Some(hero_id) {
            self.expanded_hero_id = None;
            self.selected_preview = None;
        } else {
            self.expanded_hero_id = Some(hero_id);
            self.selected_preview = self
                .meta
                .as_ref()
                .and_then(|engine| engine.get_hero_preview(hero_id));
        }
        cx.notify();
    }

    fn refresh_champ_select_data(&mut self, state: &ChampSelectState) {
        let Some(meta) = self.meta.as_ref() else {
            return;
        };

        let active_hero_id = if state.local_champion_id > 0 {
            state.local_champion_id
        } else {
            state.local_intent_id
        };

        self.current_hero_stats = if active_hero_id > 0 {
            meta.get_hero_stats(active_hero_id)
        } else {
            None
        };

        self.bench_highlights = meta.highlight_bench(&state.bench_champions, BENCH_HIGHLIGHT_LIMIT);

        let teammate_ids: Vec<i32> = state
            .my_team
            .iter()
            .filter(|slot| slot.cell_id != state.local_player_cell_id)
            .map(|slot| {
                if slot.champion_id > 0 {
                    slot.champion_id
                } else {
                    slot.pick_intent
                }
            })
            .filter(|&id| id > 0)
            .collect();

        self.team_recs = meta.recommend_for_team(
            &teammate_ids,
            &state.bench_champions,
            TEAM_RECOMMENDATION_LIMIT,
        );

        if let Some(hero_id) = self.expanded_hero_id {
            self.selected_preview = meta.get_hero_preview(hero_id);
        }
    }

    fn sync_window_mode(&mut self, window: &Window) {
        match self.mode {
            AppMode::LobbyDocked => {
                let target_rect = self.core_handle.lcu_pid().and_then(win32::find_window_by_pid);
                if self.last_window_sync != Some((AppMode::LobbyDocked, target_rect)) {
                    if let Err(err) = win32::set_lobby_dock_mode(window, target_rect) {
                        warn!(error = %err, "failed to dock lobby sidebar");
                    }
                    self.last_window_sync = Some((AppMode::LobbyDocked, target_rect));
                }
            }
            AppMode::InGameOverlay => {
                if !self.in_game_styles_applied
                    || self.last_window_sync.map(|(m, _)| m) != Some(AppMode::InGameOverlay)
                {
                    if let Err(err) = win32::apply_overlay_styles(window) {
                        warn!(error = %err, "failed to apply in-game overlay styles");
                    } else {
                        self.in_game_styles_applied = true;
                    }
                    self.last_window_sync = Some((AppMode::InGameOverlay, None));
                }
            }
            AppMode::Dormant => {
                if self.last_window_sync.map(|(m, _)| m) != Some(AppMode::Dormant) {
                    if let Err(err) = win32::hide_window(window) {
                        warn!(error = %err, "failed to hide dormant overlay window");
                    }
                    self.last_window_sync = Some((AppMode::Dormant, None));
                }
            }
        }
    }
}

impl Render for HexOverlayApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.mode {
            AppMode::LobbyDocked => lobby_view::render_lobby_sidebar(self, cx).into_any_element(),
            AppMode::InGameOverlay => div()
                .size_full()
                .p_3()
                .flex()
                .items_start()
                .justify_end()
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .border_1()
                        .border_color(theme::border_gold())
                        .bg(rgba(0x0A1428CC))
                        .text_xs()
                        .text_color(theme::cyan())
                        .child("HEXOVERLAY // IN-GAME STANDBY"),
                )
                .into_any_element(),
            AppMode::Dormant => div()
                .size_full()
                .bg(rgba(0x00000000))
                .into_any_element(),
        }
    }
}
