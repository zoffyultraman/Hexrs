//! Root GPUI application entity, event-loop polling, WGC vision wiring, icon cache, diagnostics, and Win32 orchestration.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crossbeam_channel::{unbounded, Receiver};
use gpui::{div, prelude::*, px, Context, Div, FontWeight, Render, Rgba, Window};
use hex_core::{
    AppEvent, AppMode, AugmentOfferLayout, ChampSelectState, CoreDiagnostics, CoreHandle,
    HeroTier, LiveGameState, LivePlayerEntry, LoadingPlayerSlot, LoadingScreenSnapshot,
    LoadingTeamSide, PremadeGroupSummary, RatingTitlesConfig, RoiManifest, SummonerRatingStatus,
    SummonerRatingSummary, TeamRatingsSnapshot,
};
use hex_data::{
    spawn_app_update_check, spawn_background_update, AppUpdateConfig, AppUpdateOutcome,
    AugmentEvaluation, BenchHighlight, DynamicBuildPlan, EogReporter, HeroPreview, HeroStats,
    IconCache, MetaEngine, TeamRecommendations, UpdateOutcome, UpdaterConfig,
    DEFAULT_RELEASES_PAGE_URL,
};
use hex_vision::{
    LoadingAlignmentTargets, LoadingSlotTarget, SharedRoiManifest, TabAlignmentTargets,
    VisionController, VisionGameContext,
};
use tracing::{debug, info, warn};

use crate::loading_overlay_view::{
    compute_team_loading_summary, LoadingCardHeroData, LoadingCardSlotData, LoadingOverlayState,
    TabScoreboardSideState,
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
use crate::win32::{
    self, OverlayCorner, TabScoreboardSide, Win32WindowAction, Win32WindowCommand, WindowRectInfo,
};

/// Compile-time injected Hexrs application version tag (set by `build.rs` / CI).
pub const APP_VERSION: &str = env!("HEXRS_APP_VERSION");
/// Compile-time injected short Git commit SHA (set by `build.rs` / CI).
pub const APP_GIT_SHA: &str = env!("HEXRS_GIT_SHA");

const BENCH_HIGHLIGHT_LIMIT: usize = 12;
const TEAM_RECOMMENDATION_LIMIT: usize = 5;
const PREVIEW_YASUO_HERO_ID: i32 = 157;

/// UI-facing client application release check status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppUpdateUiState {
    Checking,
    UpToDate {
        latest_version: String,
        release_url: String,
    },
    UpdateAvailable {
        latest_version: String,
        release_url: String,
        download_url: Option<String>,
        is_prerelease: bool,
    },
    Failed(String),
}

/// Transient in-memory player key `(champion_id, normalized_display_name)` for stable Tab row locking.
/// Never log, persist to disk, or emit to telemetry.
type TabRosterKey = (i32, String);

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
    pub last_match_loading_screen: Option<LoadingScreenSnapshot>,
    pub tab_scoreboard_held: bool,
    pub preview_loading_screen: bool,
    pub preview_tab_scoreboard: bool,
    loading_card_order: Option<(u64, Vec<usize>, Vec<usize>)>,
    tab_visual_keys: Option<(u64, Vec<TabRosterKey>, Vec<TabRosterKey>)>,
    last_tab_target_keys: Option<(u64, Vec<TabRosterKey>, Vec<TabRosterKey>)>,
    meta: Option<MetaEngine>,
    icon_cache: IconCache,
    icon_ready_rx: Receiver<()>,
    event_rx: Receiver<AppEvent>,
    vision_rx: Receiver<AppEvent>,
    updater_config: UpdaterConfig,
    update_tx: crossbeam_channel::Sender<anyhow::Result<UpdateOutcome>>,
    update_rx: Receiver<anyhow::Result<UpdateOutcome>>,
    update_in_flight: bool,
    pub app_update_state: AppUpdateUiState,
    app_update_config: AppUpdateConfig,
    app_update_tx: crossbeam_channel::Sender<anyhow::Result<AppUpdateOutcome>>,
    app_update_rx: Receiver<anyhow::Result<AppUpdateOutcome>>,
    app_update_in_flight: bool,
    reporter: EogReporter,
    core_handle: CoreHandle,
    vision: VisionController,
    tray: Option<TrayController>,
    last_lobby_sync: Option<(Option<WindowRectInfo>, bool)>,
    last_bench_badge_sync: Option<(Option<WindowRectInfo>, usize)>,
    last_team_badge_sync: Option<Option<WindowRectInfo>>,
    last_card_badge_sync: Option<(Option<WindowRectInfo>, usize)>,
    last_loading_sync: Option<(Option<WindowRectInfo>, bool)>,
    last_tab_left_sync: Option<(Option<WindowRectInfo>, bool, bool)>,
    last_tab_right_sync: Option<(Option<WindowRectInfo>, bool, bool)>,
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
        let app_update_config = AppUpdateConfig::from_env();
        let (app_update_tx, app_update_rx) = unbounded::<anyhow::Result<AppUpdateOutcome>>();
        spawn_app_update_check(
            app_update_config.clone(),
            APP_VERSION.to_owned(),
            app_update_tx.clone(),
        );
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
            last_match_loading_screen: None,
            tab_scoreboard_held: false,
            preview_loading_screen: false,
            preview_tab_scoreboard: false,
            loading_card_order: None,
            tab_visual_keys: None,
            last_tab_target_keys: None,
            meta,
            icon_cache,
            icon_ready_rx,
            event_rx,
            vision_rx,
            updater_config,
            update_tx,
            update_rx,
            update_in_flight: true,
            app_update_state: AppUpdateUiState::Checking,
            app_update_config,
            app_update_tx,
            app_update_rx,
            app_update_in_flight: true,
            reporter,
            core_handle,
            vision,
            tray,
            last_lobby_sync: None,
            last_bench_badge_sync: None,
            last_team_badge_sync: None,
            last_card_badge_sync: None,
            last_loading_sync: None,
            last_tab_left_sync: None,
            last_tab_right_sync: None,
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

        while let Ok(outcome) = self.app_update_rx.try_recv() {
            self.app_update_in_flight = false;
            match outcome {
                Ok(AppUpdateOutcome::UpdateAvailable {
                    latest_version,
                    release_url,
                    download_url,
                    is_prerelease,
                    ..
                }) => {
                    info!(
                        current_version = APP_VERSION,
                        %latest_version,
                        is_prerelease,
                        "newer Hexrs client release available"
                    );
                    if let Some(ref tray_ctrl) = self.tray {
                        tray_ctrl.update_app_version_status(
                            &format!("发现新版本: {latest_version} (点击下载)"),
                            true,
                        );
                    }
                    self.app_update_state = AppUpdateUiState::UpdateAvailable {
                        latest_version,
                        release_url,
                        download_url,
                        is_prerelease,
                    };
                    dirty = true;
                }
                Ok(AppUpdateOutcome::UpToDate {
                    latest_version,
                    release_url,
                    ..
                }) => {
                    debug!(
                        current_version = APP_VERSION,
                        %latest_version,
                        "Hexrs client is up to date"
                    );
                    if let Some(ref tray_ctrl) = self.tray {
                        tray_ctrl.update_app_version_status(
                            &format!("版本: {APP_VERSION} (已是最新)"),
                            false,
                        );
                    }
                    self.app_update_state = AppUpdateUiState::UpToDate {
                        latest_version,
                        release_url,
                    };
                    dirty = true;
                }
                Err(err) => {
                    debug!(error = %err, "silent app version update check failed");
                    if let Some(ref tray_ctrl) = self.tray {
                        tray_ctrl.update_app_version_status(&format!("版本: {APP_VERSION}"), false);
                    }
                    self.app_update_state = AppUpdateUiState::Failed(err.to_string());
                    dirty = true;
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
                        if self.preview_mode.is_some()
                            || self.preview_loading_screen
                            || self.preview_tab_scoreboard
                        {
                            self.preview_mode = None;
                            self.preview_loading_screen = false;
                            self.preview_tab_scoreboard = false;
                            self.clear_offered_augment_state();
                            self.diagnostics_open = true;
                            if let Some(ref tray_ctrl) = self.tray {
                                tray_ctrl.set_diagnostics_checked(true);
                            }
                        } else {
                            self.preview_loading_screen = false;
                            self.preview_tab_scoreboard = false;
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
                    TrayAction::CheckAppUpdate => {
                        self.trigger_app_update_check(cx);
                        dirty = true;
                    }
                    TrayAction::OpenReleasePage => {
                        self.open_app_release_page();
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

        let next_tab_held = self.mode == AppMode::InGameOverlay
            && self.game_focused
            && self.overlay_visible
            && self.loading_screen.is_none()
            && self.offered_augments.is_empty()
            && self.has_tab_roster_data()
            && win32::is_tab_scoreboard_hotkey_pressed();
        if next_tab_held != self.tab_scoreboard_held {
            self.tab_scoreboard_held = next_tab_held;
            dirty = true;
        }
        if self.tab_scoreboard_held {
            let (game_id, left_entries, right_entries) =
                self.build_baseline_tab_entries(self.last_match_loading_screen.as_ref());
            let left_keys: Vec<TabRosterKey> =
                left_entries.iter().map(|(k, _, _)| k.clone()).collect();
            let right_keys: Vec<TabRosterKey> =
                right_entries.iter().map(|(k, _, _)| k.clone()).collect();
            let targets = TabAlignmentTargets {
                game_id,
                left_names: left_entries.into_iter().map(|(_, name, _)| name).collect(),
                right_names: right_entries.into_iter().map(|(_, name, _)| name).collect(),
            };
            self.last_tab_target_keys = Some((game_id, left_keys, right_keys));
            self.vision.set_tab_targets(Some(targets));
        } else {
            self.vision.set_tab_targets(None);
        }

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
        self.preview_tab_scoreboard = false;
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
        self.preview_tab_scoreboard = false;
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
        self.preview_tab_scoreboard = false;
        self.clear_offered_augment_state();
        self.reset_window_sync_cache();
        cx.notify();
    }

    /// Previews the dual hold-Tab side scoreboard panels without requiring an active match.
    pub fn preview_tab_scoreboard_mode(&mut self, cx: &mut Context<Self>) {
        self.preview_mode = Some(AppMode::InGameOverlay);
        self.preview_loading_screen = false;
        self.preview_tab_scoreboard = true;
        self.clear_offered_augment_state();
        if self.live_game.is_none() {
            self.populate_hero_hud_state(Some(PREVIEW_YASUO_HERO_ID), None, &[]);
        }
        self.reset_window_sync_cache();
        cx.notify();
    }

    /// Closes the diagnostics panel and syncs the tray check item.
    pub fn close_diagnostics(&mut self, cx: &mut Context<Self>) {
        self.diagnostics_open = false;
        self.preview_mode = None;
        self.preview_loading_screen = false;
        self.preview_tab_scoreboard = false;
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

    /// Triggers a non-blocking GitHub Releases check for a newer Hexrs client build.
    pub fn trigger_app_update_check(&mut self, cx: &mut Context<Self>) {
        if self.app_update_in_flight {
            return;
        }
        self.app_update_in_flight = true;
        self.app_update_state = AppUpdateUiState::Checking;
        if let Some(ref tray_ctrl) = self.tray {
            tray_ctrl.update_app_version_status(&format!("版本: {APP_VERSION} (检查中…)"), false);
        }
        spawn_app_update_check(
            self.app_update_config.clone(),
            APP_VERSION.to_owned(),
            self.app_update_tx.clone(),
        );
        info!(
            current_version = APP_VERSION,
            "manual app version update check started"
        );
        cx.notify();
    }

    /// Opens the latest GitHub Release page (or fallback releases page) in the default browser.
    pub fn open_app_release_page(&self) {
        let url = resolve_release_open_url(&self.app_update_state);
        if let Err(err) = win32::open_url(url) {
            warn!(error = %err, "failed to open Hexrs release page in browser");
        }
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
        self.last_tab_left_sync = None;
        self.last_tab_right_sync = None;
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
                display_name: String::new(),
            })
            .collect();
        let team_two: Vec<LoadingPlayerSlot> = RED_IDS
            .into_iter()
            .enumerate()
            .map(|(slot_index, champion_id)| LoadingPlayerSlot {
                slot_index,
                champion_id,
                puuid: format!("preview-red-{slot_index}"),
                display_name: String::new(),
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
            local_champion_id: Some(BLUE_IDS[0]),
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

    fn build_loading_alignment_targets(
        &self,
        snapshot: &LoadingScreenSnapshot,
    ) -> LoadingAlignmentTargets {
        let map_team = |slots: &[LoadingPlayerSlot]| -> Vec<LoadingSlotTarget> {
            slots
                .iter()
                .take(5)
                .map(|slot| {
                    let (name_zh, title_zh, alias) = self
                        .meta
                        .as_ref()
                        .and_then(|m| m.get_hero_stats(slot.champion_id))
                        .map(|stats| (stats.name_zh, stats.title_zh, stats.alias))
                        .unwrap_or_default();
                    LoadingSlotTarget {
                        name_zh,
                        title_zh,
                        alias,
                        summoner_name: slot.display_name.clone(),
                    }
                })
                .collect()
        };
        LoadingAlignmentTargets {
            game_id: snapshot.game_id,
            team_one: map_team(&snapshot.team_one),
            team_two: map_team(&snapshot.team_two),
        }
    }

    fn map_loading_team_slots(
        &self,
        players: &[LoadingPlayerSlot],
        ratings: &TeamRatingsSnapshot,
        order: Option<&[usize]>,
    ) -> Vec<LoadingCardSlotData> {
        map_loading_team_slots_with(
            players,
            ratings,
            order,
            &self.rating_titles,
            |hero_id| self.lookup_loading_hero(hero_id),
        )
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
                    && self.offered_augments.is_empty()
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

        let (team_one_order, team_two_order) = match self.loading_card_order.as_ref() {
            Some((gid, t1, t2)) if *gid == snap.game_id => {
                (Some(t1.as_slice()), Some(t2.as_slice()))
            }
            _ => (None, None),
        };
        let team_one =
            self.map_loading_team_slots(&snap.team_one, &snap.team_one_ratings, team_one_order);
        let team_two =
            self.map_loading_team_slots(&snap.team_two, &snap.team_two_ratings, team_two_order);
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

    fn has_tab_roster_data(&self) -> bool {
        self.last_match_loading_screen.is_some()
            || self
                .live_game
                .as_ref()
                .is_some_and(|g| !g.players.is_empty())
    }

    fn build_baseline_tab_entries(
        &self,
        snap: Option<&LoadingScreenSnapshot>,
    ) -> (
        u64,
        Vec<(TabRosterKey, String, LoadingCardSlotData)>,
        Vec<(TabRosterKey, String, LoadingCardSlotData)>,
    ) {
        let game_id = snap.map(|s| s.game_id).unwrap_or(0);
        let empty_ratings = TeamRatingsSnapshot::default();
        let (snap_t1, snap_r1, snap_t2, snap_r2) = match snap {
            Some(s) => (
                s.team_one.as_slice(),
                &s.team_one_ratings,
                s.team_two.as_slice(),
                &s.team_two_ratings,
            ),
            None => (&[][..], &empty_ratings, &[][..], &empty_ratings),
        };

        let (live_t1, live_t2): (Vec<LivePlayerEntry>, Vec<LivePlayerEntry>) =
            if self.preview_tab_scoreboard && self.last_match_loading_screen.is_none() {
                (Vec::new(), Vec::new())
            } else if let Some(live) = self.live_game.as_ref() {
                let mut blue = Vec::with_capacity(5);
                let mut red = Vec::with_capacity(5);
                for p in &live.players {
                    match p.team {
                        LoadingTeamSide::Blue => blue.push(p.clone()),
                        LoadingTeamSide::Red => red.push(p.clone()),
                    }
                }
                (blue, red)
            } else {
                (Vec::new(), Vec::new())
            };

        let (team_one_order, team_two_order) = match self.loading_card_order.as_ref() {
            Some((gid, t1, t2)) if *gid == game_id => (Some(t1.as_slice()), Some(t2.as_slice())),
            _ => (None, None),
        };

        let resolve_hero = |name: &str| {
            self.meta
                .as_ref()
                .and_then(|m| m.resolve_champion_id_by_alias(name))
        };
        let lookup_hero = |hero_id: i32| self.lookup_loading_hero(hero_id);

        let left_entries = build_tab_team_entries(
            &live_t1,
            snap_t1,
            snap_r1,
            team_one_order,
            &self.rating_titles,
            resolve_hero,
            lookup_hero,
        );
        let right_entries = build_tab_team_entries(
            &live_t2,
            snap_t2,
            snap_r2,
            team_two_order,
            &self.rating_titles,
            resolve_hero,
            lookup_hero,
        );

        (game_id, left_entries, right_entries)
    }

    /// Computes left/right hold-Tab scoreboard side states and their deferred Win32 window commands.
    pub fn build_tab_scoreboard_sync(
        &mut self,
        left_hwnd_raw: isize,
        right_hwnd_raw: isize,
    ) -> (
        TabScoreboardSideState,
        Option<Win32WindowCommand>,
        TabScoreboardSideState,
        Option<Win32WindowCommand>,
    ) {
        let in_diagnostics = self.diagnostics_open && self.preview_mode.is_none();
        let effective_mode = self.preview_mode.unwrap_or(self.mode);
        let should_show = !in_diagnostics
            && (self.preview_tab_scoreboard
                || (effective_mode == AppMode::InGameOverlay
                    && self.overlay_visible
                    && self.game_focused
                    && self.tab_scoreboard_held
                    && self.loading_screen.is_none()
                    && self.offered_augments.is_empty()
                    && self.has_tab_roster_data()));

        let target_rect = if should_show {
            self.last_detected_game_rect.or_else(|| {
                self.preview_tab_scoreboard
                    .then(win32::default_preview_game_rect)
            })
        } else {
            None
        };

        let hide_sync = |this: &mut Self| {
            let left_cmd = if left_hwnd_raw != 0
                && this.last_tab_left_sync != Some((None, false, false))
            {
                this.last_tab_left_sync = Some((None, false, false));
                Some(Win32WindowCommand {
                    hwnd_raw: left_hwnd_raw,
                    action: Win32WindowAction::TabScoreboardSideOverlay {
                        target_rect: None,
                        side: TabScoreboardSide::Left,
                        has_augment_wings: false,
                        visible: false,
                    },
                })
            } else {
                None
            };
            let right_cmd = if right_hwnd_raw != 0
                && this.last_tab_right_sync != Some((None, false, false))
            {
                this.last_tab_right_sync = Some((None, false, false));
                Some(Win32WindowCommand {
                    hwnd_raw: right_hwnd_raw,
                    action: Win32WindowAction::TabScoreboardSideOverlay {
                        target_rect: None,
                        side: TabScoreboardSide::Right,
                        has_augment_wings: false,
                        visible: false,
                    },
                })
            } else {
                None
            };
            (
                TabScoreboardSideState::default(),
                left_cmd,
                TabScoreboardSideState::default(),
                right_cmd,
            )
        };

        let Some(rect) = target_rect else {
            return hide_sync(self);
        };

        let preview_snap = (self.preview_tab_scoreboard
            && self.last_match_loading_screen.is_none()
            && !self
                .live_game
                .as_ref()
                .is_some_and(|g| !g.players.is_empty()))
        .then(|| self.preview_loading_screen_snapshot());
        let snap = self
            .last_match_loading_screen
            .as_ref()
            .or(preview_snap.as_ref());
        if snap.is_none() && !self.has_tab_roster_data() {
            return hide_sync(self);
        }

        let has_augment_wings = snap
            .map(snapshot_has_augment_wings)
            .unwrap_or_else(|| {
                self.live_game.as_ref().is_some_and(|g| {
                    g.game_mode.eq_ignore_ascii_case("KIWI")
                        || g.game_mode.eq_ignore_ascii_case("CHERRY")
                })
            });
        let local_side = snap
            .map(|s| s.local_side)
            .unwrap_or(LoadingTeamSide::Blue);
        let (left_label, left_is_enemy, right_label, right_is_enemy) =
            tab_scoreboard_side_meta(local_side);

        let (game_id, left_entries, right_entries) = self.build_baseline_tab_entries(snap);
        let (left_visual, right_visual) = match self.tab_visual_keys.as_ref() {
            Some((gid, lk, rk)) if *gid == game_id => (Some(lk.as_slice()), Some(rk.as_slice())),
            _ => (None, None),
        };
        let left_slots = apply_tab_visual_order(left_entries, left_visual);
        let right_slots = apply_tab_visual_order(right_entries, right_visual);

        let empty_ratings = TeamRatingsSnapshot::default();
        let (r1, r2) = match snap {
            Some(s) => (&s.team_one_ratings, &s.team_two_ratings),
            None => (&empty_ratings, &empty_ratings),
        };
        let left_summary = compute_team_loading_summary(r1, &self.rating_titles);
        let right_summary = compute_team_loading_summary(r2, &self.rating_titles);

        let left_scale = win32::window_scale_for_hwnd(left_hwnd_raw, rect.scale()).max(0.5);
        let right_scale = win32::window_scale_for_hwnd(right_hwnd_raw, rect.scale()).max(0.5);

        let left_state = TabScoreboardSideState {
            visible: !left_slots.is_empty(),
            side: TabScoreboardSide::Left,
            side_label: left_label.to_owned(),
            is_enemy: left_is_enemy,
            has_augment_wings,
            slots: left_slots,
            summary: left_summary,
            window_width_dip: rect.width() as f32 / left_scale,
            window_height_dip: rect.height() as f32 / left_scale,
        };
        let right_state = TabScoreboardSideState {
            visible: !right_slots.is_empty(),
            side: TabScoreboardSide::Right,
            side_label: right_label.to_owned(),
            is_enemy: right_is_enemy,
            has_augment_wings,
            slots: right_slots,
            summary: right_summary,
            window_width_dip: rect.width() as f32 / right_scale,
            window_height_dip: rect.height() as f32 / right_scale,
        };

        let left_sync_key = (
            left_state.visible.then_some(rect),
            has_augment_wings,
            left_state.visible,
        );
        let left_cmd = if left_hwnd_raw != 0 && self.last_tab_left_sync != Some(left_sync_key) {
            self.last_tab_left_sync = Some(left_sync_key);
            Some(Win32WindowCommand {
                hwnd_raw: left_hwnd_raw,
                action: Win32WindowAction::TabScoreboardSideOverlay {
                    target_rect: left_sync_key.0,
                    side: TabScoreboardSide::Left,
                    has_augment_wings,
                    visible: left_state.visible,
                },
            })
        } else {
            None
        };

        let right_sync_key = (
            right_state.visible.then_some(rect),
            has_augment_wings,
            right_state.visible,
        );
        let right_cmd = if right_hwnd_raw != 0 && self.last_tab_right_sync != Some(right_sync_key) {
            self.last_tab_right_sync = Some(right_sync_key);
            Some(Win32WindowCommand {
                hwnd_raw: right_hwnd_raw,
                action: Win32WindowAction::TabScoreboardSideOverlay {
                    target_rect: right_sync_key.0,
                    side: TabScoreboardSide::Right,
                    has_augment_wings,
                    visible: right_state.visible,
                },
            })
        } else {
            None
        };

        (left_state, left_cmd, right_state, right_cmd)
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
            for loading in [
                self.loading_screen.as_mut(),
                self.last_match_loading_screen.as_mut(),
            ]
            .into_iter()
            .flatten()
            {
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
        self.vision.set_loading_targets(None);
        self.vision.set_tab_targets(None);
        self.loading_card_order = None;
        self.tab_visual_keys = None;
        self.last_tab_target_keys = None;
        self.loading_screen = None;
        self.last_match_loading_screen = None;
        self.tab_scoreboard_held = false;
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
                if self
                    .loading_card_order
                    .as_ref()
                    .is_some_and(|(gid, _, _)| *gid != snap.game_id)
                {
                    self.loading_card_order = None;
                }
                if self
                    .tab_visual_keys
                    .as_ref()
                    .is_some_and(|(gid, _, _)| *gid != snap.game_id)
                {
                    self.tab_visual_keys = None;
                }
                if self
                    .last_tab_target_keys
                    .as_ref()
                    .is_some_and(|(gid, _, _)| *gid != snap.game_id)
                {
                    self.last_tab_target_keys = None;
                }
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
                let local_champion_id = snap.local_champion_id;
                let targets = self.build_loading_alignment_targets(&snap);
                self.last_match_loading_screen = Some(snap.clone());
                self.loading_screen = Some(snap);
                self.vision.set_loading_targets(Some(targets));
                self.maybe_populate_hero_from_roster(local_champion_id);
                self.reset_window_sync_cache();
            }
            AppEvent::LoadingCardOrderDetected {
                game_id,
                team_one_order,
                team_two_order,
            } => {
                let matches_current = self
                    .loading_screen
                    .as_ref()
                    .is_some_and(|s| s.game_id == game_id)
                    || self
                        .last_match_loading_screen
                        .as_ref()
                        .is_some_and(|s| s.game_id == game_id);
                if matches_current {
                    info!(
                        game_id,
                        team_one_order = ?team_one_order,
                        team_two_order = ?team_two_order,
                        "applied OCR loading screen card order"
                    );
                    self.loading_card_order = Some((game_id, team_one_order, team_two_order));
                    self.reset_window_sync_cache();
                } else {
                    debug!(
                        game_id,
                        "ignoring stale loading screen card order for non-active game"
                    );
                }
            }
            AppEvent::TabRowOrderDetected {
                game_id,
                left_order,
                right_order,
            } => {
                let matched_keys = self
                    .last_tab_target_keys
                    .as_ref()
                    .filter(|(target_gid, _, _)| *target_gid == game_id)
                    .map(|(_, left_keys, right_keys)| {
                        let ordered_left: Vec<TabRosterKey> =
                            resolve_slot_order(Some(&left_order), left_keys.len())
                                .into_iter()
                                .map(|idx| left_keys[idx].clone())
                                .collect();
                        let ordered_right: Vec<TabRosterKey> =
                            resolve_slot_order(Some(&right_order), right_keys.len())
                                .into_iter()
                                .map(|idx| right_keys[idx].clone())
                                .collect();
                        (ordered_left, ordered_right)
                    });
                if let Some((ordered_left, ordered_right)) = matched_keys {
                    info!(
                        game_id,
                        left_order = ?left_order,
                        right_order = ?right_order,
                        "applied OCR Tab scoreboard row order"
                    );
                    self.tab_visual_keys = Some((game_id, ordered_left, ordered_right));
                    self.reset_window_sync_cache();
                } else {
                    debug!(
                        game_id,
                        "ignoring stale or unmatched Tab scoreboard row order"
                    );
                }
            }
            AppEvent::LoadingScreenCleared => {
                self.vision.set_loading_targets(None);
                if self.loading_screen.take().is_some() {
                    self.reset_window_sync_cache();
                }
            }
            AppEvent::MatchRosterUpdated(mut snap) => {
                self.refresh_rating_titles_if_changed();
                if self
                    .loading_card_order
                    .as_ref()
                    .is_some_and(|(gid, _, _)| *gid != snap.game_id)
                {
                    self.loading_card_order = None;
                }
                if self
                    .tab_visual_keys
                    .as_ref()
                    .is_some_and(|(gid, _, _)| *gid != snap.game_id)
                {
                    self.tab_visual_keys = None;
                }
                if self
                    .last_tab_target_keys
                    .as_ref()
                    .is_some_and(|(gid, _, _)| *gid != snap.game_id)
                {
                    self.last_tab_target_keys = None;
                }
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
                let local_champion_id = snap.local_champion_id;
                self.last_match_loading_screen = Some(snap);
                self.maybe_populate_hero_from_roster(local_champion_id);
                self.reset_window_sync_cache();
            }
            AppEvent::LiveGameUpdated(live) => {
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
        self.tab_scoreboard_held = false;
        self.reset_window_sync_cache();
    }

    fn maybe_populate_hero_from_roster(&mut self, local_champion_id: Option<i32>) {
        if self.mode == AppMode::InGameOverlay && self.active_hero_id.is_none() {
            if let Some(local_id) = local_champion_id.filter(|&id| id > 0) {
                let owned_items = self
                    .live_game
                    .as_ref()
                    .map(|l| l.owned_item_ids.clone())
                    .unwrap_or_default();
                self.populate_hero_hud_state(Some(local_id), None, &owned_items);
            }
        }
    }

    fn refresh_live_game_data(&mut self, live: &LiveGameState) {
        self.vision.update_game_context(
            self.mode == AppMode::InGameOverlay && self.overlay_visible,
            live_game_to_vision_ctx(live),
        );

        let roster_local_champion_id = self
            .loading_screen
            .as_ref()
            .or(self.last_match_loading_screen.as_ref())
            .and_then(|s| s.local_champion_id);
        let hero_id = resolve_live_hero_id_with(
            live,
            roster_local_champion_id,
            self.active_champ_select_hero_id,
            |name| {
                self.meta
                    .as_ref()
                    .and_then(|meta| meta.resolve_champion_id_by_alias(name))
            },
        );

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

    let (app_update_label, app_update_color, has_app_update) = match &app.app_update_state {
        AppUpdateUiState::Checking => ("检查中…".to_owned(), theme::text_muted(), false),
        AppUpdateUiState::UpToDate { latest_version, .. } => (
            if latest_version.is_empty() {
                "已是最新版本".to_owned()
            } else {
                format!("已是最新 ({latest_version})")
            },
            theme::cyan(),
            false,
        ),
        AppUpdateUiState::UpdateAvailable {
            latest_version,
            is_prerelease,
            ..
        } => (
            if *is_prerelease {
                format!("发现新版本 {latest_version} (预发布)")
            } else {
                format!("发现新版本 {latest_version}")
            },
            theme::gold(),
            true,
        ),
        AppUpdateUiState::Failed(reason) => (
            if reason.is_empty() {
                "检查失败".to_owned()
            } else {
                "检查失败 (请稍后重试)".to_owned()
            },
            theme::warning_text(),
            false,
        ),
    };
    let check_app_update_btn_label = if app.app_update_in_flight {
        "检查软件更新中…"
    } else {
        "检查软件更新"
    };
    let release_btn_label = if has_app_update {
        "前往下载新版本 ↗"
    } else {
        "查看 Release ↗"
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
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme::gold())
                                .child("Hexrs · 运行诊断面板"),
                        )
                        .child(
                            div()
                                .px(px(5.0))
                                .py(px(1.0))
                                .rounded(px(3.0))
                                .bg(theme::card_elevated_bg())
                                .border_1()
                                .border_color(theme::border_subtle())
                                .text_size(px(10.0))
                                .font_family(theme::MONO_FONT)
                                .text_color(theme::text_secondary())
                                .child(APP_VERSION),
                        ),
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
                        .child(diag_row(
                            "软件版本",
                            format!("{APP_VERSION} ({APP_GIT_SHA})"),
                        ))
                        .child(diag_row_colored(
                            "版本更新",
                            app_update_label,
                            app_update_color,
                        ))
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
                                .flex()
                                .gap_2()
                                .child(
                                    div()
                                        .id("preview-loading-screen-btn")
                                        .flex_1()
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
                                        .id("preview-tab-scoreboard-btn")
                                        .flex_1()
                                        .cursor_pointer()
                                        .px_2()
                                        .py_1()
                                        .rounded(px(3.0))
                                        .bg(theme::gold_dim())
                                        .border_1()
                                        .border_color(theme::border_gold())
                                        .hover(|s| s.bg(theme::card_elevated_bg()))
                                        .flex()
                                        .justify_center()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme::gold())
                                        .child("预览Tab战绩面板")
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            this.preview_tab_scoreboard_mode(cx);
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child("提示：局内按住 Tab 可在战绩面板两侧查看双方 10 人数据（松手隐藏）；右上角出装窗可点标题栏「收起/展开」，预览时右键托盘“显示诊断面板”即可切回。"),
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
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .id("check-app-update-btn")
                                .flex_1()
                                .cursor_pointer()
                                .px_2()
                                .py_1()
                                .rounded(px(3.0))
                                .bg(theme::card_elevated_bg())
                                .border_1()
                                .border_color(theme::border_subtle())
                                .hover(|s| s.bg(theme::bg_highlight()))
                                .flex()
                                .justify_center()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::text_primary())
                                .child(check_app_update_btn_label)
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.trigger_app_update_check(cx);
                                })),
                        )
                        .child(
                            div()
                                .id("open-app-release-btn")
                                .flex_1()
                                .cursor_pointer()
                                .px_2()
                                .py_1()
                                .rounded(px(3.0))
                                .bg(if has_app_update {
                                    theme::gold_dim()
                                } else {
                                    theme::cyan_dim()
                                })
                                .border_1()
                                .border_color(if has_app_update {
                                    theme::border_gold()
                                } else {
                                    theme::border_cyan()
                                })
                                .hover(|s| s.bg(theme::card_elevated_bg()))
                                .flex()
                                .justify_center()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if has_app_update {
                                    theme::gold()
                                } else {
                                    theme::cyan()
                                })
                                .child(release_btn_label)
                                .on_click(cx.listener(|this, _event, _window, _cx| {
                                    this.open_app_release_page();
                                })),
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

fn resolve_release_open_url(state: &AppUpdateUiState) -> &str {
    match state {
        AppUpdateUiState::UpdateAvailable {
            release_url,
            download_url,
            ..
        } => {
            if !release_url.trim().is_empty() {
                release_url.trim()
            } else if let Some(url) = download_url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
                url
            } else {
                DEFAULT_RELEASES_PAGE_URL
            }
        }
        AppUpdateUiState::UpToDate { release_url, .. } if !release_url.trim().is_empty() => {
            release_url.trim()
        }
        _ => DEFAULT_RELEASES_PAGE_URL,
    }
}

fn normalize_tab_display_name(raw: &str) -> String {
    raw.split('#')
        .next()
        .unwrap_or(raw)
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn loading_placeholder_badge(rating_titles: &RatingTitlesConfig) -> SummonerBadgeData {
    let (tier_index, title) = rating_titles.resolve(SummonerRatingStatus::Loading, 0);
    SummonerBadgeData {
        status: SummonerRatingStatus::Loading,
        score: 0,
        tier_index,
        title,
        streak: 0,
        premade_group_id: None,
        is_premade_last_mvp: false,
    }
}

fn build_tab_team_entries<R, L>(
    live_players: &[LivePlayerEntry],
    snap_players: &[LoadingPlayerSlot],
    snap_ratings: &TeamRatingsSnapshot,
    fallback_loading_order: Option<&[usize]>,
    rating_titles: &RatingTitlesConfig,
    mut resolve_hero_id: R,
    mut lookup_hero: L,
) -> Vec<(TabRosterKey, String, LoadingCardSlotData)>
where
    R: FnMut(&str) -> Option<i32>,
    L: FnMut(i32) -> Option<LoadingCardHeroData>,
{
    if !live_players.is_empty() {
        let mut used = vec![false; snap_players.len()];
        let mut entries = Vec::with_capacity(live_players.len().min(5));

        for lp in live_players.iter().take(5) {
            let live_champ_id = resolve_hero_id(&lp.raw_champion_name)
                .filter(|&id| id > 0)
                .or_else(|| resolve_hero_id(&lp.champion_name).filter(|&id| id > 0))
                .unwrap_or(0);
            let live_norm_name = normalize_tab_display_name(&lp.display_name);

            let matched_lcu_idx = if live_champ_id > 0 {
                snap_players
                    .iter()
                    .enumerate()
                    .find(|(idx, slot)| !used[*idx] && slot.champion_id == live_champ_id)
                    .map(|(idx, _)| idx)
            } else {
                None
            }
            .or_else(|| {
                if live_norm_name.is_empty() {
                    None
                } else {
                    snap_players
                        .iter()
                        .enumerate()
                        .find(|(idx, slot)| {
                            !used[*idx]
                                && normalize_tab_display_name(&slot.display_name) == live_norm_name
                        })
                        .map(|(idx, _)| idx)
                }
            });

            if let Some(lcu_idx) = matched_lcu_idx {
                used[lcu_idx] = true;
                let slot = &snap_players[lcu_idx];
                let champ_id = if live_champ_id > 0 {
                    live_champ_id
                } else {
                    slot.champion_id.max(0)
                };
                let hero = (champ_id > 0).then(|| lookup_hero(champ_id)).flatten();
                let display_name = if !lp.display_name.is_empty() {
                    lp.display_name.clone()
                } else {
                    slot.display_name.clone()
                };
                let key = (champ_id, normalize_tab_display_name(&display_name));
                let summoner = snap_ratings
                    .slots
                    .get(lcu_idx)
                    .map(summoner_summary_to_badge)
                    .unwrap_or_else(|| loading_placeholder_badge(rating_titles));
                entries.push((key, display_name, LoadingCardSlotData { hero, summoner }));
            } else {
                let hero = (live_champ_id > 0)
                    .then(|| lookup_hero(live_champ_id))
                    .flatten();
                let display_name = lp.display_name.clone();
                let key = (live_champ_id, live_norm_name);
                let (tier_index, title) =
                    rating_titles.resolve(SummonerRatingStatus::Private, 0);
                let summoner = SummonerBadgeData {
                    status: SummonerRatingStatus::Private,
                    score: 0,
                    tier_index,
                    title,
                    streak: 0,
                    premade_group_id: None,
                    is_premade_last_mvp: false,
                };
                entries.push((key, display_name, LoadingCardSlotData { hero, summoner }));
            }
        }

        return entries;
    }

    let capped_len = snap_players.len().min(5);
    let indices = resolve_slot_order(fallback_loading_order, capped_len);
    indices
        .into_iter()
        .map(|idx| {
            let slot = &snap_players[idx];
            let champ_id = slot.champion_id.max(0);
            let hero = (champ_id > 0).then(|| lookup_hero(champ_id)).flatten();
            let display_name = slot.display_name.clone();
            let key = (champ_id, normalize_tab_display_name(&display_name));
            let summoner = snap_ratings
                .slots
                .get(idx)
                .map(summoner_summary_to_badge)
                .unwrap_or_else(|| loading_placeholder_badge(rating_titles));
            (key, display_name, LoadingCardSlotData { hero, summoner })
        })
        .collect()
}

fn apply_tab_visual_order(
    entries: Vec<(TabRosterKey, String, LoadingCardSlotData)>,
    visual_keys: Option<&[TabRosterKey]>,
) -> Vec<LoadingCardSlotData> {
    let Some(target_keys) = visual_keys.filter(|keys| !keys.is_empty()) else {
        return entries.into_iter().map(|(_, _, card)| card).collect();
    };

    let mut used = vec![false; entries.len()];
    let mut ordered_indices = Vec::with_capacity(entries.len());

    for (target_champ, target_name) in target_keys {
        let matched = entries
            .iter()
            .enumerate()
            .find(|(idx, ((cid, cname), _, _))| {
                !used[*idx] && cid == target_champ && cname == target_name
            })
            .map(|(idx, _)| idx)
            .or_else(|| {
                if *target_champ <= 0 {
                    return None;
                }
                let mut matches = entries
                    .iter()
                    .enumerate()
                    .filter(|(idx, ((cid, _), _, _))| !used[*idx] && cid == target_champ)
                    .map(|(idx, _)| idx);
                let first = matches.next()?;
                if matches.next().is_none() {
                    Some(first)
                } else {
                    None
                }
            })
            .or_else(|| {
                if target_name.is_empty() {
                    return None;
                }
                entries
                    .iter()
                    .enumerate()
                    .find(|(idx, ((_, cname), _, _))| !used[*idx] && cname == target_name)
                    .map(|(idx, _)| idx)
            });

        if let Some(idx) = matched {
            used[idx] = true;
            ordered_indices.push(idx);
        }
    }

    for (idx, is_used) in used.iter().enumerate() {
        if !*is_used {
            ordered_indices.push(idx);
        }
    }

    let mut cards: Vec<Option<LoadingCardSlotData>> = entries
        .into_iter()
        .map(|(_, _, card)| Some(card))
        .collect();
    ordered_indices
        .into_iter()
        .filter_map(|idx| cards[idx].take())
        .collect()
}

fn resolve_live_hero_id_with<F>(
    live: &LiveGameState,
    roster_local_champion_id: Option<i32>,
    champ_select_hero_id: Option<i32>,
    mut resolve_by_alias: F,
) -> Option<i32>
where
    F: FnMut(&str) -> Option<i32>,
{
    live.active_champion_id
        .filter(|&id| id > 0)
        .or_else(|| resolve_by_alias(&live.active_champion_raw_name).filter(|&id| id > 0))
        .or_else(|| resolve_by_alias(&live.active_champion_name).filter(|&id| id > 0))
        .or_else(|| roster_local_champion_id.filter(|&id| id > 0))
        .or_else(|| champ_select_hero_id.filter(|&id| id > 0))
}

fn snapshot_has_augment_wings(snap: &LoadingScreenSnapshot) -> bool {
    snap.queue_id == Some(2400)
        || snap.queue_id == Some(1700)
        || snap.game_mode.eq_ignore_ascii_case("KIWI")
        || snap.game_mode.eq_ignore_ascii_case("CHERRY")
}

fn tab_scoreboard_side_meta(
    local_side: LoadingTeamSide,
) -> (&'static str, bool, &'static str, bool) {
    match local_side {
        LoadingTeamSide::Blue => ("我方(蓝)", false, "敌方(红)", true),
        LoadingTeamSide::Red => ("敌方(蓝)", true, "我方(红)", false),
    }
}

fn resolve_slot_order(order: Option<&[usize]>, len: usize) -> Vec<usize> {
    if let Some(perm) = order {
        if perm.len() == len {
            let mut seen = vec![false; len];
            let mut valid = true;
            for &idx in perm {
                if idx >= len || seen[idx] {
                    valid = false;
                    break;
                }
                seen[idx] = true;
            }
            if valid {
                return perm.to_vec();
            }
        }
    }
    (0..len).collect()
}

fn map_loading_team_slots_with<F>(
    players: &[LoadingPlayerSlot],
    ratings: &TeamRatingsSnapshot,
    order: Option<&[usize]>,
    rating_titles: &RatingTitlesConfig,
    mut lookup_hero: F,
) -> Vec<LoadingCardSlotData>
where
    F: FnMut(i32) -> Option<LoadingCardHeroData>,
{
    let capped_len = players.len().min(5);
    let indices = resolve_slot_order(order, capped_len);
    indices
        .into_iter()
        .map(|idx| {
            let player = &players[idx];
            let hero = lookup_hero(player.champion_id);
            let summoner = ratings
                .slots
                .get(idx)
                .map(summoner_summary_to_badge)
                .unwrap_or_else(|| {
                    let (tier_index, title) =
                        rating_titles.resolve(SummonerRatingStatus::Loading, 0);
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
    use super::{
        apply_tab_visual_order, build_tab_team_entries, map_loading_team_slots_with,
        order_subset_cards_for_display, resolve_live_hero_id_with, resolve_release_open_url,
        resolve_slot_order, snapshot_has_augment_wings, tab_scoreboard_side_meta,
        AppUpdateUiState, HeroTier, LiveGameState, LivePlayerEntry, LoadingCardHeroData,
        LoadingPlayerSlot, LoadingScreenSnapshot, LoadingTeamSide, RatingTitlesConfig,
        SummonerRatingStatus, SummonerRatingSummary, TabRosterKey, TeamRatingsSnapshot,
        DEFAULT_RELEASES_PAGE_URL,
    };

    #[test]
    fn resolves_release_open_url_with_fallback() {
        assert_eq!(
            resolve_release_open_url(&AppUpdateUiState::Checking),
            DEFAULT_RELEASES_PAGE_URL
        );
        assert_eq!(
            resolve_release_open_url(&AppUpdateUiState::Failed("net".to_owned())),
            DEFAULT_RELEASES_PAGE_URL
        );
        assert_eq!(
            resolve_release_open_url(&AppUpdateUiState::UpToDate {
                latest_version: "v0.1.0".to_owned(),
                release_url: "https://github.com/zoffyultraman/Hexrs/releases/tag/v0.1.0".to_owned(),
            }),
            "https://github.com/zoffyultraman/Hexrs/releases/tag/v0.1.0"
        );
        assert_eq!(
            resolve_release_open_url(&AppUpdateUiState::UpdateAvailable {
                latest_version: "v0.2.0".to_owned(),
                release_url: "https://github.com/zoffyultraman/Hexrs/releases/tag/v0.2.0".to_owned(),
                download_url: Some("https://github.com/zoffyultraman/Hexrs/releases/download/v0.2.0/Hexrs.zip".to_owned()),
                is_prerelease: false,
            }),
            "https://github.com/zoffyultraman/Hexrs/releases/tag/v0.2.0"
        );
        assert_eq!(
            resolve_release_open_url(&AppUpdateUiState::UpdateAvailable {
                latest_version: "v0.2.0".to_owned(),
                release_url: String::new(),
                download_url: Some("https://github.com/zoffyultraman/Hexrs/releases/download/v0.2.0/Hexrs.zip".to_owned()),
                is_prerelease: true,
            }),
            "https://github.com/zoffyultraman/Hexrs/releases/download/v0.2.0/Hexrs.zip"
        );
    }

    #[test]
    fn merges_live_five_players_with_partial_lcu_roster_including_anonymous_players() {
        let live_players = vec![
            LivePlayerEntry {
                team: LoadingTeamSide::Blue,
                champion_name: "亚索".to_owned(),
                raw_champion_name: "game_character_displayname_Yasuo".to_owned(),
                display_name: "PlayerOne".to_owned(),
                position: String::new(),
            },
            LivePlayerEntry {
                team: LoadingTeamSide::Blue,
                champion_name: "阿狸".to_owned(),
                raw_champion_name: "game_character_displayname_Ahri".to_owned(),
                display_name: "AnonStreamer1".to_owned(),
                position: String::new(),
            },
            LivePlayerEntry {
                team: LoadingTeamSide::Blue,
                champion_name: "李青".to_owned(),
                raw_champion_name: "game_character_displayname_LeeSin".to_owned(),
                display_name: "PlayerThree".to_owned(),
                position: String::new(),
            },
            LivePlayerEntry {
                team: LoadingTeamSide::Blue,
                champion_name: "泽拉斯".to_owned(),
                raw_champion_name: "game_character_displayname_Xerath".to_owned(),
                display_name: "AnonStreamer2".to_owned(),
                position: String::new(),
            },
            LivePlayerEntry {
                team: LoadingTeamSide::Blue,
                champion_name: "金克丝".to_owned(),
                raw_champion_name: "game_character_displayname_Jinx".to_owned(),
                display_name: "PlayerFive".to_owned(),
                position: String::new(),
            },
        ];

        // LCU omitted the 2 anonymous players (Ahri 103 & Xerath 101).
        let snap_players = vec![
            LoadingPlayerSlot {
                slot_index: 0,
                champion_id: 157,
                puuid: "p1".to_owned(),
                display_name: "PlayerOne".to_owned(),
            },
            LoadingPlayerSlot {
                slot_index: 1,
                champion_id: 64,
                puuid: "p3".to_owned(),
                display_name: "PlayerThree".to_owned(),
            },
            LoadingPlayerSlot {
                slot_index: 2,
                champion_id: 222,
                puuid: "p5".to_owned(),
                display_name: "PlayerFive".to_owned(),
            },
        ];
        let snap_ratings = TeamRatingsSnapshot {
            slots: vec![
                SummonerRatingSummary {
                    cell_id: 0,
                    status: SummonerRatingStatus::Ready,
                    score: 86,
                    tier_index: 0,
                    title: "通天代".to_owned(),
                    games_count: 14,
                    win_rate: 58.0,
                    avg_kda: 4.1,
                    streak: 4,
                    premade_group_id: Some(1),
                    is_premade_last_mvp: true,
                },
                SummonerRatingSummary {
                    cell_id: 1,
                    status: SummonerRatingStatus::Ready,
                    score: 71,
                    tier_index: 1,
                    title: "小代".to_owned(),
                    games_count: 11,
                    win_rate: 53.0,
                    avg_kda: 3.2,
                    streak: 0,
                    premade_group_id: Some(1),
                    is_premade_last_mvp: false,
                },
                SummonerRatingSummary {
                    cell_id: 2,
                    status: SummonerRatingStatus::Ready,
                    score: 54,
                    tier_index: 3,
                    title: "普通".to_owned(),
                    games_count: 9,
                    win_rate: 49.0,
                    avg_kda: 2.4,
                    streak: -3,
                    premade_group_id: None,
                    is_premade_last_mvp: false,
                },
            ],
            premade_groups: Vec::new(),
        };
        let titles = RatingTitlesConfig::default();
        let resolve = |raw: &str| match raw {
            "game_character_displayname_Yasuo" => Some(157),
            "game_character_displayname_Ahri" => Some(103),
            "game_character_displayname_LeeSin" => Some(64),
            "game_character_displayname_Xerath" => Some(101),
            "game_character_displayname_Jinx" => Some(222),
            _ => None,
        };
        let lookup = |id: i32| {
            Some(LoadingCardHeroData {
                tier: HeroTier::S,
                win_rate: 52.5,
                name_zh: format!("Hero{id}"),
            })
        };

        let entries = build_tab_team_entries(
            &live_players,
            &snap_players,
            &snap_ratings,
            None,
            &titles,
            resolve,
            lookup,
        );

        assert_eq!(entries.len(), 5);
        // Matched LCU players keep full score/premade/MVP/streak badges.
        assert_eq!(entries[0].2.hero.as_ref().map(|h| h.name_zh.as_str()), Some("Hero157"));
        assert_eq!(entries[0].2.summoner.score, 86);
        assert_eq!(entries[0].2.summoner.premade_group_id, Some(1));
        assert!(entries[0].2.summoner.is_premade_last_mvp);

        assert_eq!(entries[2].2.hero.as_ref().map(|h| h.name_zh.as_str()), Some("Hero64"));
        assert_eq!(entries[2].2.summoner.score, 71);
        assert_eq!(entries[2].2.summoner.premade_group_id, Some(1));

        assert_eq!(entries[4].2.hero.as_ref().map(|h| h.name_zh.as_str()), Some("Hero222"));
        assert_eq!(entries[4].2.summoner.score, 54);
        assert_eq!(entries[4].2.summoner.streak, -3);

        // Anonymous players omitted by LCU still render full hero meta + Private rating status.
        assert_eq!(entries[1].2.hero.as_ref().map(|h| h.name_zh.as_str()), Some("Hero103"));
        assert_eq!(entries[1].2.summoner.status, SummonerRatingStatus::Private);

        assert_eq!(entries[3].2.hero.as_ref().map(|h| h.name_zh.as_str()), Some("Hero101"));
        assert_eq!(entries[3].2.summoner.status, SummonerRatingStatus::Private);
    }

    #[test]
    fn tab_visual_keys_lock_row_order_across_late_rating_updates() {
        let live_players = vec![
            LivePlayerEntry {
                team: LoadingTeamSide::Red,
                champion_name: "亚索".to_owned(),
                raw_champion_name: "Yasuo".to_owned(),
                display_name: "Alpha".to_owned(),
                position: String::new(),
            },
            LivePlayerEntry {
                team: LoadingTeamSide::Red,
                champion_name: "阿狸".to_owned(),
                raw_champion_name: "Ahri".to_owned(),
                display_name: "Beta".to_owned(),
                position: String::new(),
            },
            LivePlayerEntry {
                team: LoadingTeamSide::Red,
                champion_name: "李青".to_owned(),
                raw_champion_name: "LeeSin".to_owned(),
                display_name: "Gamma".to_owned(),
                position: String::new(),
            },
        ];
        let snap_players = vec![
            LoadingPlayerSlot {
                slot_index: 0,
                champion_id: 157,
                puuid: "a".to_owned(),
                display_name: "Alpha".to_owned(),
            },
            LoadingPlayerSlot {
                slot_index: 1,
                champion_id: 103,
                puuid: "b".to_owned(),
                display_name: "Beta".to_owned(),
            },
            LoadingPlayerSlot {
                slot_index: 2,
                champion_id: 64,
                puuid: "c".to_owned(),
                display_name: "Gamma".to_owned(),
            },
        ];
        let titles = RatingTitlesConfig::default();
        let resolve = |raw: &str| match raw {
            "Yasuo" => Some(157),
            "Ahri" => Some(103),
            "LeeSin" => Some(64),
            _ => None,
        };
        let lookup = |id: i32| {
            Some(LoadingCardHeroData {
                tier: HeroTier::A,
                win_rate: 51.0,
                name_zh: format!("H{id}"),
            })
        };

        // Initial build before SGP ratings finish loading.
        let initial_entries = build_tab_team_entries(
            &live_players,
            &snap_players,
            &TeamRatingsSnapshot::default(),
            None,
            &titles,
            resolve,
            lookup,
        );
        let target_keys: Vec<TabRosterKey> =
            initial_entries.iter().map(|(k, _, _)| k.clone()).collect();
        // Simulated TabRowOrderDetected permutation `[2, 0, 1]`.
        let visual_keys: Vec<TabRosterKey> = resolve_slot_order(Some(&[2, 0, 1]), target_keys.len())
            .into_iter()
            .map(|i| target_keys[i].clone())
            .collect();

        let cards_before = apply_tab_visual_order(initial_entries, Some(&visual_keys));
        assert_eq!(
            cards_before
                .iter()
                .map(|c| c.hero.as_ref().map(|h| h.name_zh.as_str()))
                .collect::<Vec<_>>(),
            vec![Some("H64"), Some("H157"), Some("H103")]
        );

        // Late MatchRosterUpdated arrives with populated SGP scores.
        let late_ratings = TeamRatingsSnapshot {
            slots: vec![
                SummonerRatingSummary {
                    cell_id: 0,
                    status: SummonerRatingStatus::Ready,
                    score: 80,
                    tier_index: 1,
                    title: "小代".to_owned(),
                    games_count: 10,
                    win_rate: 55.0,
                    avg_kda: 3.5,
                    streak: 3,
                    premade_group_id: None,
                    is_premade_last_mvp: false,
                },
                SummonerRatingSummary {
                    cell_id: 1,
                    status: SummonerRatingStatus::Ready,
                    score: 65,
                    tier_index: 2,
                    title: "人上人".to_owned(),
                    games_count: 10,
                    win_rate: 50.0,
                    avg_kda: 2.8,
                    streak: 0,
                    premade_group_id: None,
                    is_premade_last_mvp: false,
                },
                SummonerRatingSummary {
                    cell_id: 2,
                    status: SummonerRatingStatus::Ready,
                    score: 91,
                    tier_index: 0,
                    title: "通天代".to_owned(),
                    games_count: 12,
                    win_rate: 62.0,
                    avg_kda: 4.8,
                    streak: 5,
                    premade_group_id: None,
                    is_premade_last_mvp: false,
                },
            ],
            premade_groups: Vec::new(),
        };
        let updated_entries = build_tab_team_entries(
            &live_players,
            &snap_players,
            &late_ratings,
            None,
            &titles,
            resolve,
            lookup,
        );
        let cards_after = apply_tab_visual_order(updated_entries, Some(&visual_keys));
        assert_eq!(
            cards_after
                .iter()
                .map(|c| (
                    c.hero.as_ref().map(|h| h.name_zh.as_str()),
                    c.summoner.score
                ))
                .collect::<Vec<_>>(),
            vec![(Some("H64"), 91), (Some("H157"), 80), (Some("H103"), 65)]
        );
    }

    #[test]
    fn resolves_live_hero_id_by_five_level_priority_and_skips_invalid_ids() {
        let lookup = |name: &str| match name {
            "game_character_displayname_Ahri" => Some(103),
            "亚索" => Some(157),
            "invalid_zero" => Some(0),
            _ => None,
        };

        // 1. Direct live.active_champion_id takes highest priority.
        let mut live = LiveGameState {
            active_champion_id: Some(238),
            active_champion_raw_name: "game_character_displayname_Ahri".to_owned(),
            active_champion_name: "亚索".to_owned(),
            ..LiveGameState::default()
        };
        assert_eq!(
            resolve_live_hero_id_with(&live, Some(64), Some(11), lookup),
            Some(238)
        );

        // 2. Non-positive active_champion_id falls back to active_champion_raw_name.
        live.active_champion_id = Some(0);
        assert_eq!(
            resolve_live_hero_id_with(&live, Some(64), Some(11), lookup),
            Some(103)
        );

        // 3. Unresolved/invalid raw_name falls back to active_champion_name.
        live.active_champion_id = None;
        live.active_champion_raw_name = "invalid_zero".to_owned();
        assert_eq!(
            resolve_live_hero_id_with(&live, Some(64), Some(11), lookup),
            Some(157)
        );

        // 4. Unresolved live names fall back to roster local_champion_id.
        live.active_champion_raw_name.clear();
        live.active_champion_name.clear();
        assert_eq!(
            resolve_live_hero_id_with(&live, Some(64), Some(11), lookup),
            Some(64)
        );

        // 5. Invalid/missing roster local_champion_id falls back to champ_select_hero_id.
        assert_eq!(
            resolve_live_hero_id_with(&live, Some(0), Some(11), lookup),
            Some(11)
        );

        // 6. All missing or non-positive returns None.
        assert_eq!(
            resolve_live_hero_id_with(&live, Some(-1), Some(0), lookup),
            None
        );
    }

    #[test]
    fn detects_augment_wings_modes_and_queues() {
        let mut snap = LoadingScreenSnapshot::default();
        assert!(!snapshot_has_augment_wings(&snap));

        snap.queue_id = Some(2400);
        assert!(snapshot_has_augment_wings(&snap));

        snap.queue_id = Some(1700);
        assert!(snapshot_has_augment_wings(&snap));

        snap.queue_id = Some(450);
        snap.game_mode = "ARAM".to_owned();
        assert!(!snapshot_has_augment_wings(&snap));

        snap.game_mode = "kiwi".to_owned();
        assert!(snapshot_has_augment_wings(&snap));

        snap.game_mode = "CHERRY".to_owned();
        assert!(snapshot_has_augment_wings(&snap));
    }

    #[test]
    fn maps_tab_scoreboard_sides_by_local_side() {
        assert_eq!(
            tab_scoreboard_side_meta(LoadingTeamSide::Blue),
            ("我方(蓝)", false, "敌方(红)", true)
        );
        assert_eq!(
            tab_scoreboard_side_meta(LoadingTeamSide::Red),
            ("敌方(蓝)", true, "我方(红)", false)
        );
    }

    #[test]
    fn subset_cards_visual_order_places_third_blessed_card_in_center() {
        assert_eq!(order_subset_cards_for_display(&[10, 20]), vec![10, 20]);
        assert_eq!(
            order_subset_cards_for_display(&[14, 517, 497]),
            vec![14, 497, 517]
        );
    }

    #[test]
    fn resolve_slot_order_validates_permutations_and_falls_back_safely() {
        assert_eq!(resolve_slot_order(None, 5), vec![0, 1, 2, 3, 4]);
        assert_eq!(
            resolve_slot_order(Some(&[2, 0, 4, 1, 3]), 5),
            vec![2, 0, 4, 1, 3]
        );
        // Length mismatch falls back to identity.
        assert_eq!(resolve_slot_order(Some(&[1, 0]), 3), vec![0, 1, 2]);
        // Duplicate index falls back to identity.
        assert_eq!(resolve_slot_order(Some(&[0, 1, 1]), 3), vec![0, 1, 2]);
        // Out-of-bounds index falls back to identity.
        assert_eq!(resolve_slot_order(Some(&[0, 1, 3]), 3), vec![0, 1, 2]);
    }

    #[test]
    fn map_loading_team_slots_reorders_heroes_and_ratings_in_lockstep() {
        let players = vec![
            LoadingPlayerSlot {
                slot_index: 0,
                champion_id: 101,
                puuid: "p0".to_owned(),
                display_name: String::new(),
            },
            LoadingPlayerSlot {
                slot_index: 1,
                champion_id: 102,
                puuid: "p1".to_owned(),
                display_name: String::new(),
            },
            LoadingPlayerSlot {
                slot_index: 2,
                champion_id: 103,
                puuid: "p2".to_owned(),
                display_name: String::new(),
            },
        ];
        let ratings = TeamRatingsSnapshot {
            slots: vec![
                SummonerRatingSummary {
                    cell_id: 0,
                    status: SummonerRatingStatus::Ready,
                    score: 88,
                    tier_index: 0,
                    title: "通天代".to_owned(),
                    games_count: 15,
                    win_rate: 60.0,
                    avg_kda: 4.5,
                    streak: 5,
                    premade_group_id: Some(1),
                    is_premade_last_mvp: true,
                },
                SummonerRatingSummary {
                    cell_id: 1,
                    status: SummonerRatingStatus::Ready,
                    score: 62,
                    tier_index: 2,
                    title: "人上人".to_owned(),
                    games_count: 10,
                    win_rate: 51.0,
                    avg_kda: 3.0,
                    streak: -3,
                    premade_group_id: Some(1),
                    is_premade_last_mvp: false,
                },
                SummonerRatingSummary {
                    cell_id: 2,
                    status: SummonerRatingStatus::Private,
                    score: 0,
                    tier_index: 4,
                    title: "隐藏".to_owned(),
                    games_count: 0,
                    win_rate: 0.0,
                    avg_kda: 0.0,
                    streak: 0,
                    premade_group_id: None,
                    is_premade_last_mvp: false,
                },
            ],
            premade_groups: Vec::new(),
        };
        let titles = RatingTitlesConfig::default();

        let mapped = map_loading_team_slots_with(
            &players,
            &ratings,
            Some(&[2, 0, 1]),
            &titles,
            |hero_id| {
                Some(LoadingCardHeroData {
                    tier: HeroTier::S,
                    win_rate: 50.0 + (hero_id - 100) as f64,
                    name_zh: format!("H{hero_id}"),
                })
            },
        );

        assert_eq!(mapped.len(), 3);
        assert_eq!(mapped[0].hero.as_ref().map(|h| h.name_zh.as_str()), Some("H103"));
        assert_eq!(mapped[0].summoner.status, SummonerRatingStatus::Private);
        assert_eq!(mapped[0].summoner.premade_group_id, None);

        assert_eq!(mapped[1].hero.as_ref().map(|h| h.name_zh.as_str()), Some("H101"));
        assert_eq!(mapped[1].summoner.score, 88);
        assert_eq!(mapped[1].summoner.streak, 5);
        assert_eq!(mapped[1].summoner.premade_group_id, Some(1));
        assert!(mapped[1].summoner.is_premade_last_mvp);

        assert_eq!(mapped[2].hero.as_ref().map(|h| h.name_zh.as_str()), Some("H102"));
        assert_eq!(mapped[2].summoner.score, 62);
        assert_eq!(mapped[2].summoner.streak, -3);
        assert_eq!(mapped[2].summoner.premade_group_id, Some(1));
        assert!(!mapped[2].summoner.is_premade_last_mvp);
    }
}
