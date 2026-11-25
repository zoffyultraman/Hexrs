//! Root GPUI application entity, event-loop polling, WGC vision wiring, tray menu, and Win32 orchestration.

use std::sync::{Arc, RwLock};

use crossbeam_channel::{unbounded, Receiver};
use gpui::{div, prelude::*, rgba, Context, Render, Window};
use hex_core::{AppEvent, AppMode, ChampSelectState, CoreHandle, LiveGameState, RoiManifest};
use hex_data::{
    spawn_background_update, AugmentEvaluation, BenchHighlight, DynamicBuildPlan, EogReporter,
    HeroPreview, HeroStats, MetaEngine, TeamRecommendations, UpdateOutcome, UpdaterConfig,
};
use hex_vision::{SharedRoiManifest, VisionController};
use tracing::{debug, info, warn};

use crate::lobby_view;
use crate::overlay_view;
use crate::tray::{TrayAction, TrayController};
use crate::win32::{self, WindowRectInfo};

const BENCH_HIGHLIGHT_LIMIT: usize = 3;
const TEAM_RECOMMENDATION_LIMIT: usize = 5;

/// Root GPUI state and controller bridging `hex-core`, `hex-vision`, `hex-data`, `tray-icon`, and Win32.
pub struct HexOverlayApp {
    pub mode: AppMode,
    pub lcu_connected: bool,
    pub overlay_visible: bool,
    pub hotkey_interactive: bool,
    pub telemetry_enabled: bool,
    pub updater_refresh_requested: bool,
    pub champ_select: Option<ChampSelectState>,
    pub live_game: Option<LiveGameState>,
    pub bench_highlights: Vec<BenchHighlight>,
    pub team_recs: TeamRecommendations,
    pub current_hero_stats: Option<HeroStats>,
    pub expanded_hero_id: Option<i32>,
    pub selected_preview: Option<HeroPreview>,
    pub owned_augment_ids: Vec<i32>,
    pub offered_augments: Vec<AugmentEvaluation>,
    pub last_offer_capture_ms: Option<u64>,
    pub dynamic_build: Option<DynamicBuildPlan>,
    pub active_hero_name_zh: Option<String>,
    pub roi_manifest: SharedRoiManifest,
    meta: Option<MetaEngine>,
    event_rx: Receiver<AppEvent>,
    vision_rx: Receiver<AppEvent>,
    updater_config: UpdaterConfig,
    update_tx: crossbeam_channel::Sender<anyhow::Result<UpdateOutcome>>,
    update_rx: Receiver<anyhow::Result<UpdateOutcome>>,
    update_in_flight: bool,
    reporter: EogReporter,
    core_handle: CoreHandle,
    vision: VisionController,
    tray: Option<TrayController>,
    last_lobby_sync: Option<(Option<WindowRectInfo>, bool)>,
    last_overlay_sync: Option<(Option<WindowRectInfo>, bool, bool)>,
    last_dormant_hidden: bool,
}

impl HexOverlayApp {
    /// Constructs the GPUI root entity, initializing `MetaEngine`, `VisionController`, and tray status.
    pub fn new(
        event_rx: Receiver<AppEvent>,
        core_handle: CoreHandle,
        tray: Option<TrayController>,
    ) -> Self {
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

        let hashes = meta
            .as_ref()
            .map(MetaEngine::list_augment_hashes)
            .unwrap_or_default();
        let roi_manifest = Arc::new(RwLock::new(RoiManifest::default()));
        let (vision_tx, vision_rx) = unbounded::<AppEvent>();
        let vision = VisionController::start(hashes, Arc::clone(&roi_manifest), vision_tx);

        let updater_config = UpdaterConfig::from_env();
        let (update_tx, update_rx) = unbounded::<anyhow::Result<UpdateOutcome>>();
        spawn_background_update(updater_config.clone(), update_tx.clone());
        let reporter = EogReporter::from_env();

        let mode = core_handle.current_mode();
        let lcu_connected = core_handle.is_lcu_connected();
        if let Some(ref tray_ctrl) = tray {
            tray_ctrl.update_status(mode);
        }

        Self {
            mode,
            lcu_connected,
            overlay_visible: true,
            hotkey_interactive: false,
            telemetry_enabled: true,
            updater_refresh_requested: false,
            champ_select: None,
            live_game: None,
            bench_highlights: Vec::new(),
            team_recs: TeamRecommendations::default(),
            current_hero_stats: None,
            expanded_hero_id: None,
            selected_preview: None,
            owned_augment_ids: Vec::new(),
            offered_augments: Vec::new(),
            last_offer_capture_ms: None,
            dynamic_build: None,
            active_hero_name_zh: None,
            roi_manifest,
            meta,
            event_rx,
            vision_rx,
            updater_config,
            update_tx,
            update_rx,
            update_in_flight: true,
            reporter,
            core_handle,
            vision,
            tray,
            last_lobby_sync: None,
            last_overlay_sync: None,
            last_dormant_hidden: false,
        }
    }

    /// Drains pending `hex-core` & `hex-vision` events, polls Alt hotkey + tray, and syncs Win32 state.
    pub fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut dirty = false;

        while let Ok(event) = self.event_rx.try_recv() {
            dirty = true;
            self.handle_app_event(event);
        }

        while let Ok(event) = self.vision_rx.try_recv() {
            dirty = true;
            self.handle_app_event(event);
        }

        while let Ok(outcome) = self.update_rx.try_recv() {
            self.update_in_flight = false;
            self.updater_refresh_requested = false;
            match outcome {
                Ok(UpdateOutcome::AlreadyUpToDate { version }) => {
                    debug!(%version, "meta.db is already up to date");
                }
                Ok(UpdateOutcome::Updated {
                    version,
                    db_path,
                    roi_manifest,
                }) => {
                    info!(%version, path = %db_path.display(), "reloading hot-updated meta.db");
                    if let Some(meta) = self.meta.as_mut() {
                        match meta.reload_from_path(&db_path) {
                            Ok(()) => {
                                self.vision.update_hashes(meta.list_augment_hashes());
                                if let Some(cs) = self.champ_select.clone() {
                                    self.refresh_champ_select_data(&cs);
                                }
                                if let Some(live) = self.live_game.clone() {
                                    self.refresh_live_game_data(&live);
                                }
                                dirty = true;
                            }
                            Err(err) => {
                                warn!(error = %err, "failed to reload hot-updated meta.db");
                            }
                        }
                    }
                    if let Some(manifest) = roi_manifest {
                        if let Ok(mut guard) = self.roi_manifest.write() {
                            *guard = manifest;
                            dirty = true;
                        }
                    }
                }
                Err(err) => {
                    debug!(error = %err, "silent meta hot-update check skipped; using local DB");
                }
            }
        }

        if let Some(actions) = self.tray.as_ref().map(TrayController::poll_actions) {
            for action in actions {
                match action {
                    TrayAction::ToggleOverlay => {
                        self.overlay_visible = self
                            .tray
                            .as_ref()
                            .map(TrayController::is_overlay_checked)
                            .unwrap_or(!self.overlay_visible);
                        info!(visible = self.overlay_visible, "overlay visibility toggled via tray");
                        if self.mode == AppMode::InGameOverlay {
                            let mode_str = self
                                .live_game
                                .as_ref()
                                .map(|l| l.game_mode.as_str())
                                .unwrap_or("");
                            self.vision.set_active(self.overlay_visible, mode_str);
                        }
                        dirty = true;
                    }
                    TrayAction::ToggleTelemetry => {
                        self.telemetry_enabled = self
                            .tray
                            .as_ref()
                            .map(TrayController::is_telemetry_checked)
                            .unwrap_or(!self.telemetry_enabled);
                        self.reporter.set_enabled(self.telemetry_enabled);
                        info!(enabled = self.telemetry_enabled, "anonymized EOG telemetry toggled");
                    }
                    TrayAction::CheckDataUpdate => {
                        if !self.update_in_flight {
                            self.update_in_flight = true;
                            self.updater_refresh_requested = true;
                            spawn_background_update(
                                self.updater_config.clone(),
                                self.update_tx.clone(),
                            );
                            info!("manual data hot-update check started from tray");
                        }
                    }
                    TrayAction::OpenDataDir => {
                        if let Err(err) = win32::open_data_directory(&hex_data::data_dir()) {
                            warn!(error = %err, "failed to open local data directory");
                        }
                    }
                    TrayAction::Quit => {
                        info!("exit requested from system tray; shutting down services");
                        self.vision.shutdown();
                        self.core_handle.shutdown();
                        cx.quit();
                        return;
                    }
                }
            }
        }

        if self.mode == AppMode::InGameOverlay && self.overlay_visible {
            let alt_pressed = win32::is_alt_hotkey_pressed();
            if alt_pressed != self.hotkey_interactive {
                self.hotkey_interactive = alt_pressed;
                dirty = true;
            }
        } else if self.hotkey_interactive {
            self.hotkey_interactive = false;
            dirty = true;
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

    /// Dismisses the currently displayed floating augment offer badges.
    pub fn dismiss_augment_offer(&mut self, cx: &mut Context<Self>) {
        self.offered_augments.clear();
        self.last_offer_capture_ms = None;
        cx.notify();
    }

    fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::ModeChanged(next_mode) => {
                info!(?next_mode, "overlay mode transitioned");
                self.mode = next_mode;
                self.last_lobby_sync = None;
                self.last_overlay_sync = None;
                self.last_dormant_hidden = false;

                if let Some(ref tray_ctrl) = self.tray {
                    tray_ctrl.update_status(next_mode);
                }

                if next_mode != AppMode::LobbyDocked {
                    self.expanded_hero_id = None;
                    self.selected_preview = None;
                }

                if next_mode == AppMode::InGameOverlay {
                    let game_mode = self
                        .live_game
                        .as_ref()
                        .map(|l| l.game_mode.as_str())
                        .unwrap_or("");
                    self.vision.set_active(self.overlay_visible, game_mode);
                } else {
                    self.vision.set_active(false, "");
                    self.offered_augments.clear();
                    self.owned_augment_ids.clear();
                    self.last_offer_capture_ms = None;
                    self.dynamic_build = None;
                    self.active_hero_name_zh = None;
                    self.hotkey_interactive = false;
                }
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
                self.refresh_live_game_data(&live);
                self.live_game = Some(live);
            }
            AppEvent::AugmentOfferDetected {
                candidates,
                capture_ms,
            } => {
                debug!(?candidates, capture_ms, "augment offer detected by hex-vision");
                self.evaluate_detected_offer(&candidates, capture_ms);
            }
            AppEvent::EndOfGameCaptured(eog) => {
                info!(
                    game_mode = %eog.game_mode,
                    participants = eog.participants.len(),
                    telemetry_enabled = self.telemetry_enabled,
                    "anonymized end-of-game stats captured"
                );
                if self.telemetry_enabled {
                    self.reporter.submit_async(eog);
                }
            }
        }
    }

    fn resolve_active_hero_id(&self) -> Option<i32> {
        if let Some(live) = self.live_game.as_ref() {
            if let Some(id) = live.active_champion_id {
                return Some(id);
            }
            if let Some(meta) = self.meta.as_ref() {
                if let Some(id) = meta.resolve_champion_id_by_alias(&live.active_champion_name) {
                    return Some(id);
                }
            }
        }

        self.champ_select
            .as_ref()
            .map(|cs| {
                if cs.local_champion_id > 0 {
                    cs.local_champion_id
                } else {
                    cs.local_intent_id
                }
            })
            .filter(|&id| id > 0)
    }

    fn refresh_live_game_data(&mut self, live: &LiveGameState) {
        self.vision
            .set_active(self.mode == AppMode::InGameOverlay && self.overlay_visible, &live.game_mode);

        let Some(meta) = self.meta.as_ref() else {
            return;
        };

        let hero_id = live
            .active_champion_id
            .or_else(|| meta.resolve_champion_id_by_alias(&live.active_champion_name));

        self.active_hero_name_zh = hero_id
            .and_then(|id| meta.get_hero_stats(id))
            .map(|stats| stats.name_zh);

        self.dynamic_build = hero_id.and_then(|id| {
            meta.get_dynamic_build(id, &self.owned_augment_ids, &live.owned_item_ids)
        });
    }

    fn evaluate_detected_offer(&mut self, candidates: &[i32], capture_ms: u64) {
        let Some(meta) = self.meta.as_ref() else {
            return;
        };
        let hero_id = self.resolve_active_hero_id().unwrap_or(0);
        let evals = meta.evaluate_augment_offer(hero_id, candidates, &self.owned_augment_ids);
        if !evals.is_empty() {
            self.offered_augments = evals;
            self.last_offer_capture_ms = Some(capture_ms);
        }
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
                let target_rect = if self.overlay_visible {
                    self.core_handle.lcu_pid().and_then(win32::find_window_by_pid)
                } else {
                    None
                };
                let state = (target_rect, self.overlay_visible);
                if self.last_lobby_sync != Some(state) {
                    if let Err(err) = win32::set_lobby_dock_mode(window, target_rect) {
                        warn!(error = %err, "failed to dock lobby sidebar");
                    }
                    self.last_lobby_sync = Some(state);
                }
            }
            AppMode::InGameOverlay => {
                let target_rect = if self.overlay_visible {
                    self.core_handle.game_pid().and_then(win32::find_window_by_pid)
                } else {
                    None
                };
                let state = (target_rect, self.hotkey_interactive, self.overlay_visible);
                if self.last_overlay_sync != Some(state) {
                    if let Err(err) = win32::set_in_game_overlay_mode(
                        window,
                        target_rect,
                        self.hotkey_interactive,
                        self.overlay_visible,
                    ) {
                        warn!(error = %err, "failed to update in-game overlay window");
                    }
                    self.last_overlay_sync = Some(state);
                }
            }
            AppMode::Dormant => {
                if !self.last_dormant_hidden {
                    if let Err(err) = win32::hide_window(window) {
                        warn!(error = %err, "failed to hide dormant overlay window");
                    }
                    self.last_dormant_hidden = true;
                }
            }
        }
    }
}

impl Render for HexOverlayApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.mode {
            AppMode::LobbyDocked => lobby_view::render_lobby_sidebar(self, cx).into_any_element(),
            AppMode::InGameOverlay => {
                overlay_view::render_in_game_overlay(self, window, cx).into_any_element()
            }
            AppMode::Dormant => div()
                .size_full()
                .bg(rgba(0x00000000))
                .into_any_element(),
        }
    }
}
