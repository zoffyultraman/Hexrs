//! Root GPUI application entity, event-loop polling, WGC vision wiring, icon cache, diagnostics, and Win32 orchestration.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crossbeam_channel::{unbounded, Receiver};
use gpui::{div, prelude::*, px, rgba, Context, Div, FontWeight, Render, Rgba, Window};
use hex_core::{
    AppEvent, AppMode, ChampSelectState, CoreDiagnostics, CoreHandle, HeroTier, LiveGameState,
    RoiManifest,
};
use hex_data::{
    spawn_background_update, AugmentEvaluation, BenchHighlight, DynamicBuildPlan, EogReporter,
    HeroPreview, HeroStats, IconCache, MetaEngine, TeamRecommendations, UpdateOutcome,
    UpdaterConfig,
};
use hex_vision::{SharedRoiManifest, VisionController, VisionGameContext};
use tracing::{debug, info, warn};

use crate::lobby_view;
use crate::log_file_path;
use crate::overlay_view;
use crate::theme;
use crate::tray::{TrayAction, TrayController};
use crate::win32::{self, OverlayCorner, WindowRectInfo};

const BENCH_HIGHLIGHT_LIMIT: usize = 12;
const TEAM_RECOMMENDATION_LIMIT: usize = 5;
const PREVIEW_YASUO_HERO_ID: i32 = 157;

/// Root GPUI state and controller bridging `hex-core`, `hex-vision`, `hex-data`, `tray-icon`, and Win32.
pub struct HexOverlayApp {
    pub mode: AppMode,
    pub lcu_connected: bool,
    pub overlay_visible: bool,
    pub overlay_corner: OverlayCorner,
    pub telemetry_enabled: bool,
    pub updater_refresh_requested: bool,
    pub diagnostics_open: bool,
    pub preview_mode: Option<AppMode>,
    pub champ_select: Option<ChampSelectState>,
    pub active_champ_select_hero_id: Option<i32>,
    pub live_game: Option<LiveGameState>,
    pub team_hero_stats: Vec<HeroStats>,
    pub bench_highlights: Vec<BenchHighlight>,
    pub team_recs: TeamRecommendations,
    pub current_hero_stats: Option<HeroStats>,
    pub expanded_hero_id: Option<i32>,
    pub selected_preview: Option<HeroPreview>,
    pub owned_augment_ids: Vec<i32>,
    pub offered_augments: Vec<AugmentEvaluation>,
    pub last_offer_capture_ms: Option<u64>,
    pub dynamic_build: Option<DynamicBuildPlan>,
    pub active_hero_id: Option<i32>,
    pub active_hero_alias: Option<String>,
    pub active_hero_name_zh: Option<String>,
    pub active_hero_icon: Option<PathBuf>,
    pub item_icons: HashMap<i32, PathBuf>,
    pub roi_manifest: SharedRoiManifest,
    pub last_detected_lobby_rect: Option<WindowRectInfo>,
    pub last_detected_game_rect: Option<WindowRectInfo>,
    meta: Option<MetaEngine>,
    icon_cache: IconCache,
    icon_ready_rx: Receiver<()>,
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
    last_overlay_sync: Option<(Option<WindowRectInfo>, OverlayCorner, bool, bool)>,
    last_diagnostics_sync: bool,
    last_dormant_hidden: bool,
    auto_elevation_prompted: bool,
    last_auto_expanded_hero_id: Option<i32>,
    last_logged_hud_signature: Option<(Option<i32>, usize, bool)>,
    pending_repaint_ticks: u8,
}

impl HexOverlayApp {
    /// Constructs the GPUI root entity, initializing `MetaEngine`, `IconCache`, `VisionController`, and tray status.
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

        let (icon_ready_tx, icon_ready_rx) = unbounded::<()>();
        let icon_cache = IconCache::new(icon_ready_tx);

        let hashes = meta
            .as_ref()
            .map(MetaEngine::list_augment_hashes)
            .unwrap_or_default();
        let augment_names = meta
            .as_ref()
            .map(MetaEngine::list_augment_names)
            .unwrap_or_default();
        let roi_manifest = Arc::new(RwLock::new(RoiManifest::default()));
        let (vision_tx, vision_rx) = unbounded::<AppEvent>();
        let vision = VisionController::start(
            hashes,
            augment_names,
            Arc::clone(&roi_manifest),
            vision_tx,
        );

        let updater_config = UpdaterConfig::from_env();
        let (update_tx, update_rx) = unbounded::<anyhow::Result<UpdateOutcome>>();
        spawn_background_update(updater_config.clone(), update_tx.clone());
        let reporter = EogReporter::from_env();

        let mode = core_handle.current_mode();
        let lcu_connected = core_handle.is_lcu_connected();
        if let Some(ref tray_ctrl) = tray {
            tray_ctrl.update_status(mode);
            tray_ctrl.update_corner_label(OverlayCorner::default());
        }

        Self {
            mode,
            lcu_connected,
            overlay_visible: true,
            overlay_corner: OverlayCorner::default(),
            telemetry_enabled: true,
            updater_refresh_requested: false,
            diagnostics_open: false,
            preview_mode: None,
            champ_select: None,
            active_champ_select_hero_id: None,
            live_game: None,
            team_hero_stats: Vec::new(),
            bench_highlights: Vec::new(),
            team_recs: TeamRecommendations::default(),
            current_hero_stats: None,
            expanded_hero_id: None,
            selected_preview: None,
            owned_augment_ids: Vec::new(),
            offered_augments: Vec::new(),
            last_offer_capture_ms: None,
            dynamic_build: None,
            active_hero_id: None,
            active_hero_alias: None,
            active_hero_name_zh: None,
            active_hero_icon: None,
            item_icons: HashMap::new(),
            roi_manifest,
            last_detected_lobby_rect: None,
            last_detected_game_rect: None,
            meta,
            icon_cache,
            icon_ready_rx,
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
            last_diagnostics_sync: false,
            last_dormant_hidden: false,
            auto_elevation_prompted: false,
            last_auto_expanded_hero_id: None,
            last_logged_hud_signature: None,
            pending_repaint_ticks: 0,
        }
    }

    /// Drains pending `hex-core`, `hex-vision`, icon cache, and tray events, then syncs Win32 state.
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

        let mut icons_ready = false;
        while self.icon_ready_rx.try_recv().is_ok() {
            icons_ready = true;
        }
        if icons_ready {
            self.refresh_active_icons();
            dirty = true;
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
                                self.vision.update_augment_names(meta.list_augment_names());
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
                            let ctx = self
                                .live_game
                                .as_ref()
                                .map(live_game_to_vision_ctx)
                                .unwrap_or_default();
                            self.vision.update_game_context(self.overlay_visible, ctx);
                        }
                        self.reset_window_sync_cache();
                        dirty = true;
                    }
                    TrayAction::ToggleOverlayCorner => {
                        self.overlay_corner = match self.overlay_corner {
                            OverlayCorner::TopLeft => OverlayCorner::TopRight,
                            OverlayCorner::TopRight => OverlayCorner::TopLeft,
                        };
                        if let Some(ref tray_ctrl) = self.tray {
                            tray_ctrl.update_corner_label(self.overlay_corner);
                        }
                        info!(corner = ?self.overlay_corner, "in-game corner HUD position toggled");
                        self.reset_window_sync_cache();
                        dirty = true;
                    }
                    TrayAction::ToggleDiagnostics => {
                        if self.preview_mode.is_some() {
                            self.preview_mode = None;
                            self.offered_augments.clear();
                            self.last_offer_capture_ms = None;
                            self.diagnostics_open = true;
                            if let Some(ref tray_ctrl) = self.tray {
                                tray_ctrl.set_diagnostics_checked(true);
                            }
                        } else {
                            self.diagnostics_open = self
                                .tray
                                .as_ref()
                                .map(TrayController::is_diagnostics_checked)
                                .unwrap_or(!self.diagnostics_open);
                        }
                        info!(diagnostics_open = self.diagnostics_open, "diagnostics panel toggled via tray");
                        self.reset_window_sync_cache();
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
                    TrayAction::OpenLogFile => {
                        if let Err(err) = win32::open_log_file(&log_file_path()) {
                            warn!(error = %err, "failed to open runtime log file");
                        }
                    }
                    TrayAction::RelaunchAsAdmin => {
                        self.relaunch_as_admin(cx);
                        return;
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

        if !self.auto_elevation_prompted
            && self.mode == AppMode::Dormant
            && self.preview_mode.is_none()
        {
            let diag = self.core_handle.diagnostics();
            if diag.lcu_pid.is_some() && diag.lcu_port.is_none() && !diag.is_elevated {
                self.auto_elevation_prompted = true;
                self.diagnostics_open = true;
                if let Some(ref tray_ctrl) = self.tray {
                    tray_ctrl.set_diagnostics_checked(true);
                }
                self.reset_window_sync_cache();
                info!(
                    "detected unprivileged LCU access gap; auto-opening diagnostics panel for UAC prompt"
                );
                dirty = true;
            }
        }

        if self.diagnostics_open && self.preview_mode.is_none() {
            // Keep live CoreDiagnostics values refreshed while diagnostics panel is visible.
            dirty = true;
        }

        if self.sync_window_mode(window) {
            self.pending_repaint_ticks = 4;
        }
        if self.pending_repaint_ticks > 0 {
            self.pending_repaint_ticks -= 1;
            window.refresh();
            dirty = true;
        }

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

    /// Sets or exits UI preview mode from the diagnostics panel.
    pub fn set_preview_mode(&mut self, mode: Option<AppMode>, cx: &mut Context<Self>) {
        self.preview_mode = mode;
        self.offered_augments.clear();
        self.last_offer_capture_ms = None;
        if mode == Some(AppMode::InGameOverlay) && self.live_game.is_none() {
            self.populate_hero_hud_state(Some(PREVIEW_YASUO_HERO_ID), None, &[]);
        } else if mode.is_none() && self.mode != AppMode::InGameOverlay {
            self.clear_in_game_hud_state();
        }
        self.reset_window_sync_cache();
        cx.notify();
    }

    /// Previews the horizontal 3-card augment strip using real augments from `MetaEngine`.
    pub fn preview_augment_bar(&mut self, cx: &mut Context<Self>) {
        self.preview_mode = Some(AppMode::InGameOverlay);
        if self.live_game.is_none() {
            self.populate_hero_hud_state(Some(PREVIEW_YASUO_HERO_ID), None, &[]);
        }
        let sample_ids: Vec<i32> = self
            .meta
            .as_ref()
            .map(|m| {
                m.list_augment_hashes()
                    .into_iter()
                    .take(3)
                    .map(|entry| entry.augment_id)
                    .collect()
            })
            .unwrap_or_default();

        if !sample_ids.is_empty() {
            self.evaluate_detected_offer(&sample_ids, 1);
        }
        self.reset_window_sync_cache();
        cx.notify();
    }

    /// Closes the diagnostics panel and syncs the tray check item.
    pub fn close_diagnostics(&mut self, cx: &mut Context<Self>) {
        self.diagnostics_open = false;
        self.preview_mode = None;
        self.offered_augments.clear();
        self.last_offer_capture_ms = None;
        if let Some(ref tray_ctrl) = self.tray {
            tray_ctrl.set_diagnostics_checked(false);
        }
        if self.mode != AppMode::InGameOverlay {
            self.clear_in_game_hud_state();
        }
        self.reset_window_sync_cache();
        cx.notify();
    }

    /// Triggers UAC elevation and exits this unprivileged instance upon success.
    pub fn relaunch_as_admin(&mut self, cx: &mut Context<Self>) {
        match win32::relaunch_as_administrator() {
            Ok(()) => {
                info!("relaunching HexOverlay with UAC elevation; shutting down current instance");
                self.vision.shutdown();
                self.core_handle.shutdown();
                cx.quit();
            }
            Err(err) => {
                warn!(error = %err, "failed to relaunch HexOverlay as administrator");
            }
        }
    }

    fn reset_window_sync_cache(&mut self) {
        self.last_lobby_sync = None;
        self.last_overlay_sync = None;
        self.last_diagnostics_sync = false;
        self.last_dormant_hidden = false;
    }

    fn clear_in_game_hud_state(&mut self) {
        self.owned_augment_ids.clear();
        self.offered_augments.clear();
        self.last_offer_capture_ms = None;
        self.dynamic_build = None;
        self.active_hero_id = None;
        self.active_hero_alias = None;
        self.active_hero_name_zh = None;
        self.active_hero_icon = None;
        self.item_icons.clear();
        self.last_logged_hud_signature = None;
    }

    fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::ModeChanged(next_mode) => {
                info!(?next_mode, "overlay mode transitioned");
                self.mode = next_mode;
                if self.diagnostics_open
                    && self.preview_mode.is_none()
                    && next_mode != AppMode::Dormant
                {
                    self.diagnostics_open = false;
                    if let Some(ref tray_ctrl) = self.tray {
                        tray_ctrl.set_diagnostics_checked(false);
                    }
                }
                self.reset_window_sync_cache();

                if let Some(ref tray_ctrl) = self.tray {
                    tray_ctrl.update_status(next_mode);
                }

                if next_mode != AppMode::LobbyDocked {
                    self.expanded_hero_id = None;
                    self.selected_preview = None;
                    self.last_auto_expanded_hero_id = None;
                }

                if next_mode == AppMode::InGameOverlay {
                    if let Some(live) = self.live_game.clone() {
                        self.refresh_live_game_data(&live);
                    } else {
                        self.populate_hero_hud_state(self.active_champ_select_hero_id, None, &[]);
                        self.vision
                            .update_game_context(self.overlay_visible, VisionGameContext::default());
                    }
                } else {
                    self.vision
                        .update_game_context(false, VisionGameContext::default());
                    self.clear_in_game_hud_state();
                }
            }
            AppEvent::LcuConnectionChanged(connected) => {
                self.lcu_connected = connected;
                if !connected {
                    self.champ_select = None;
                    self.active_champ_select_hero_id = None;
                    self.team_hero_stats.clear();
                    self.bench_highlights.clear();
                    self.team_recs = TeamRecommendations::default();
                    self.current_hero_stats = None;
                    self.expanded_hero_id = None;
                    self.selected_preview = None;
                    self.last_auto_expanded_hero_id = None;
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
            AppEvent::AugmentOfferCleared => {
                if !self.offered_augments.is_empty() {
                    debug!("clearing augment offer strip");
                    self.offered_augments.clear();
                    self.last_offer_capture_ms = None;
                    self.reset_window_sync_cache();
                }
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

    fn evaluate_detected_offer(&mut self, candidates: &[i32], capture_ms: u64) {
        let Some(meta) = self.meta.as_ref() else {
            return;
        };
        let hero_id = self
            .active_hero_id
            .or(self.active_champ_select_hero_id)
            .unwrap_or(0);

        let mut slots = Vec::with_capacity(candidates.len().min(3));
        let mut matched_any = false;

        for &aug_id in candidates.iter().take(3) {
            if aug_id > 0 {
                if let Some(eval) = meta
                    .evaluate_augment_offer(hero_id, &[aug_id], &self.owned_augment_ids)
                    .into_iter()
                    .next()
                {
                    matched_any = true;
                    slots.push(eval);
                    continue;
                }
            }
            slots.push(placeholder_augment_eval());
        }

        if matched_any {
            self.offered_augments = slots;
            self.last_offer_capture_ms = Some(capture_ms);
            self.reset_window_sync_cache();
        }
    }

    fn refresh_live_game_data(&mut self, live: &LiveGameState) {
        self.vision.update_game_context(
            self.mode == AppMode::InGameOverlay && self.overlay_visible,
            live_game_to_vision_ctx(live),
        );

        let hero_id = live
            .active_champion_id
            .or_else(|| {
                self.meta
                    .as_ref()
                    .and_then(|meta| meta.resolve_champion_id_by_alias(&live.active_champion_name))
            })
            .or(self.active_champ_select_hero_id);

        let fallback_name = (!live.active_champion_name.is_empty())
            .then_some(live.active_champion_name.as_str());
        self.populate_hero_hud_state(hero_id, fallback_name, &live.owned_item_ids);
    }

    fn populate_hero_hud_state(
        &mut self,
        hero_id: Option<i32>,
        fallback_name: Option<&str>,
        owned_item_ids: &[i32],
    ) {
        let stats = hero_id.and_then(|id| self.meta.as_ref()?.get_hero_stats(id));

        self.active_hero_id = hero_id;
        self.active_hero_alias = stats
            .as_ref()
            .map(|s| s.alias.clone())
            .or_else(|| fallback_name.map(str::to_owned));
        self.active_hero_name_zh = stats
            .map(|s| s.name_zh)
            .or_else(|| fallback_name.map(str::to_owned));
        self.dynamic_build = self.meta.as_ref().map(|meta| {
            meta.get_dynamic_build_or_fallback(hero_id, &self.owned_augment_ids, owned_item_ids)
        });

        let sig = (
            self.active_hero_id,
            self.dynamic_build.as_ref().map(|p| p.items.len()).unwrap_or(0),
            self.dynamic_build.is_some(),
        );
        if self.last_logged_hud_signature != Some(sig) {
            self.last_logged_hud_signature = Some(sig);
            info!(
                ?hero_id,
                hero_name = ?self.active_hero_name_zh,
                item_count = sig.1,
                "updated in-game equipment HUD"
            );
        }

        self.refresh_active_icons();
    }

    fn refresh_active_icons(&mut self) {
        self.active_hero_icon = self
            .active_hero_id
            .and_then(|id| self.icon_cache.champion_icon(id, self.active_hero_alias.as_deref()));

        if let Some(plan) = self.dynamic_build.as_ref() {
            for item in plan.items.iter().take(3) {
                if let Some(path) = self.icon_cache.item_icon(item.id) {
                    self.item_icons.insert(item.id, path);
                }
            }
        }
    }

    fn refresh_champ_select_data(&mut self, state: &ChampSelectState) {
        let active_hero_id = if state.local_champion_id > 0 {
            state.local_champion_id
        } else {
            state.local_intent_id
        };
        self.active_champ_select_hero_id = if active_hero_id > 0 {
            Some(active_hero_id)
        } else {
            None
        };

        let Some(meta) = self.meta.as_ref() else {
            return;
        };

        self.current_hero_stats = if active_hero_id > 0 {
            meta.get_hero_stats(active_hero_id)
        } else {
            None
        };

        let mut seen_team_ids = Vec::with_capacity(state.my_team.len() + 1);
        if active_hero_id > 0 {
            seen_team_ids.push(active_hero_id);
        }
        for slot in &state.my_team {
            let slot_hero_id = if slot.champion_id > 0 {
                slot.champion_id
            } else {
                slot.pick_intent
            };
            if slot_hero_id > 0 && !seen_team_ids.contains(&slot_hero_id) {
                seen_team_ids.push(slot_hero_id);
            }
        }
        self.team_hero_stats = seen_team_ids
            .iter()
            .filter_map(|&id| meta.get_hero_stats(id))
            .collect();

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

        if active_hero_id > 0 {
            if self.last_auto_expanded_hero_id != Some(active_hero_id) {
                self.last_auto_expanded_hero_id = Some(active_hero_id);
                self.expanded_hero_id = Some(active_hero_id);
                self.selected_preview = meta.get_hero_preview(active_hero_id);
            } else if let Some(hero_id) = self.expanded_hero_id {
                self.selected_preview = meta.get_hero_preview(hero_id);
            }
        } else {
            self.last_auto_expanded_hero_id = None;
            if let Some(hero_id) = self.expanded_hero_id {
                self.selected_preview = meta.get_hero_preview(hero_id);
            }
        }
    }

    fn sync_window_mode(&mut self, window: &Window) -> bool {
        if self.diagnostics_open && self.preview_mode.is_none() {
            if !self.last_diagnostics_sync {
                if let Err(err) = win32::set_diagnostics_window_mode(window) {
                    warn!(error = %err, "failed to show diagnostics window");
                }
                self.last_diagnostics_sync = true;
                self.last_dormant_hidden = false;
                return true;
            }
            return false;
        }
        self.last_diagnostics_sync = false;

        let effective_mode = self.preview_mode.unwrap_or(self.mode);
        let is_preview = self.preview_mode.is_some();

        match effective_mode {
            AppMode::LobbyDocked => {
                let detected_rect = win32::find_league_lobby_window(self.core_handle.lcu_pid());
                if detected_rect != self.last_detected_lobby_rect {
                    match (self.last_detected_lobby_rect, detected_rect) {
                        (None, Some(rect)) => info!(?rect, "detected League lobby window"),
                        (Some(_), None) => info!("League lobby window no longer visible"),
                        (Some(_), Some(rect)) => debug!(?rect, "League lobby window moved/resized"),
                        (None, None) => {}
                    }
                    self.last_detected_lobby_rect = detected_rect;
                }

                let target_rect = if is_preview {
                    detected_rect.or_else(|| Some(win32::default_preview_lobby_rect()))
                } else if self.overlay_visible {
                    detected_rect
                } else {
                    None
                };

                let state = (target_rect, is_preview || self.overlay_visible);
                if self.last_lobby_sync != Some(state) {
                    if let Err(err) = win32::set_lobby_dock_mode(window, target_rect) {
                        warn!(error = %err, "failed to dock lobby sidebar");
                    }
                    self.last_lobby_sync = Some(state);
                    return true;
                }
                false
            }
            AppMode::InGameOverlay => {
                let detected_rect = win32::find_league_game_window(self.core_handle.game_pid());
                if detected_rect != self.last_detected_game_rect {
                    match (self.last_detected_game_rect, detected_rect) {
                        (None, Some(rect)) => info!(?rect, "detected League in-game window"),
                        (Some(_), None) => info!("League in-game window no longer visible"),
                        (Some(_), Some(rect)) => debug!(?rect, "League in-game window moved/resized"),
                        (None, None) => {}
                    }
                    self.last_detected_game_rect = detected_rect;
                }

                let target_rect = if is_preview {
                    detected_rect.or_else(|| Some(win32::default_preview_game_rect()))
                } else if self.overlay_visible {
                    detected_rect
                } else {
                    None
                };
                let visible = is_preview || self.overlay_visible;
                let has_augment_offer = !self.offered_augments.is_empty();

                let state = (target_rect, self.overlay_corner, visible, has_augment_offer);
                if self.last_overlay_sync != Some(state) {
                    let res = if has_augment_offer {
                        win32::set_in_game_augment_bar_mode(window, target_rect, visible)
                    } else {
                        win32::set_in_game_overlay_mode(
                            window,
                            target_rect,
                            self.overlay_corner,
                            visible,
                        )
                    };
                    if let Err(err) = res {
                        warn!(error = %err, has_augment_offer, "failed to update in-game overlay window");
                    }
                    self.last_overlay_sync = Some(state);
                    return true;
                }
                false
            }
            AppMode::Dormant => {
                if !self.last_dormant_hidden {
                    if let Err(err) = win32::hide_window(window) {
                        warn!(error = %err, "failed to hide dormant overlay window");
                    }
                    self.last_dormant_hidden = true;
                }
                false
            }
        }
    }
}

fn live_game_to_vision_ctx(live: &LiveGameState) -> VisionGameContext {
    VisionGameContext {
        game_mode: live.game_mode.clone(),
        level: live.level,
        game_time_secs: live.game_time_secs,
        is_dead: live.is_dead,
        respawn_timer: live.respawn_timer,
    }
}

impl Render for HexOverlayApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.diagnostics_open && self.preview_mode.is_none() {
            return render_diagnostics_panel(self, cx).into_any_element();
        }

        match self.preview_mode.unwrap_or(self.mode) {
            AppMode::LobbyDocked => lobby_view::render_lobby_sidebar(self, cx).into_any_element(),
            AppMode::InGameOverlay => overlay_view::render_in_game_overlay(self).into_any_element(),
            AppMode::Dormant => div()
                .size_full()
                .bg(rgba(0x00000000))
                .into_any_element(),
        }
    }
}

fn placeholder_augment_eval() -> AugmentEvaluation {
    AugmentEvaluation {
        augment_id: 0,
        name_zh: String::new(),
        rarity: String::new(),
        desc_zh: String::new(),
        tier: HeroTier::B,
        win_rate: 50.0,
        pick_rate: 0.0,
        synergy_alert: None,
    }
}

fn render_diagnostics_panel(app: &HexOverlayApp, cx: &mut Context<HexOverlayApp>) -> Div {
    let diag: CoreDiagnostics = app.core_handle.diagnostics();
    let lobby_rect = app
        .last_detected_lobby_rect
        .or_else(|| win32::find_league_lobby_window(diag.lcu_pid));
    let game_rect = app
        .last_detected_game_rect
        .or_else(|| win32::find_league_game_window(diag.game_pid));

    let lobby_rect_str = format_rect_info(lobby_rect);
    let game_rect_str = format_rect_info(game_rect);
    let (meta_status, counts_str, data_version_str) = match app.meta.as_ref() {
        Some(engine) => {
            let (champs, augs) = engine.stats_counts();
            let version = engine.data_version();
            (
                "已加载内存 SQLite",
                format!("{champs} 英雄 / {augs} 海克斯"),
                if version.is_empty() {
                    "-".to_owned()
                } else {
                    version
                },
            )
        }
        None => (
            "未加载",
            "0 英雄 / 0 海克斯".to_owned(),
            "-".to_owned(),
        ),
    };

    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(theme::canvas_bg())
        .border_1()
        .border_color(theme::border_gold())
        .text_color(theme::text_primary())
        .child(
            div()
                .px_3()
                .py_2()
                .bg(theme::card_bg())
                .border_b_1()
                .border_color(theme::border_subtle())
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme::gold())
                        .child("HexOverlay · 运行诊断面板"),
                )
                .child(
                    div()
                        .id("close-diagnostics-btn")
                        .cursor_pointer()
                        .px_2()
                        .py(px(2.0))
                        .rounded_md()
                        .bg(theme::card_elevated_bg())
                        .border_1()
                        .border_color(theme::border_subtle())
                        .hover(|s| s.bg(theme::warning_bg()))
                        .text_xs()
                        .text_color(theme::text_primary())
                        .child("关闭 [×]")
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.close_diagnostics(cx);
                        })),
                ),
        )
        .child(
            div()
                .id("diagnostics-scroll")
                .flex_1()
                .overflow_y_scroll()
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .p_2()
                        .rounded_lg()
                        .bg(theme::card_bg())
                        .border_1()
                        .border_color(theme::border_subtle())
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme::cyan())
                                .child("核心连接与状态机"),
                        )
                        .child(diag_row("当前模式", format!("{:?}", diag.mode)))
                        .child(diag_row(
                            "LCU PID / Port",
                            format!(
                                "{} / {}",
                                diag.lcu_pid
                                    .map(|p| p.to_string())
                                    .unwrap_or_else(|| "未检测到".to_owned()),
                                diag.lcu_port
                                    .map(|p| p.to_string())
                                    .unwrap_or_else(|| "-".to_owned())
                            ),
                        ))
                        .child(diag_row(
                            "LCU WS / 选人态",
                            format!(
                                "{} / {}",
                                if diag.lcu_connected_ws { "已连接" } else { "未连接" },
                                if diag.in_champ_select { "选人中" } else { "否" }
                            ),
                        ))
                        .child(diag_row(
                            "游戏进程 / Live 2999",
                            format!(
                                "{} (PID {}) / {}",
                                if diag.game_running { "运行中" } else { "未运行" },
                                diag.game_pid
                                    .map(|p| p.to_string())
                                    .unwrap_or_else(|| "-".to_owned()),
                                if diag.live_api_active { "活跃" } else { "等待中" }
                            ),
                        ))
                        .child(diag_row("MetaEngine 数据库", meta_status.to_owned()))
                        .child(diag_row("特征库规模", counts_str))
                        .child(diag_row("数据版本", data_version_str))
                        .child(diag_row_colored(
                            "当前权限",
                            if diag.is_elevated {
                                "管理员 (已提权)".to_owned()
                            } else {
                                "普通用户 (若国服提权请点下方提权)".to_owned()
                            },
                            if diag.is_elevated {
                                theme::cyan()
                            } else {
                                theme::warning_text()
                            },
                        ))
                        .when_some(
                            diag.lcu_diagnostic
                                .as_ref()
                                .filter(|_| diag.lcu_port.is_none()),
                            |card, msg| {
                                card.child(
                                    div()
                                        .mt_1()
                                        .p_2()
                                        .rounded_md()
                                        .bg(theme::warning_bg())
                                        .border_1()
                                        .border_color(theme::warning_border())
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(theme::warning_text())
                                                .child("LCU 诊断"),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme::text_primary())
                                                .child(msg.clone()),
                                        ),
                                )
                            },
                        )
                        .when(!diag.is_elevated, |card| {
                            card.child(
                                div()
                                    .id("relaunch-admin-btn")
                                    .mt_1()
                                    .cursor_pointer()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(theme::warning_bg())
                                    .border_1()
                                    .border_color(theme::warning_border())
                                    .hover(|s| s.bg(theme::card_elevated_bg()))
                                    .flex()
                                    .justify_center()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::warning_text())
                                    .child("以管理员身份重启 (UAC 提权)")
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.relaunch_as_admin(cx);
                                    })),
                            )
                        }),
                )
                .child(
                    div()
                        .p_2()
                        .rounded_lg()
                        .bg(theme::card_bg())
                        .border_1()
                        .border_color(theme::border_subtle())
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme::cyan())
                                .child("Win32 目标窗口定位"),
                        )
                        .child(diag_row("大厅窗口 (RCLIENT)", lobby_rect_str))
                        .child(diag_row("局内窗口 (RiotWindowClass)", game_rect_str)),
                )
                .child(
                    div()
                        .p_2()
                        .rounded_lg()
                        .bg(theme::card_bg())
                        .border_1()
                        .border_color(theme::border_gold())
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme::gold())
                                .child("UI 渲染快速预览（无需对局即可测试）"),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    div()
                                        .id("preview-lobby-btn")
                                        .flex_1()
                                        .cursor_pointer()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(theme::gold_dim())
                                        .border_1()
                                        .border_color(theme::border_gold())
                                        .hover(|s| s.bg(theme::card_elevated_bg()))
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme::gold())
                                        .child("预览选人侧边栏")
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            this.set_preview_mode(Some(AppMode::LobbyDocked), cx);
                                        })),
                                )
                                .child(
                                    div()
                                        .id("preview-overlay-btn")
                                        .flex_1()
                                        .cursor_pointer()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(theme::cyan_dim())
                                        .border_1()
                                        .border_color(theme::border_cyan())
                                        .hover(|s| s.bg(theme::card_elevated_bg()))
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme::cyan())
                                        .child("预览局内悬浮窗")
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            this.set_preview_mode(Some(AppMode::InGameOverlay), cx);
                                        })),
                                )
                                .child(
                                    div()
                                        .id("preview-augment-bar-btn")
                                        .flex_1()
                                        .cursor_pointer()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(theme::synergy_gold_bg())
                                        .border_1()
                                        .border_color(theme::synergy_gold_border())
                                        .hover(|s| s.bg(theme::card_elevated_bg()))
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme::synergy_gold_text())
                                        .child("预览海克斯识别条")
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            this.preview_augment_bar(cx);
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child("提示：局内小窗永久穿透，预览时右键托盘“显示诊断面板”即可切回。"),
                        ),
                )
                .child(
                    div()
                        .p_2()
                        .rounded_lg()
                        .bg(theme::card_bg())
                        .border_1()
                        .border_color(theme::border_subtle())
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme::gold())
                                .child("排查建议"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child("1. 局内请使用“无边框窗口化”而非全屏独占，否则系统级置顶悬浮窗将被独占画面遮挡。"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child("2. 若 WeGame / 客户端以管理员身份运行导致无法识别，请点击上方按钮或右键以管理员身份运行。"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child(format!("3. 完整运行日志位于：{}", log_file_path().display())),
                        ),
                ),
        )
}

fn diag_row(label: &str, value: String) -> Div {
    diag_row_colored(label, value, theme::text_primary())
}

fn diag_row_colored(label: &str, value: String, value_color: Rgba) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .text_xs()
                .text_color(theme::text_muted())
                .child(label.to_owned()),
        )
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(value_color)
                .child(value),
        )
}

fn format_rect_info(rect: Option<WindowRectInfo>) -> String {
    match rect {
        Some(r) => format!(
            "{}x{} @ ({},{}) {}DPI",
            r.width(),
            r.height(),
            r.left,
            r.top,
            r.dpi
        ),
        None => "未找到可见窗口".to_owned(),
    }
}
