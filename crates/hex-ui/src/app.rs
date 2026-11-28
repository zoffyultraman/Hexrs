//! Root GPUI application entity, event-loop polling, WGC vision wiring, icon cache, diagnostics, and Win32 orchestration.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crossbeam_channel::{unbounded, Receiver};
use gpui::{div, prelude::*, px, Context, Div, FontWeight, Render, Rgba, Window};
use hex_core::{
    AppEvent, AppMode, AugmentOfferLayout, ChampSelectState, CoreDiagnostics, CoreHandle,
    HeroTier, LiveGameState, LoadingPlayerSlot, LoadingScreenSnapshot, LoadingTeamSide,
    PremadeGroupSummary, RatingTitlesConfig, RoiManifest, SummonerRatingStatus,
    SummonerRatingSummary, TeamRatingsSnapshot,
};
use hex_data::{
    spawn_background_update, AugmentEvaluation, BenchHighlight, DynamicBuildPlan, EogReporter,
    HeroPreview, HeroStats, IconCache, MetaEngine, TeamRecommendations, UpdateOutcome,
    UpdaterConfig,
};
use hex_vision::{SharedRoiManifest, VisionController, VisionGameContext};
use tracing::{debug, info, warn};

use crate::loading_overlay_view::{
    compute_team_loading_summary, LoadingCardHeroData, LoadingCardSlotData, LoadingOverlayState,
};
use crate::lobby_badge_view::{
    BenchBadgeState, CardBadgeState, CardSlotBadgeData, SlotBadgeData, SummonerBadgeData,
    TeamBadgeState, TeamSlotBadgeEntry,
};
use crate::lobby_view;
use crate::log_file_path;
use crate::overlay_view;
use crate::theme;
use crate::tray::{TrayAction, TrayController};
use crate::win32::{self, OverlayCorner, Win32WindowAction, Win32WindowCommand, WindowRectInfo};

const BENCH_HIGHLIGHT_LIMIT: usize = 12;
const TEAM_RECOMMENDATION_LIMIT: usize = 5;
const PREVIEW_YASUO_HERO_ID: i32 = 157;

/// Root GPUI state and controller bridging `hex-core`, `hex-vision`, `hex-data`, `tray-icon`, and Win32.
pub struct HexrsApp {
    pub mode: AppMode,
    pub lcu_connected: bool,
    pub overlay_visible: bool,
    pub overlay_corner: OverlayCorner,
    pub in_game_hud_collapsed: bool,
    pub telemetry_enabled: bool,
    pub updater_refresh_requested: bool,
    pub diagnostics_open: bool,
    pub preview_mode: Option<AppMode>,
    pub champ_select: Option<ChampSelectState>,
    pub active_champ_select_hero_id: Option<i32>,
    pub live_game: Option<LiveGameState>,
    pub team_hero_stats: Vec<HeroStats>,
    pub team_ratings: Option<TeamRatingsSnapshot>,
    pub bench_highlights: Vec<BenchHighlight>,
    pub team_recs: TeamRecommendations,
    pub current_hero_stats: Option<HeroStats>,
    pub expanded_hero_id: Option<i32>,
    pub selected_preview: Option<HeroPreview>,
    pub owned_augment_ids: Vec<i32>,
    pub offered_augments: Vec<AugmentEvaluation>,
    pub offered_augment_layout: Option<AugmentOfferLayout>,
    pub augment_bar_gap_dip: f32,
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
    pub lobby_focused: bool,
    pub game_focused: bool,
    pub loading_screen: Option<LoadingScreenSnapshot>,
    pub preview_loading_screen: bool,
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
    last_bench_badge_sync: Option<(Option<WindowRectInfo>, usize)>,
    last_team_badge_sync: Option<Option<WindowRectInfo>>,
    last_card_badge_sync: Option<(Option<WindowRectInfo>, usize)>,
    last_loading_sync: Option<(Option<WindowRectInfo>, bool)>,
    last_overlay_sync: Option<(
        Option<WindowRectInfo>,
        OverlayCorner,
        bool,
        bool,
        usize,
        Option<(usize, i32, i32, i32)>,
    )>,
    last_diagnostics_sync: bool,
    last_dormant_hidden: bool,
    auto_elevation_prompted: bool,
    last_auto_expanded_hero_id: Option<i32>,
    last_logged_hud_signature: Option<(Option<i32>, usize, bool)>,
    rating_titles: RatingTitlesConfig,
    rating_titles_mtime: Option<std::time::SystemTime>,
}

impl HexrsApp {
    /// Constructs the GPUI root entity, initializing `MetaEngine`, `IconCache`, `VisionController`, and tray status.
    pub fn new(
        event_rx: Receiver<AppEvent>,
        core_handle: CoreHandle,
        tray: Option<TrayController>,
    ) -> Self {
        let meta = match MetaEngine::open_default() {
            Ok(engine) => {
                let (hero_count, augment_count) = engine.stats_counts();
                info!(
                    data_version = %engine.data_version(),
                    hero_count,
                    augment_count,
                    "MetaEngine initialized successfully"
                );
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

        let titles_path = rating_titles_path();
        let rating_titles = RatingTitlesConfig::load_or_init(&titles_path);
        let rating_titles_mtime = read_file_mtime(&titles_path);

        Self {
            mode,
            lcu_connected,
            overlay_visible: true,
            overlay_corner: OverlayCorner::default(),
            in_game_hud_collapsed: false,
            telemetry_enabled: true,
            updater_refresh_requested: false,
            diagnostics_open: false,
            preview_mode: None,
            champ_select: None,
            active_champ_select_hero_id: None,
            live_game: None,
            team_hero_stats: Vec::new(),
            team_ratings: None,
            bench_highlights: Vec::new(),
            team_recs: TeamRecommendations::default(),
            current_hero_stats: None,
            expanded_hero_id: None,
            selected_preview: None,
            owned_augment_ids: Vec::new(),
            offered_augments: Vec::new(),
            offered_augment_layout: None,
            augment_bar_gap_dip: 28.0,
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
            lobby_focused: false,
            game_focused: false,
            loading_screen: None,
            preview_loading_screen: false,
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
            last_bench_badge_sync: None,
            last_team_badge_sync: None,
            last_card_badge_sync: None,
            last_loading_sync: None,
            last_overlay_sync: None,
            last_diagnostics_sync: false,
            last_dormant_hidden: false,
            auto_elevation_prompted: false,
            last_auto_expanded_hero_id: None,
            last_logged_hud_signature: None,
            rating_titles,
            rating_titles_mtime,
        }
    }

    /// Drains pending `hex-core`, `hex-vision`, icon cache, and tray events, returning any deferred Win32 command.
    pub fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Win32WindowCommand> {
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
                        if self.preview_mode.is_some() || self.preview_loading_screen {
                            self.preview_mode = None;
                            self.preview_loading_screen = false;
                            self.clear_offered_augment_state();
                            self.diagnostics_open = true;
                            if let Some(ref tray_ctrl) = self.tray {
                                tray_ctrl.set_diagnostics_checked(true);
                            }
                        } else {
                            self.preview_loading_screen = false;
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
                        return None;
                    }
                    TrayAction::Quit => {
                        info!("exit requested from system tray; shutting down services");
                        self.vision.shutdown();
                        self.core_handle.shutdown();
                        cx.quit();
                        return None;
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

        let pending_cmd = self.sync_window_mode(window);

        if dirty {
            cx.notify();
        }

        pending_cmd
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

    /// Toggles the in-game corner build HUD between expanded (`324x204`) and compact header (`324x46`).
    pub fn toggle_in_game_hud_collapsed(&mut self, cx: &mut Context<Self>) {
        self.in_game_hud_collapsed = !self.in_game_hud_collapsed;
        info!(
            collapsed = self.in_game_hud_collapsed,
            "toggled in-game corner build HUD collapsed state"
        );
        self.reset_window_sync_cache();
        cx.notify();
    }

    /// Sets or exits UI preview mode from the diagnostics panel.
    pub fn set_preview_mode(&mut self, mode: Option<AppMode>, cx: &mut Context<Self>) {
        self.preview_mode = mode;
        self.preview_loading_screen = false;
        self.clear_offered_augment_state();
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
        self.preview_loading_screen = false;
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
            self.evaluate_detected_offer(&sample_ids, 1, None);
        }
        self.reset_window_sync_cache();
        cx.notify();
    }

    /// Previews the 10-player loading screen HUD overlay without requiring an active match.
    pub fn preview_loading_screen_mode(&mut self, cx: &mut Context<Self>) {
        self.preview_mode = Some(AppMode::InGameOverlay);
        self.preview_loading_screen = true;
        self.clear_offered_augment_state();
        self.reset_window_sync_cache();
        cx.notify();
    }

    /// Closes the diagnostics panel and syncs the tray check item.
    pub fn close_diagnostics(&mut self, cx: &mut Context<Self>) {
        self.diagnostics_open = false;
        self.preview_mode = None;
        self.preview_loading_screen = false;
        self.clear_offered_augment_state();
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
                info!("relaunching Hexrs with UAC elevation; shutting down current instance");
                self.vision.shutdown();
                self.core_handle.shutdown();
                cx.quit();
            }
            Err(err) => {
                warn!(error = %err, "failed to relaunch Hexrs as administrator");
            }
        }
    }

    fn reset_window_sync_cache(&mut self) {
        self.last_lobby_sync = None;
        self.last_bench_badge_sync = None;
        self.last_team_badge_sync = None;
        self.last_card_badge_sync = None;
        self.last_loading_sync = None;
        self.last_overlay_sync = None;
        self.last_diagnostics_sync = false;
        self.last_dormant_hidden = false;
    }

    /// Computes non-blocking bench, team, and pick-card strip badge states + deferred Win32 window commands.
    pub fn build_lobby_badge_sync(
        &mut self,
        bench_hwnd_raw: isize,
        team_hwnd_raw: isize,
        card_hwnd_raw: isize,
    ) -> (
        BenchBadgeState,
        Option<Win32WindowCommand>,
        TeamBadgeState,
        Option<Win32WindowCommand>,
        CardBadgeState,
        Option<Win32WindowCommand>,
    ) {
        let in_diagnostics = self.diagnostics_open && self.preview_mode.is_none();
        let effective_mode = self.preview_mode.unwrap_or(self.mode);
        let is_preview = self.preview_mode.is_some();

        let target_rect = if !in_diagnostics
            && effective_mode == AppMode::LobbyDocked
            && (is_preview || (self.overlay_visible && self.lobby_focused))
        {
            self.last_detected_lobby_rect
                .or_else(|| is_preview.then(win32::default_preview_lobby_rect))
        } else {
            None
        };

        let Some(rect) = target_rect else {
            let bench_cmd =
                if bench_hwnd_raw != 0 && self.last_bench_badge_sync != Some((None, 0)) {
                    self.last_bench_badge_sync = Some((None, 0));
                    Some(Win32WindowCommand {
                        hwnd_raw: bench_hwnd_raw,
                        action: Win32WindowAction::BenchBadgeStrip {
                            target_rect: None,
                            slot_count: 0,
                        },
                    })
                } else {
                    None
                };
            let team_cmd = if team_hwnd_raw != 0 && self.last_team_badge_sync != Some(None) {
                self.last_team_badge_sync = Some(None);
                Some(Win32WindowCommand {
                    hwnd_raw: team_hwnd_raw,
                    action: Win32WindowAction::TeamBadgeStrip { target_rect: None },
                })
            } else {
                None
            };
            let card_cmd = if card_hwnd_raw != 0 && self.last_card_badge_sync != Some((None, 0)) {
                self.last_card_badge_sync = Some((None, 0));
                Some(Win32WindowCommand {
                    hwnd_raw: card_hwnd_raw,
                    action: Win32WindowAction::CardBadgeStrip {
                        target_rect: None,
                        card_count: 0,
                    },
                })
            } else {
                None
            };
            return (
                BenchBadgeState::default(),
                bench_cmd,
                TeamBadgeState::default(),
                team_cmd,
                CardBadgeState::default(),
                card_cmd,
            );
        };

        let (bench_slots, team_slots, card_slots) = if let Some(cs) = self.champ_select.as_ref() {
            let bench = cs
                .bench_champions
                .iter()
                .take(10)
                .map(|&id| self.lookup_slot_badge(id))
                .collect();
            let rating_slots = self
                .team_ratings
                .as_ref()
                .map(|s| s.slots.as_slice())
                .unwrap_or(&[]);
            let team = cs
                .my_team
                .iter()
                .take(5)
                .enumerate()
                .map(|(idx, slot)| {
                    let id = if slot.champion_id > 0 {
                        slot.champion_id
                    } else {
                        slot.pick_intent
                    };
                    let hero = self.lookup_slot_badge(id);
                    let summoner = rating_slots
                        .iter()
                        .find(|r| r.cell_id == slot.cell_id)
                        .or_else(|| rating_slots.get(idx))
                        .map(summoner_summary_to_badge);
                    TeamSlotBadgeEntry { hero, summoner }
                })
                .collect();
            let cards = if cs.local_champion_id == 0 && (2..=3).contains(&cs.subset_champions.len())
            {
                order_subset_cards_for_display(&cs.subset_champions)
                    .into_iter()
                    .map(|id| self.lookup_card_slot_badge(id))
                    .collect()
            } else {
                Vec::new()
            };
            (bench, team, cards)
        } else if is_preview {
            const PREVIEW_BENCH_IDS: &[i32] =
                &[25, 142, 59, 78, 517, 154, 106, 157, 777, 145, 67, 875, 32, 54];
            const PREVIEW_TEAM_IDS: &[i32] =
                &[498, 23, 36, 526, 42, 157, 145, 875, 54, 32];
            const PREVIEW_CARD_IDS: &[i32] = &[157, 432, 145, 54];
            let bench = self.collect_preview_badges(PREVIEW_BENCH_IDS, 7);
            let team_heroes = self.collect_preview_badges(PREVIEW_TEAM_IDS, 5);
            let preview_ratings = self.preview_team_ratings();
            let team = (0..5)
                .map(|idx| TeamSlotBadgeEntry {
                    hero: team_heroes.get(idx).and_then(|x| x.clone()),
                    summoner: preview_ratings.slots.get(idx).map(summoner_summary_to_badge),
                })
                .collect();
            let cards = self.collect_preview_card_badges(PREVIEW_CARD_IDS, 2);
            (bench, team, cards)
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };

        let has_bench = bench_slots.iter().any(Option::is_some);
        let has_team = team_slots
            .iter()
            .any(|s| s.hero.is_some() || s.summoner.is_some());
        let has_cards = (2..=3).contains(&card_slots.len()) && card_slots.iter().any(Option::is_some);

        let bench_target = has_bench.then_some(rect);
        let bench_slot_count = if has_bench { bench_slots.len().min(10) } else { 0 };
        let bench_cmd = if bench_hwnd_raw != 0
            && self.last_bench_badge_sync != Some((bench_target, bench_slot_count))
        {
            self.last_bench_badge_sync = Some((bench_target, bench_slot_count));
            Some(Win32WindowCommand {
                hwnd_raw: bench_hwnd_raw,
                action: Win32WindowAction::BenchBadgeStrip {
                    target_rect: bench_target,
                    slot_count: bench_slot_count,
                },
            })
        } else {
            None
        };

        let team_target = has_team.then_some(rect);
        let team_cmd = if team_hwnd_raw != 0 && self.last_team_badge_sync != Some(team_target) {
            self.last_team_badge_sync = Some(team_target);
            Some(Win32WindowCommand {
                hwnd_raw: team_hwnd_raw,
                action: Win32WindowAction::TeamBadgeStrip {
                    target_rect: team_target,
                },
            })
        } else {
            None
        };

        let card_target = has_cards.then_some(rect);
        let card_count = if has_cards { card_slots.len().min(3) } else { 0 };
        let card_cmd = if card_hwnd_raw != 0
            && self.last_card_badge_sync != Some((card_target, card_count))
        {
            self.last_card_badge_sync = Some((card_target, card_count));
            Some(Win32WindowCommand {
                hwnd_raw: card_hwnd_raw,
                action: Win32WindowAction::CardBadgeStrip {
                    target_rect: card_target,
                    card_count,
                },
            })
        } else {
            None
        };

        let bench_scale = win32::window_scale_for_hwnd(bench_hwnd_raw, rect.scale()).max(0.5);
        let team_scale = win32::window_scale_for_hwnd(team_hwnd_raw, rect.scale()).max(0.5);
        let card_scale = win32::window_scale_for_hwnd(card_hwnd_raw, rect.scale()).max(0.5);

        let bench_state = BenchBadgeState {
            visible: has_bench,
            slots: bench_slots,
            client_width_dip: rect.width() as f32 / bench_scale,
            client_height_dip: rect.height() as f32 / bench_scale,
        };
        let team_state = TeamBadgeState {
            visible: has_team,
            slots: team_slots,
            strip_width_dip: win32::compute_team_strip_width_dip(rect.width() as f32 / team_scale),
            client_height_dip: rect.height() as f32 / team_scale,
        };
        let card_state = CardBadgeState {
            visible: has_cards,
            cards: card_slots,
            client_width_dip: rect.width() as f32 / card_scale,
            client_height_dip: rect.height() as f32 / card_scale,
        };

        (
            bench_state,
            bench_cmd,
            team_state,
            team_cmd,
            card_state,
            card_cmd,
        )
    }

    fn lookup_loading_hero(&self, hero_id: i32) -> Option<LoadingCardHeroData> {
        if hero_id <= 0 {
            return None;
        }
        let stats = self.meta.as_ref()?.get_hero_stats(hero_id)?;
        Some(LoadingCardHeroData {
            tier: stats.tier,
            win_rate: stats.win_rate,
            name_zh: stats.name_zh,
        })
    }

    fn preview_loading_screen_snapshot(&self) -> LoadingScreenSnapshot {
        const BLUE_IDS: [i32; 5] = [157, 498, 23, 36, 526];
        const RED_IDS: [i32; 5] = [145, 777, 875, 54, 42];

        let team_one: Vec<LoadingPlayerSlot> = BLUE_IDS
            .into_iter()
            .enumerate()
            .map(|(slot_index, champion_id)| LoadingPlayerSlot {
                slot_index,
                champion_id,
                puuid: format!("preview-blue-{slot_index}"),
            })
            .collect();
        let team_two: Vec<LoadingPlayerSlot> = RED_IDS
            .into_iter()
            .enumerate()
            .map(|(slot_index, champion_id)| LoadingPlayerSlot {
                slot_index,
                champion_id,
                puuid: format!("preview-red-{slot_index}"),
            })
            .collect();

        let enemy_specs = [
            (0, SummonerRatingStatus::Ready, 81, 4, Some(1), true),
            (1, SummonerRatingStatus::Ready, 68, 0, Some(1), false),
            (2, SummonerRatingStatus::Ready, 55, -3, None, false),
            (3, SummonerRatingStatus::Ready, 43, -4, None, false),
            (4, SummonerRatingStatus::InsufficientSamples, 0, 0, None, false),
        ];
        let enemy_slots = enemy_specs
            .into_iter()
            .map(
                |(cell_id, status, score, streak, premade_group_id, is_premade_last_mvp)| {
                    let (tier_index, title) = self.rating_titles.resolve(status, score);
                    SummonerRatingSummary {
                        cell_id,
                        status,
                        score,
                        tier_index,
                        title,
                        games_count: if status == SummonerRatingStatus::Ready {
                            10
                        } else {
                            2
                        },
                        win_rate: 51.0,
                        avg_kda: 2.9,
                        streak,
                        premade_group_id,
                        is_premade_last_mvp,
                    }
                },
            )
            .collect();

        LoadingScreenSnapshot {
            game_id: 0,
            game_mode: "KIWI".to_owned(),
            queue_id: Some(2400),
            local_side: LoadingTeamSide::Blue,
            team_one,
            team_two,
            team_one_ratings: self.preview_team_ratings(),
            team_two_ratings: TeamRatingsSnapshot {
                slots: enemy_slots,
                premade_groups: vec![PremadeGroupSummary {
                    group_id: 1,
                    slot_indices: vec![0, 1],
                    shared_games: 3,
                    last_mvp_slot_index: Some(0),
                }],
            },
        }
    }

    fn map_loading_team_slots(
        &self,
        players: &[LoadingPlayerSlot],
        ratings: &TeamRatingsSnapshot,
    ) -> Vec<LoadingCardSlotData> {
        players
            .iter()
            .take(5)
            .enumerate()
            .map(|(idx, player)| {
                let hero = self.lookup_loading_hero(player.champion_id);
                let summoner = ratings
                    .slots
                    .get(idx)
                    .map(summoner_summary_to_badge)
                    .unwrap_or_else(|| {
                        let (tier_index, title) =
                            self.rating_titles.resolve(SummonerRatingStatus::Loading, 0);
                        SummonerBadgeData {
                            status: SummonerRatingStatus::Loading,
                            score: 0,
                            tier_index,
                            title,
                            streak: 0,
                            premade_group_id: None,
                            is_premade_last_mvp: false,
                        }
                    });
                LoadingCardSlotData { hero, summoner }
            })
            .collect()
    }

    /// Computes the 10-player loading screen overlay state and its deferred Win32 window command.
    pub fn build_loading_overlay_sync(
        &mut self,
        loading_hwnd_raw: isize,
    ) -> (LoadingOverlayState, Option<Win32WindowCommand>) {
        let in_diagnostics = self.diagnostics_open && self.preview_mode.is_none();
        let effective_mode = self.preview_mode.unwrap_or(self.mode);
        let should_show = !in_diagnostics
            && (self.preview_loading_screen
                || (effective_mode == AppMode::InGameOverlay
                    && self.overlay_visible
                    && self.game_focused
                    && self.loading_screen.is_some()));

        let target_rect = if should_show {
            self.last_detected_game_rect
                .or_else(|| self.preview_loading_screen.then(win32::default_preview_game_rect))
        } else {
            None
        };

        let Some(rect) = target_rect else {
            let cmd = if loading_hwnd_raw != 0 && self.last_loading_sync != Some((None, false)) {
                self.last_loading_sync = Some((None, false));
                Some(Win32WindowCommand {
                    hwnd_raw: loading_hwnd_raw,
                    action: Win32WindowAction::LoadingScreenOverlay {
                        target_rect: None,
                        visible: false,
                    },
                })
            } else {
                None
            };
            return (LoadingOverlayState::default(), cmd);
        };

        let preview_snap = (self.preview_loading_screen && self.loading_screen.is_none())
            .then(|| self.preview_loading_screen_snapshot());
        let Some(snap) = self.loading_screen.as_ref().or(preview_snap.as_ref()) else {
            let cmd = if loading_hwnd_raw != 0 && self.last_loading_sync != Some((None, false)) {
                self.last_loading_sync = Some((None, false));
                Some(Win32WindowCommand {
                    hwnd_raw: loading_hwnd_raw,
                    action: Win32WindowAction::LoadingScreenOverlay {
                        target_rect: None,
                        visible: false,
                    },
                })
            } else {
                None
            };
            return (LoadingOverlayState::default(), cmd);
        };

        let team_one = self.map_loading_team_slots(&snap.team_one, &snap.team_one_ratings);
        let team_two = self.map_loading_team_slots(&snap.team_two, &snap.team_two_ratings);
        let team_one_summary =
            compute_team_loading_summary(&snap.team_one_ratings, &self.rating_titles);
        let team_two_summary =
            compute_team_loading_summary(&snap.team_two_ratings, &self.rating_titles);

        let scale = win32::window_scale_for_hwnd(loading_hwnd_raw, rect.scale()).max(0.5);
        let state = LoadingOverlayState {
            visible: true,
            local_side: snap.local_side,
            team_one,
            team_two,
            team_one_summary,
            team_two_summary,
            window_width_dip: rect.width() as f32 / scale,
            window_height_dip: rect.height() as f32 / scale,
        };

        let cmd = if loading_hwnd_raw != 0 && self.last_loading_sync != Some((Some(rect), true)) {
            self.last_loading_sync = Some((Some(rect), true));
            Some(Win32WindowCommand {
                hwnd_raw: loading_hwnd_raw,
                action: Win32WindowAction::LoadingScreenOverlay {
                    target_rect: Some(rect),
                    visible: true,
                },
            })
        } else {
            None
        };

        (state, cmd)
    }

    fn lookup_card_slot_badge(&self, hero_id: i32) -> Option<CardSlotBadgeData> {
        if hero_id <= 0 {
            return None;
        }
        let stats = self.meta.as_ref()?.get_hero_stats(hero_id)?;
        Some(CardSlotBadgeData {
            tier: stats.tier,
            win_rate: stats.win_rate,
            name_zh: stats.name_zh,
        })
    }

    fn collect_preview_card_badges(
        &self,
        candidates: &[i32],
        max_cards: usize,
    ) -> Vec<Option<CardSlotBadgeData>> {
        candidates
            .iter()
            .filter_map(|&id| self.lookup_card_slot_badge(id))
            .take(max_cards)
            .map(Some)
            .collect()
    }

    fn lookup_slot_badge(&self, hero_id: i32) -> Option<SlotBadgeData> {
        if hero_id <= 0 {
            return None;
        }
        let stats = self.meta.as_ref()?.get_hero_stats(hero_id)?;
        Some(SlotBadgeData {
            tier: stats.tier,
            win_rate: stats.win_rate,
        })
    }

    fn collect_preview_badges(&self, candidates: &[i32], max_slots: usize) -> Vec<Option<SlotBadgeData>> {
        candidates
            .iter()
            .filter_map(|&id| self.lookup_slot_badge(id))
            .take(max_slots)
            .map(Some)
            .collect()
    }

    /// Builds a 5-slot `2+3` preview snapshot for demonstrating rating titles, streaks, and premade MVPs.
    pub fn preview_team_ratings(&self) -> TeamRatingsSnapshot {
        let specs = [
            (0, SummonerRatingStatus::Ready, 86, 5, Some(1), true),
            (1, SummonerRatingStatus::Ready, 74, 0, Some(1), false),
            (2, SummonerRatingStatus::Ready, 62, -3, Some(2), false),
            (3, SummonerRatingStatus::Ready, 51, 0, Some(2), true),
            (4, SummonerRatingStatus::Private, 0, 0, Some(2), false),
        ];

        let slots = specs
            .into_iter()
            .map(
                |(cell_id, status, score, streak, premade_group_id, is_premade_last_mvp)| {
                    let (tier_index, title) = self.rating_titles.resolve(status, score);
                    SummonerRatingSummary {
                        cell_id,
                        status,
                        score,
                        tier_index,
                        title,
                        games_count: if status == SummonerRatingStatus::Ready {
                            12
                        } else {
                            0
                        },
                        win_rate: 55.0,
                        avg_kda: 3.4,
                        streak,
                        premade_group_id,
                        is_premade_last_mvp,
                    }
                },
            )
            .collect();

        TeamRatingsSnapshot {
            slots,
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
        }
    }

    fn refresh_rating_titles_if_changed(&mut self) {
        let path = rating_titles_path();
        let next_mtime = read_file_mtime(&path);
        if next_mtime != self.rating_titles_mtime {
            self.rating_titles_mtime = next_mtime;
            self.rating_titles = RatingTitlesConfig::load_or_init(&path);
            if let Some(snapshot) = self.team_ratings.as_mut() {
                for slot in &mut snapshot.slots {
                    let (tier_index, title) = self.rating_titles.resolve(slot.status, slot.score);
                    slot.tier_index = tier_index;
                    slot.title = title;
                }
            }
            if let Some(loading) = self.loading_screen.as_mut() {
                for slot in &mut loading.team_one_ratings.slots {
                    let (tier_index, title) = self.rating_titles.resolve(slot.status, slot.score);
                    slot.tier_index = tier_index;
                    slot.title = title;
                }
                for slot in &mut loading.team_two_ratings.slots {
                    let (tier_index, title) = self.rating_titles.resolve(slot.status, slot.score);
                    slot.tier_index = tier_index;
                    slot.title = title;
                }
            }
        }
    }

    fn clear_offered_augment_state(&mut self) {
        self.offered_augments.clear();
        self.offered_augment_layout = None;
        self.last_offer_capture_ms = None;
    }

    fn clear_in_game_hud_state(&mut self) {
        self.owned_augment_ids.clear();
        self.clear_offered_augment_state();
        self.loading_screen = None;
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
                    self.team_ratings = None;
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
                    self.team_ratings = None;
                    self.bench_highlights.clear();
                    self.team_recs = TeamRecommendations::default();
                    self.current_hero_stats = None;
                    self.expanded_hero_id = None;
                    self.selected_preview = None;
                    self.last_auto_expanded_hero_id = None;
                }
            }
            AppEvent::ChampSelectUpdated(state) => {
                self.refresh_rating_titles_if_changed();
                self.refresh_champ_select_data(&state);
                self.champ_select = Some(state);
            }
            AppEvent::TeamRatingsUpdated(mut snapshot) => {
                self.refresh_rating_titles_if_changed();
                for slot in &mut snapshot.slots {
                    let (tier_index, title) = self.rating_titles.resolve(slot.status, slot.score);
                    slot.tier_index = tier_index;
                    slot.title = title;
                }
                self.team_ratings = Some(snapshot);
            }
            AppEvent::LoadingScreenUpdated(mut snap) => {
                self.refresh_rating_titles_if_changed();
                for slot in &mut snap.team_one_ratings.slots {
                    let (tier_index, title) = self.rating_titles.resolve(slot.status, slot.score);
                    slot.tier_index = tier_index;
                    slot.title = title;
                }
                for slot in &mut snap.team_two_ratings.slots {
                    let (tier_index, title) = self.rating_titles.resolve(slot.status, slot.score);
                    slot.tier_index = tier_index;
                    slot.title = title;
                }
                self.loading_screen = Some(snap);
                self.reset_window_sync_cache();
            }
            AppEvent::LoadingScreenCleared => {
                if self.loading_screen.take().is_some() {
                    self.reset_window_sync_cache();
                }
            }
            AppEvent::LiveGameUpdated(live) => {
                if live.game_time_secs > 0.0 && self.loading_screen.take().is_some() {
                    self.reset_window_sync_cache();
                }
                self.refresh_live_game_data(&live);
                self.live_game = Some(live);
            }
            AppEvent::AugmentOfferDetected {
                candidates,
                capture_ms,
                layout,
            } => {
                info!(
                    ?candidates,
                    capture_ms,
                    ?layout,
                    active_hero_id = ?self.active_hero_id,
                    active_champ_select_hero_id = ?self.active_champ_select_hero_id,
                    game_focused = self.game_focused,
                    overlay_visible = self.overlay_visible,
                    "augment offer detected by hex-vision"
                );
                self.evaluate_detected_offer(&candidates, capture_ms, layout);
            }
            AppEvent::AugmentOfferCleared => {
                if !self.offered_augments.is_empty() || self.offered_augment_layout.is_some() {
                    info!("clearing augment offer strip");
                    self.clear_offered_augment_state();
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

    fn evaluate_detected_offer(
        &mut self,
        candidates: &[i32],
        capture_ms: u64,
        layout: Option<AugmentOfferLayout>,
    ) {
        let Some(meta) = self.meta.as_ref() else {
            warn!(
                ?candidates,
                "cannot evaluate augment offer: MetaEngine is not loaded"
            );
            return;
        };
        let hero_id = self
            .active_hero_id
            .or(self.active_champ_select_hero_id)
            .unwrap_or(0);
        if hero_id == 0 {
            warn!(
                ?candidates,
                live_champ = self
                    .live_game
                    .as_ref()
                    .map(|g| g.active_champion_name.as_str())
                    .unwrap_or(""),
                "evaluating augment offer with unresolved hero_id=0; using global averages"
            );
        }

        let mut slots = Vec::with_capacity(candidates.len().min(4));
        let mut matched_any = false;

        for &aug_id in candidates.iter().take(4) {
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
                warn!(
                    hero_id,
                    aug_id,
                    "augment_id missing from meta.db augment_meta; using unrecognized placeholder"
                );
            }
            slots.push(placeholder_augment_eval());
        }

        let data_version = meta.data_version();
        let summary: Vec<String> = slots
            .iter()
            .map(|eval| {
                format!(
                    "{}:{}({},{:.1}%,{:.1}%,n={},low={})",
                    eval.augment_id,
                    eval.name_zh,
                    eval.tier.label(),
                    eval.win_rate,
                    eval.pick_rate,
                    eval.sample_size,
                    eval.low_sample
                )
            })
            .collect();

        if !matched_any {
            warn!(
                hero_id,
                ?candidates,
                data_version = %data_version,
                "augment offer dropped: zero candidates resolved"
            );
            return;
        }

        if slots.iter().any(|eval| eval.augment_id == 0 || eval.low_sample) {
            warn!(
                hero_id,
                data_version = %data_version,
                slots = ?summary,
                "evaluated augment offer with unrecognized or low-sample slots"
            );
        } else {
            info!(
                hero_id,
                data_version = %data_version,
                slots = ?summary,
                "evaluated augment offer"
            );
        }

        self.offered_augments = slots;
        self.offered_augment_layout = layout;
        self.last_offer_capture_ms = Some(capture_ms);
        self.reset_window_sync_cache();
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
            for item in plan.items.iter().take(6) {
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

    fn sync_window_mode(&mut self, window: &Window) -> Option<Win32WindowCommand> {
        let hwnd_raw = match win32::window_hwnd_raw(window) {
            Ok(raw) => raw,
            Err(err) => {
                warn!(error = %err, "failed to extract Win32 HWND for window sync");
                return None;
            }
        };

        if self.diagnostics_open && self.preview_mode.is_none() {
            let lobby_r = win32::find_league_lobby_window(self.core_handle.lcu_pid());
            let game_r = win32::find_league_game_window(self.core_handle.game_pid());
            self.last_detected_lobby_rect = lobby_r;
            self.last_detected_game_rect = game_r;
            self.lobby_focused = win32::is_lobby_window_foreground(
                self.core_handle.lcu_pid(),
                lobby_r.map(|r| r.hwnd_raw).unwrap_or(0),
                hwnd_raw,
            );
            self.game_focused = win32::is_game_window_foreground(
                self.core_handle.game_pid(),
                game_r.map(|r| r.hwnd_raw).unwrap_or(0),
                0,
            );
            if !self.last_diagnostics_sync {
                self.last_diagnostics_sync = true;
                self.last_dormant_hidden = false;
                return Some(Win32WindowCommand {
                    hwnd_raw,
                    action: Win32WindowAction::Diagnostics,
                });
            }
            return None;
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

                self.lobby_focused = win32::is_lobby_window_foreground(
                    self.core_handle.lcu_pid(),
                    detected_rect.map(|r| r.hwnd_raw).unwrap_or(0),
                    hwnd_raw,
                );
                let visible = is_preview || (self.overlay_visible && self.lobby_focused);
                let target_rect = if is_preview {
                    detected_rect.or_else(|| Some(win32::default_preview_lobby_rect()))
                } else if visible {
                    detected_rect
                } else {
                    None
                };

                let state = (target_rect, visible);
                if self.last_lobby_sync != Some(state) {
                    self.last_lobby_sync = Some(state);
                    return Some(Win32WindowCommand {
                        hwnd_raw,
                        action: Win32WindowAction::LobbyDock(target_rect),
                    });
                }
                None
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

                self.game_focused = win32::is_game_window_foreground(
                    self.core_handle.game_pid(),
                    detected_rect.map(|r| r.hwnd_raw).unwrap_or(0),
                    hwnd_raw,
                );
                let base_visible = is_preview || (self.overlay_visible && self.game_focused);
                let suppress_corner_for_loading = self.offered_augments.is_empty()
                    && (self.loading_screen.is_some() || self.preview_loading_screen);
                let visible = base_visible && !suppress_corner_for_loading;
                let target_rect = if is_preview {
                    if visible {
                        detected_rect.or_else(|| Some(win32::default_preview_game_rect()))
                    } else {
                        None
                    }
                } else if visible {
                    detected_rect
                } else {
                    None
                };
                let augment_slot_count = self.offered_augments.len().min(4);
                let offer_layout = if augment_slot_count > 0 {
                    self.offered_augment_layout.as_ref()
                } else {
                    None
                };
                let quantized_layout = quantize_offer_layout(offer_layout);
                if augment_slot_count > 0 {
                    self.augment_bar_gap_dip = win32::compute_augment_bar_gap_dip(
                        target_rect,
                        augment_slot_count,
                        offer_layout,
                    );
                }

                let state = (
                    target_rect,
                    self.overlay_corner,
                    visible,
                    self.in_game_hud_collapsed,
                    augment_slot_count,
                    quantized_layout,
                );
                if self.last_overlay_sync != Some(state) {
                    self.last_overlay_sync = Some(state);
                    if augment_slot_count > 0 {
                        if visible && target_rect.is_some() {
                            info!(
                                augment_slot_count,
                                visible,
                                overlay_visible = self.overlay_visible,
                                game_focused = self.game_focused,
                                has_target_rect = target_rect.is_some(),
                                is_preview,
                                "syncing in-game augment bar window"
                            );
                        } else {
                            warn!(
                                augment_slot_count,
                                visible,
                                overlay_visible = self.overlay_visible,
                                game_focused = self.game_focused,
                                has_target_rect = target_rect.is_some(),
                                is_preview,
                                "in-game augment bar not shown during window sync"
                            );
                        }
                    }
                    let action = if augment_slot_count > 0 {
                        Win32WindowAction::InGameAugmentBar {
                            target_rect,
                            visible,
                            slot_count: augment_slot_count,
                            layout: offer_layout.cloned(),
                        }
                    } else {
                        Win32WindowAction::InGameOverlay {
                            target_rect,
                            corner: self.overlay_corner,
                            visible,
                            collapsed: self.in_game_hud_collapsed,
                        }
                    };
                    return Some(Win32WindowCommand { hwnd_raw, action });
                }
                None
            }
            AppMode::Dormant => {
                if !self.last_dormant_hidden {
                    self.last_dormant_hidden = true;
                    return Some(Win32WindowCommand {
                        hwnd_raw,
                        action: Win32WindowAction::Hide,
                    });
                }
                None
            }
        }
    }
}

fn quantize_offer_layout(layout: Option<&AugmentOfferLayout>) -> Option<(usize, i32, i32, i32)> {
    layout.and_then(|l| {
        let first = *l.slot_centers_x.first()?;
        let last = *l.slot_centers_x.last()?;
        Some((
            l.slot_centers_x.len(),
            (first * 100.0).round() as i32,
            (last * 100.0).round() as i32,
            (l.title_y * 100.0).round() as i32,
        ))
    })
}

fn summoner_summary_to_badge(summary: &SummonerRatingSummary) -> SummonerBadgeData {
    SummonerBadgeData {
        status: summary.status,
        score: summary.score,
        tier_index: summary.tier_index,
        title: summary.title.clone(),
        streak: summary.streak,
        premade_group_id: summary.premade_group_id,
        is_premade_last_mvp: summary.is_premade_last_mvp,
    }
}

fn rating_titles_path() -> PathBuf {
    hex_data::data_dir().join("rating_titles.json")
}

fn read_file_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
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

impl Render for HexrsApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.diagnostics_open && self.preview_mode.is_none() {
            return render_diagnostics_panel(self, cx).into_any_element();
        }

        match self.preview_mode.unwrap_or(self.mode) {
            AppMode::LobbyDocked => lobby_view::render_lobby_sidebar(self, cx).into_any_element(),
            AppMode::InGameOverlay => overlay_view::render_in_game_overlay(self, cx).into_any_element(),
            AppMode::Dormant => div()
                .size_full()
                .bg(theme::transparent())
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
        sample_size: 0,
        low_sample: false,
        synergy_alert: None,
    }
}

fn render_diagnostics_panel(app: &HexrsApp, cx: &mut Context<HexrsApp>) -> Div {
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
                        .child("Hexrs · 运行诊断面板"),
                )
                .child(
                    div()
                        .id("close-diagnostics-btn")
                        .cursor_pointer()
                        .px_2()
                        .py(px(2.0))
                        .rounded(px(3.0))
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
                                        .rounded(px(3.0))
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
                                    .rounded(px(3.0))
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
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme::cyan())
                                .child("Win32 目标窗口定位"),
                        )
                        .child(diag_row("大厅窗口 (RCLIENT)", lobby_rect_str))
                        .child(diag_row("局内窗口 (RiotWindowClass)", game_rect_str))
                        .child(diag_row(
                            "客户端焦点",
                            format!(
                                "大厅: {} · 局内: {}",
                                if app.lobby_focused { "聚焦" } else { "未聚焦" },
                                if app.game_focused { "聚焦" } else { "未聚焦" }
                            ),
                        )),
                )
                .child(
                    div()
                        .p_2()
                        .rounded(px(4.0))
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
                                        .rounded(px(3.0))
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
                                        .rounded(px(3.0))
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
                                        .rounded(px(3.0))
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
                                .id("preview-loading-screen-btn")
                                .cursor_pointer()
                                .px_2()
                                .py_1()
                                .rounded(px(3.0))
                                .bg(theme::cyan_dim())
                                .border_1()
                                .border_color(theme::border_cyan())
                                .hover(|s| s.bg(theme::card_elevated_bg()))
                                .flex()
                                .justify_center()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::cyan())
                                .child("预览加载页10人")
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.preview_loading_screen_mode(cx);
                                })),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child("提示：右上角出装窗可点击标题栏「收起/展开」，三选一与加载页保持穿透；预览时右键托盘“显示诊断面板”即可切回。"),
                        ),
                )
                .child(
                    div()
                        .p_2()
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

fn order_subset_cards_for_display(ids: &[i32]) -> Vec<i32> {
    if ids.len() == 3 {
        vec![ids[0], ids[2], ids[1]]
    } else {
        ids.iter().copied().take(3).collect()
    }
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

#[cfg(test)]
mod tests {
    use super::order_subset_cards_for_display;

    #[test]
    fn subset_cards_visual_order_places_third_blessed_card_in_center() {
        assert_eq!(order_subset_cards_for_display(&[10, 20]), vec![10, 20]);
        assert_eq!(
            order_subset_cards_for_display(&[14, 517, 497]),
            vec![14, 497, 517]
        );
    }
}
