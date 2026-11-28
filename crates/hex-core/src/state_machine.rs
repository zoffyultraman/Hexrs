//! Background Tokio runtime & state machine coordinating process polling, LCU WS/REST, and Live API.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use futures_util::future::join_all;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle as TokioJoinHandle;
use tokio::time::{interval, MissedTickBehavior};
use tracing::{debug, info, warn};

use crate::lcu::{GameflowSessionSnapshot, LcuClient, LcuWsEvent};
use crate::live::LiveClient;
use crate::models::{
    AppEvent, AppMode, ChampSelectState, LiveGameState, LoadingPlayerSlot, LoadingScreenSnapshot,
    LoadingTeamSide, RatingTitlesConfig, TeamSlot,
};
use crate::player_rating::{analyze_team_ratings, MatchFetchResult};
use crate::process::{LcuCredentials, ProcessMonitor, ProcessSnapshot};
use crate::sgp::{get_sgp_match_history_url, get_sgp_server_id, SgpClient};

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(1500);
const CHAMP_SELECT_REST_POLL_INTERVAL: Duration = Duration::from_millis(1200);
const LIVE_API_POLL_INTERVAL: Duration = Duration::from_millis(600);
const LIVE_API_IDLE_TICK_DIVIDER: u8 = 3;
const MATCH_CACHE_TTL: Duration = Duration::from_secs(600);
const MATCH_CACHE_MAX_ENTRIES: usize = 64;

/// Diagnostic snapshot exposed via `CoreHandle::diagnostics()` for UI/tray inspection.
#[derive(Debug, Clone, Default)]
pub struct CoreDiagnostics {
    pub mode: AppMode,
    pub lcu_pid: Option<u32>,
    pub lcu_port: Option<u16>,
    pub lcu_connected_ws: bool,
    pub in_champ_select: bool,
    pub game_pid: Option<u32>,
    pub game_running: bool,
    pub live_api_active: bool,
    pub is_elevated: bool,
    pub lcu_diagnostic: Option<String>,
}

#[derive(Debug, Default)]
struct CoreSharedState {
    mode: AppMode,
    snapshot: ProcessSnapshot,
    lcu_connected: bool,
    in_champ_select: bool,
    live_api_active: bool,
    last_lcu_error: Option<String>,
}

/// Control handle for querying core state and cleanly shutting down the background service.
pub struct CoreHandle {
    shutdown_tx: watch::Sender<bool>,
    shared: Arc<RwLock<CoreSharedState>>,
    join_handle: Option<JoinHandle<()>>,
}

impl CoreHandle {
    /// Returns the active overlay operating mode.
    pub fn current_mode(&self) -> AppMode {
        self.shared
            .read()
            .map(|guard| guard.mode)
            .unwrap_or_default()
    }

    /// Returns the latest observed process snapshot (`lcu_pid`, `game_pid`, etc.).
    pub fn process_snapshot(&self) -> ProcessSnapshot {
        self.shared
            .read()
            .map(|guard| guard.snapshot.clone())
            .unwrap_or_default()
    }

    /// Returns the currently observed `LeagueClientUx.exe` PID, if running.
    pub fn lcu_pid(&self) -> Option<u32> {
        self.shared
            .read()
            .ok()
            .and_then(|guard| guard.snapshot.lcu_pid)
    }

    /// Returns the currently observed `League of Legends.exe` PID, if running.
    pub fn game_pid(&self) -> Option<u32> {
        self.shared
            .read()
            .ok()
            .and_then(|guard| guard.snapshot.game_pid)
    }

    /// Returns whether the LCU WebSocket is currently connected.
    pub fn is_lcu_connected(&self) -> bool {
        self.shared
            .read()
            .map(|guard| guard.lcu_connected)
            .unwrap_or(false)
    }

    /// Returns full runtime diagnostics for debugging connection/mode state.
    pub fn diagnostics(&self) -> CoreDiagnostics {
        self.shared
            .read()
            .map(|guard| {
                let lcu_port = guard.snapshot.lcu.as_ref().map(|c| c.port);
                let lcu_diagnostic = guard.snapshot.lcu_diagnostic.clone().or_else(|| {
                    if !guard.in_champ_select && lcu_port.is_some() {
                        guard.last_lcu_error.clone()
                    } else {
                        None
                    }
                });

                CoreDiagnostics {
                    mode: guard.mode,
                    lcu_pid: guard.snapshot.lcu_pid,
                    lcu_port,
                    lcu_connected_ws: guard.lcu_connected,
                    in_champ_select: guard.in_champ_select,
                    game_pid: guard.snapshot.game_pid,
                    game_running: guard.snapshot.game_running,
                    live_api_active: guard.live_api_active,
                    is_elevated: guard.snapshot.is_elevated,
                    lcu_diagnostic,
                }
            })
            .unwrap_or_default()
    }

    /// Signals the background Tokio runtime to stop and joins its worker thread.
    pub fn shutdown(&mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for CoreHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Spawns and owns the background `hex-core` coordinator service.
pub struct CoreRuntime;

impl CoreRuntime {
    /// Starts the background `hex-core` runtime on a dedicated OS thread with a 2-thread Tokio pool.
    pub fn spawn() -> Result<(Receiver<AppEvent>, CoreHandle)> {
        let (event_tx, event_rx) = unbounded::<AppEvent>();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let shared = Arc::new(RwLock::new(CoreSharedState::default()));
        let shared_clone = Arc::clone(&shared);

        let join_handle = thread::Builder::new()
            .name("hex-core-runtime".to_owned())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(err) => {
                        warn!(error = %err, "failed to create hex-core tokio runtime");
                        return;
                    }
                };

                rt.block_on(run_coordinator(event_tx, shutdown_rx, shared_clone));
            })
            .context("failed to spawn hex-core-runtime thread")?;

        Ok((
            event_rx,
            CoreHandle {
                shutdown_tx,
                shared,
                join_handle: Some(join_handle),
            },
        ))
    }
}

/// Convenience entrypoint starting the `hex-core` service and returning `(Receiver<AppEvent>, CoreHandle)`.
pub fn spawn_core_service() -> Result<(Receiver<AppEvent>, CoreHandle)> {
    CoreRuntime::spawn()
}

struct ChampPollOutcome {
    session: Result<Option<ChampSelectState>>,
    phase: Option<String>,
}

struct CoordinatorChannels {
    ws_event_tx: mpsc::UnboundedSender<LcuWsEvent>,
    gameflow_tx: mpsc::UnboundedSender<Option<(String, Option<i32>)>>,
    subset_tx: mpsc::UnboundedSender<Vec<i32>>,
    ratings_tx: mpsc::UnboundedSender<Vec<(String, MatchFetchResult)>>,
    loading_session_tx: mpsc::UnboundedSender<Option<GameflowSessionSnapshot>>,
    loading_ratings_tx: mpsc::UnboundedSender<Vec<(String, MatchFetchResult)>>,
    local_puuid_tx: mpsc::UnboundedSender<Option<String>>,
}

struct CoordinatorState {
    event_tx: Sender<AppEvent>,
    shared: Arc<RwLock<CoreSharedState>>,
    mode: AppMode,
    snapshot: ProcessSnapshot,
    active_creds: Option<LcuCredentials>,
    active_lcu: Option<LcuClient>,
    lcu_connected: bool,
    in_champ_select: bool,
    live_api_active: bool,
    last_lcu_error: Option<String>,
    cached_game_mode: Option<String>,
    cached_queue_id: Option<i32>,
    cached_subset_champions: Vec<i32>,
    last_champ_select: Option<ChampSelectState>,
    local_puuid: Option<String>,
    last_ally_puuids: Vec<String>,
    loading_session: Option<GameflowSessionSnapshot>,
    loading_active: bool,
    loading_session_fetch_in_flight: bool,
    loading_ratings_fetch_in_flight: bool,
    last_loading_rated_key: Vec<String>,
    live_match_started: bool,
    gameflow_fetch_in_flight: bool,
    subset_fetch_in_flight: bool,
    match_cache: HashMap<String, (Instant, MatchFetchResult)>,
    last_rated_team_key: Vec<(i32, String)>,
    ratings_fetch_in_flight: bool,
    champ_poll_in_flight: bool,
    live_poll_in_flight: bool,
    ws_shutdown_tx: Option<watch::Sender<bool>>,
    ws_task: Option<TokioJoinHandle<()>>,
}

impl CoordinatorState {
    fn new(event_tx: Sender<AppEvent>, shared: Arc<RwLock<CoreSharedState>>) -> Self {
        Self {
            event_tx,
            shared,
            mode: AppMode::Dormant,
            snapshot: ProcessSnapshot::default(),
            active_creds: None,
            active_lcu: None,
            lcu_connected: false,
            in_champ_select: false,
            live_api_active: false,
            last_lcu_error: None,
            cached_game_mode: None,
            cached_queue_id: None,
            cached_subset_champions: Vec::new(),
            last_champ_select: None,
            local_puuid: None,
            last_ally_puuids: Vec::new(),
            loading_session: None,
            loading_active: false,
            loading_session_fetch_in_flight: false,
            loading_ratings_fetch_in_flight: false,
            last_loading_rated_key: Vec::new(),
            live_match_started: false,
            gameflow_fetch_in_flight: false,
            subset_fetch_in_flight: false,
            match_cache: HashMap::new(),
            last_rated_team_key: Vec::new(),
            ratings_fetch_in_flight: false,
            champ_poll_in_flight: false,
            live_poll_in_flight: false,
            ws_shutdown_tx: None,
            ws_task: None,
        }
    }

    fn stop_lcu_ws(&mut self) {
        if let Some(tx) = self.ws_shutdown_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(task) = self.ws_task.take() {
            task.abort();
        }
    }

    fn sync_shared_and_mode(&mut self) {
        let next_mode = compute_mode(
            self.snapshot.game_running || self.live_api_active,
            self.active_lcu.is_some(),
            self.in_champ_select,
        );
        let mode_changed = next_mode != self.mode;
        self.mode = next_mode;

        if let Ok(mut guard) = self.shared.write() {
            guard.mode = self.mode;
            guard.snapshot = self.snapshot.clone();
            guard.lcu_connected = self.lcu_connected;
            guard.in_champ_select = self.in_champ_select;
            guard.live_api_active = self.live_api_active;
            guard.last_lcu_error = self.last_lcu_error.clone();
        }

        if mode_changed {
            info!(
                ?next_mode,
                game_running = self.snapshot.game_running,
                live_api_active = self.live_api_active,
                in_champ_select = self.in_champ_select,
                "overlay state machine mode transition"
            );
            let _ = self.event_tx.send(AppEvent::ModeChanged(next_mode));
        }
    }

    fn handle_process_snapshot(
        &mut self,
        next: ProcessSnapshot,
        channels: &CoordinatorChannels,
    ) {
        let prev_game_running = self.snapshot.game_running;
        let creds_changed = next.lcu != self.active_creds;
        self.snapshot = next;

        if prev_game_running && !self.snapshot.game_running {
            self.live_match_started = false;
            self.clear_loading_screen();
        }

        if creds_changed {
            self.stop_lcu_ws();
            let was_connected = self.lcu_connected;
            self.lcu_connected = false;
            self.in_champ_select = false;
            self.last_lcu_error = None;
            self.cached_game_mode = None;
            self.cached_queue_id = None;
            self.cached_subset_champions.clear();
            self.last_champ_select = None;
            self.local_puuid = None;
            self.last_ally_puuids.clear();
            self.clear_loading_screen();
            self.loading_session_fetch_in_flight = false;
            self.loading_ratings_fetch_in_flight = false;
            self.live_match_started = false;
            self.gameflow_fetch_in_flight = false;
            self.subset_fetch_in_flight = false;
            self.match_cache.clear();
            self.last_rated_team_key.clear();
            self.ratings_fetch_in_flight = false;
            self.champ_poll_in_flight = false;
            self.active_lcu = None;
            self.active_creds = self.snapshot.lcu.clone();

            if was_connected {
                let _ = self.event_tx.send(AppEvent::LcuConnectionChanged(false));
            }

            if let Some(creds) = self.active_creds.clone() {
                match LcuClient::new(creds.port, &creds.token) {
                    Ok(client) => {
                        let (ws_stop_tx, ws_stop_rx) = watch::channel(false);
                        let ws_client = client.clone();
                        let tx = channels.ws_event_tx.clone();
                        let ws_task = tokio::spawn(async move {
                            ws_client.run_ws_loop(tx, ws_stop_rx).await;
                        });
                        self.ws_shutdown_tx = Some(ws_stop_tx);
                        self.ws_task = Some(ws_task);

                        let fetch_client = client.clone();
                        let tx_init = channels.ws_event_tx.clone();
                        tokio::spawn(async move {
                            emit_initial_champ_select(&fetch_client, &tx_init, true).await;
                        });

                        let puuid_client = client.clone();
                        let puuid_tx = channels.local_puuid_tx.clone();
                        tokio::spawn(async move {
                            let resolved = puuid_client
                                .get_current_summoner_puuid()
                                .await
                                .ok()
                                .flatten();
                            let _ = puuid_tx.send(resolved);
                        });

                        self.active_lcu = Some(client);
                    }
                    Err(err) => {
                        warn!(error = %err, "failed to initialize LcuClient");
                        self.last_lcu_error = Some(format!("LcuClient 初始化失败: {err:#}"));
                    }
                }
            }
        }

        if self.snapshot.game_running
            && !self.live_match_started
            && self.loading_session.is_none()
            && !self.loading_session_fetch_in_flight
        {
            self.trigger_loading_session_fetch(channels);
        }

        self.sync_shared_and_mode();
    }

    fn normalize_champ_select(&mut self, mut state: ChampSelectState) -> ChampSelectState {
        if state.game_mode.is_empty() {
            if let Some(mode) = &self.cached_game_mode {
                state.game_mode = mode.clone();
            }
        } else {
            self.cached_game_mode = Some(state.game_mode.clone());
        }

        if state.queue_id.is_none() {
            state.queue_id = self.cached_queue_id;
        } else {
            self.cached_queue_id = state.queue_id;
        }

        if state.local_champion_id > 0 {
            state.subset_champions.clear();
        } else if !state.subset_champions.is_empty() {
            self.cached_subset_champions = state.subset_champions.clone();
        } else if !self.cached_subset_champions.is_empty() {
            state.subset_champions = self.cached_subset_champions.clone();
        }

        state
    }

    fn clear_champ_select_state(&mut self) {
        self.in_champ_select = false;
        self.cached_game_mode = None;
        self.cached_queue_id = None;
        self.cached_subset_champions.clear();
        self.last_champ_select = None;
        self.gameflow_fetch_in_flight = false;
        self.subset_fetch_in_flight = false;
        self.last_rated_team_key.clear();
    }

    fn clear_loading_screen(&mut self) {
        let was_active = self.loading_active || self.loading_session.is_some();
        self.loading_session = None;
        self.loading_active = false;
        self.last_loading_rated_key.clear();
        if was_active {
            let _ = self.event_tx.send(AppEvent::LoadingScreenCleared);
        }
    }

    fn trigger_loading_session_fetch(&mut self, channels: &CoordinatorChannels) {
        if self.live_match_started || self.loading_session_fetch_in_flight {
            return;
        }
        let Some(client) = self.active_lcu.clone() else {
            return;
        };
        self.loading_session_fetch_in_flight = true;
        let tx = channels.loading_session_tx.clone();
        tokio::spawn(async move {
            let sess = client.get_gameflow_session().await.ok().flatten();
            let _ = tx.send(sess);
        });
    }

    fn handle_ws_event(&mut self, event: LcuWsEvent, channels: &CoordinatorChannels) {
        match event {
            LcuWsEvent::Connected => {
                self.last_lcu_error = None;
                if !self.lcu_connected {
                    self.lcu_connected = true;
                    let _ = self.event_tx.send(AppEvent::LcuConnectionChanged(true));
                }
                if let Some(client) = self.active_lcu.clone() {
                    let tx = channels.ws_event_tx.clone();
                    let init_client = client.clone();
                    tokio::spawn(async move {
                        emit_initial_champ_select(&init_client, &tx, true).await;
                    });
                    if self.local_puuid.is_none() {
                        let puuid_tx = channels.local_puuid_tx.clone();
                        tokio::spawn(async move {
                            let resolved = client.get_current_summoner_puuid().await.ok().flatten();
                            let _ = puuid_tx.send(resolved);
                        });
                    }
                }
            }
            LcuWsEvent::Disconnected => {
                if self.lcu_connected {
                    self.lcu_connected = false;
                    let _ = self.event_tx.send(AppEvent::LcuConnectionChanged(false));
                }
            }
            LcuWsEvent::WsError(msg) => {
                self.last_lcu_error = Some(msg);
            }
            LcuWsEvent::GameflowPhase(phase) => {
                debug!(%phase, "LCU gameflow phase changed");
                if phase == "ChampSelect" {
                    self.in_champ_select = true;
                    self.live_match_started = false;
                    self.clear_loading_screen();
                    if self.last_champ_select.is_none() {
                        if let Some(client) = self.active_lcu.clone() {
                            let tx = channels.ws_event_tx.clone();
                            tokio::spawn(async move {
                                emit_initial_champ_select(&client, &tx, false).await;
                            });
                        }
                    }
                } else if is_loading_phase(&phase) {
                    self.clear_champ_select_state();
                    if !self.live_match_started {
                        self.trigger_loading_session_fetch(channels);
                    }
                } else if is_post_or_lobby_phase(&phase) {
                    self.clear_champ_select_state();
                    self.live_match_started = false;
                    self.clear_loading_screen();
                }
            }
            LcuWsEvent::GameflowSessionUpdated(session) => {
                debug!(
                    phase = %session.phase,
                    game_id = session.game_id,
                    team_one = session.team_one.len(),
                    team_two = session.team_two.len(),
                    "LCU gameflow session updated"
                );
                if session.phase == "ChampSelect" {
                    self.live_match_started = false;
                    self.clear_loading_screen();
                } else if is_loading_phase(&session.phase) {
                    self.clear_champ_select_state();
                    self.apply_loading_session(session, channels);
                } else if is_post_or_lobby_phase(&session.phase) {
                    self.clear_champ_select_state();
                    self.live_match_started = false;
                    self.clear_loading_screen();
                }
            }
            LcuWsEvent::ChampSelectUpdated(raw_state) => {
                self.in_champ_select = true;
                self.last_lcu_error = None;
                self.live_match_started = false;
                self.clear_loading_screen();
                let state = self.normalize_champ_select(raw_state);

                if let Some(local_slot) = state
                    .my_team
                    .iter()
                    .find(|s| s.cell_id == state.local_player_cell_id && !s.puuid.is_empty())
                {
                    self.local_puuid = Some(local_slot.puuid.clone());
                }
                let ally_puuids: Vec<String> = state
                    .my_team
                    .iter()
                    .filter(|s| !s.puuid.is_empty())
                    .map(|s| s.puuid.clone())
                    .collect();
                if !ally_puuids.is_empty() {
                    self.last_ally_puuids = ally_puuids;
                }

                if state.game_mode.is_empty() && !self.gameflow_fetch_in_flight {
                    if let Some(client) = self.active_lcu.clone() {
                        self.gameflow_fetch_in_flight = true;
                        let gf_tx = channels.gameflow_tx.clone();
                        tokio::spawn(async move {
                            let resolved = client
                                .get_current_game_mode_and_queue()
                                .await
                                .ok()
                                .flatten();
                            let _ = gf_tx.send(resolved);
                        });
                    }
                }

                if state.local_champion_id == 0
                    && self.cached_subset_champions.is_empty()
                    && !self.subset_fetch_in_flight
                {
                    if let Some(client) = self.active_lcu.clone() {
                        self.subset_fetch_in_flight = true;
                        let stx = channels.subset_tx.clone();
                        tokio::spawn(async move {
                            let ids = client.get_subset_champion_list().await.unwrap_or_default();
                            let _ = stx.send(ids);
                        });
                    }
                }

                self.last_champ_select = Some(state.clone());
                let _ = self.event_tx.send(AppEvent::ChampSelectUpdated(state));
                self.maybe_refresh_team_ratings(channels);
            }
            LcuWsEvent::SubsetChampionsUpdated(ids) => {
                self.apply_subset_champions(ids);
            }
            LcuWsEvent::ChampSelectEnded => {
                self.clear_champ_select_state();
            }
            LcuWsEvent::EndOfGameCaptured(stats) => {
                let _ = self.event_tx.send(AppEvent::EndOfGameCaptured(stats));
            }
        }

        self.sync_shared_and_mode();
    }

    fn handle_champ_poll_result(
        &mut self,
        outcome: ChampPollOutcome,
        channels: &CoordinatorChannels,
    ) {
        self.champ_poll_in_flight = false;
        match outcome.session {
            Ok(Some(raw_session)) => {
                self.last_lcu_error = None;
                let normalized = self.normalize_champ_select(raw_session);
                if Some(&normalized) != self.last_champ_select.as_ref() {
                    self.handle_ws_event(LcuWsEvent::ChampSelectUpdated(normalized), channels);
                }
            }
            Ok(None) => {
                let cleared_err = self.last_lcu_error.take().is_some();
                let mut synced = false;
                match outcome.phase.as_deref() {
                    Some("ChampSelect") => {
                        if !self.in_champ_select {
                            info!("LCU gameflow-phase=ChampSelect detected via REST fallback");
                            self.in_champ_select = true;
                            self.live_match_started = false;
                            self.clear_loading_screen();
                            self.sync_shared_and_mode();
                            synced = true;
                        }
                    }
                    Some(phase) if is_loading_phase(phase) => {
                        if self.in_champ_select {
                            self.handle_ws_event(LcuWsEvent::ChampSelectEnded, channels);
                            synced = true;
                        }
                        if !self.live_match_started && self.loading_session.is_none() {
                            self.trigger_loading_session_fetch(channels);
                        }
                    }
                    Some(phase) if is_post_or_lobby_phase(phase) => {
                        self.live_match_started = false;
                        self.clear_loading_screen();
                        if self.in_champ_select {
                            self.handle_ws_event(LcuWsEvent::ChampSelectEnded, channels);
                            synced = true;
                        }
                    }
                    _ => {
                        if self.in_champ_select && outcome.phase.is_none() {
                            self.handle_ws_event(LcuWsEvent::ChampSelectEnded, channels);
                            synced = true;
                        }
                    }
                }
                if cleared_err && !synced {
                    self.sync_shared_and_mode();
                }
            }
            Err(err) => {
                warn!(error = %err, "LCU REST champ select poll fallback failed");
                self.last_lcu_error = Some(format!("LCU REST: {err:#}"));
                self.sync_shared_and_mode();
            }
        }
    }

    fn handle_live_poll_result(&mut self, result: Result<Option<LiveGameState>>) {
        self.live_poll_in_flight = false;
        match result {
            Ok(Some(live_state)) => {
                if !self.live_api_active {
                    info!(
                        game_mode = %live_state.game_mode,
                        "Live Client Data API (127.0.0.1:2999) detected active match"
                    );
                    self.live_api_active = true;
                }
                if live_state.game_time_secs > 0.0 {
                    self.live_match_started = true;
                    self.clear_loading_screen();
                }
                let _ = self.event_tx.send(AppEvent::LiveGameUpdated(live_state));
                self.sync_shared_and_mode();
            }
            Ok(None) => {
                if !self.snapshot.game_running {
                    self.live_match_started = false;
                    self.clear_loading_screen();
                }
                if self.live_api_active {
                    self.live_api_active = false;
                    self.sync_shared_and_mode();
                }
            }
            Err(err) => {
                debug!(error = %err, "Live API poll failed");
                if !self.snapshot.game_running {
                    self.live_match_started = false;
                    self.clear_loading_screen();
                }
                if self.live_api_active {
                    self.live_api_active = false;
                    self.sync_shared_and_mode();
                }
            }
        }
    }

    fn handle_subset_resolved(&mut self, ids: Vec<i32>) {
        self.subset_fetch_in_flight = false;
        self.apply_subset_champions(ids);
    }

    fn apply_subset_champions(&mut self, ids: Vec<i32>) {
        self.cached_subset_champions = ids.clone();
        if self.in_champ_select {
            if let Some(mut session) = self.last_champ_select.clone() {
                if session.local_champion_id == 0 && session.subset_champions != ids {
                    session.subset_champions = ids;
                    debug!(ids = ?session.subset_champions, "resolved champ select subset cards");
                    self.last_champ_select = Some(session.clone());
                    let _ = self.event_tx.send(AppEvent::ChampSelectUpdated(session));
                }
            }
        }
    }

    fn prune_match_cache(&mut self, now: Instant) {
        self.match_cache
            .retain(|_, (ts, _)| now.saturating_duration_since(*ts) < MATCH_CACHE_TTL);
    }

    fn insert_match_cache(&mut self, puuid: String, result: MatchFetchResult, now: Instant) {
        if !matches!(result, MatchFetchResult::Ok(_) | MatchFetchResult::Private) {
            return;
        }
        self.prune_match_cache(now);
        if !self.match_cache.contains_key(&puuid) && self.match_cache.len() >= MATCH_CACHE_MAX_ENTRIES
        {
            if let Some(oldest_key) = self
                .match_cache
                .iter()
                .min_by_key(|(_, (ts, _))| *ts)
                .map(|(k, _)| k.clone())
            {
                self.match_cache.remove(&oldest_key);
            }
        }
        self.match_cache.insert(puuid, (now, result));
    }

    fn maybe_refresh_team_ratings(&mut self, channels: &CoordinatorChannels) {
        if !self.in_champ_select {
            return;
        }
        let Some(session) = self.last_champ_select.clone() else {
            return;
        };
        if session.my_team.is_empty() {
            return;
        }
        let team_key: Vec<(i32, String)> = session
            .my_team
            .iter()
            .map(|s| (s.cell_id, s.puuid.clone()))
            .collect();
        if !team_key.iter().any(|(_, p)| !p.is_empty()) || team_key == self.last_rated_team_key {
            return;
        }

        let now = Instant::now();
        self.prune_match_cache(now);

        let mut cached_map: HashMap<String, MatchFetchResult> = HashMap::new();
        let mut uncached_puuids: Vec<String> = Vec::new();
        for slot in &session.my_team {
            if slot.puuid.is_empty() {
                continue;
            }
            if let Some((_, res)) = self.match_cache.get(&slot.puuid) {
                cached_map.insert(slot.puuid.clone(), res.clone());
            } else if !uncached_puuids.contains(&slot.puuid) {
                uncached_puuids.push(slot.puuid.clone());
            }
        }

        if uncached_puuids.is_empty() {
            self.last_rated_team_key = team_key;
            let snapshot =
                analyze_team_ratings(&session.my_team, &cached_map, &RatingTitlesConfig::default());
            let _ = self.event_tx.send(AppEvent::TeamRatingsUpdated(snapshot));
            return;
        }

        if self.ratings_fetch_in_flight {
            return;
        }

        let Some(client) = self.active_lcu.clone() else {
            return;
        };

        self.last_rated_team_key = team_key;
        self.ratings_fetch_in_flight = true;

        let loading_snap =
            analyze_team_ratings(&session.my_team, &cached_map, &RatingTitlesConfig::default());
        let _ = self.event_tx.send(AppEvent::TeamRatingsUpdated(loading_snap));

        let creds_region = self.active_creds.as_ref().and_then(|c| c.region.clone());
        let creds_rso = self
            .active_creds
            .as_ref()
            .and_then(|c| c.rso_platform_id.clone());
        spawn_sgp_match_fetch(
            client,
            creds_region,
            creds_rso,
            uncached_puuids,
            5,
            channels.ratings_tx.clone(),
        );
    }

    fn handle_ratings_resolved(
        &mut self,
        results: Vec<(String, MatchFetchResult)>,
        channels: &CoordinatorChannels,
    ) {
        self.ratings_fetch_in_flight = false;
        let now = Instant::now();
        let mut fresh_results: HashMap<String, MatchFetchResult> = HashMap::new();
        for (puuid, res) in results {
            fresh_results.insert(puuid.clone(), res.clone());
            self.insert_match_cache(puuid, res, now);
        }

        if self.in_champ_select {
            if let Some(session) = self.last_champ_select.clone() {
                let mut match_map: HashMap<String, MatchFetchResult> = HashMap::new();
                for slot in &session.my_team {
                    if slot.puuid.is_empty() {
                        continue;
                    }
                    if let Some(res) = fresh_results.get(&slot.puuid) {
                        match_map.insert(slot.puuid.clone(), res.clone());
                    } else if let Some((_, res)) = self.match_cache.get(&slot.puuid) {
                        match_map.insert(slot.puuid.clone(), res.clone());
                    }
                }
                let snapshot = analyze_team_ratings(
                    &session.my_team,
                    &match_map,
                    &RatingTitlesConfig::default(),
                );
                let _ = self.event_tx.send(AppEvent::TeamRatingsUpdated(snapshot));
            }
            self.maybe_refresh_team_ratings(channels);
        }
    }

    fn build_loading_screen_snapshot(
        &mut self,
        session: &GameflowSessionSnapshot,
        fresh_results: Option<&HashMap<String, MatchFetchResult>>,
    ) -> LoadingScreenSnapshot {
        let now = Instant::now();
        self.prune_match_cache(now);

        let mut match_map: HashMap<String, MatchFetchResult> = HashMap::new();
        for slot in session.team_one.iter().chain(session.team_two.iter()) {
            if slot.puuid.is_empty() {
                continue;
            }
            if let Some(fresh) = fresh_results.and_then(|m| m.get(&slot.puuid)) {
                match_map.insert(slot.puuid.clone(), fresh.clone());
            } else if let Some((_, cached)) = self.match_cache.get(&slot.puuid) {
                match_map.insert(slot.puuid.clone(), cached.clone());
            }
        }

        let local_side = determine_local_side(
            self.local_puuid.as_deref(),
            &self.last_ally_puuids,
            &session.team_one,
            &session.team_two,
        );
        let team_one_slots = loading_slots_to_team_slots(&session.team_one);
        let team_two_slots = loading_slots_to_team_slots(&session.team_two);
        let cfg = RatingTitlesConfig::default();
        let team_one_ratings = analyze_team_ratings(&team_one_slots, &match_map, &cfg);
        let team_two_ratings = analyze_team_ratings(&team_two_slots, &match_map, &cfg);

        LoadingScreenSnapshot {
            game_id: session.game_id,
            game_mode: session.game_mode.clone(),
            queue_id: session.queue_id,
            local_side,
            team_one: session.team_one.clone(),
            team_two: session.team_two.clone(),
            team_one_ratings,
            team_two_ratings,
        }
    }

    fn apply_loading_session(
        &mut self,
        session: GameflowSessionSnapshot,
        channels: &CoordinatorChannels,
    ) {
        if self.live_match_started || (session.team_one.is_empty() && session.team_two.is_empty()) {
            return;
        }

        self.loading_session = Some(session.clone());
        self.loading_active = true;

        // 0ms instant emit: cached allies resolve immediately while uncached enemies show Loading.
        let snapshot = self.build_loading_screen_snapshot(&session, None);
        let _ = self.event_tx.send(AppEvent::LoadingScreenUpdated(snapshot));

        self.maybe_refresh_loading_ratings(channels);
    }

    fn maybe_refresh_loading_ratings(&mut self, channels: &CoordinatorChannels) {
        if !self.loading_active || self.live_match_started {
            return;
        }
        let Some(session) = self.loading_session.clone() else {
            return;
        };

        let now = Instant::now();
        self.prune_match_cache(now);

        let mut all_puuids: Vec<String> = Vec::new();
        let mut uncached_puuids: Vec<String> = Vec::new();
        for slot in session.team_one.iter().chain(session.team_two.iter()) {
            if slot.puuid.is_empty() {
                continue;
            }
            if !all_puuids.contains(&slot.puuid) {
                all_puuids.push(slot.puuid.clone());
            }
            if !self.match_cache.contains_key(&slot.puuid)
                && !uncached_puuids.contains(&slot.puuid)
            {
                uncached_puuids.push(slot.puuid.clone());
            }
        }

        if uncached_puuids.is_empty() {
            self.last_loading_rated_key = all_puuids;
            return;
        }
        if self.loading_ratings_fetch_in_flight || all_puuids == self.last_loading_rated_key {
            return;
        }

        let Some(client) = self.active_lcu.clone() else {
            return;
        };

        self.last_loading_rated_key = all_puuids;
        self.loading_ratings_fetch_in_flight = true;

        let creds_region = self.active_creds.as_ref().and_then(|c| c.region.clone());
        let creds_rso = self
            .active_creds
            .as_ref()
            .and_then(|c| c.rso_platform_id.clone());
        spawn_sgp_match_fetch(
            client,
            creds_region,
            creds_rso,
            uncached_puuids,
            10,
            channels.loading_ratings_tx.clone(),
        );
    }

    fn handle_loading_session_resolved(
        &mut self,
        session: Option<GameflowSessionSnapshot>,
        channels: &CoordinatorChannels,
    ) {
        self.loading_session_fetch_in_flight = false;
        if let Some(sess) = session {
            if is_loading_phase(&sess.phase)
                || (!self.live_match_started
                    && self.snapshot.game_running
                    && (!sess.team_one.is_empty() || !sess.team_two.is_empty()))
            {
                self.apply_loading_session(sess, channels);
            }
        }
    }

    fn handle_loading_ratings_resolved(
        &mut self,
        results: Vec<(String, MatchFetchResult)>,
        channels: &CoordinatorChannels,
    ) {
        self.loading_ratings_fetch_in_flight = false;
        let now = Instant::now();
        let mut fresh_results: HashMap<String, MatchFetchResult> = HashMap::new();
        for (puuid, res) in results {
            fresh_results.insert(puuid.clone(), res.clone());
            self.insert_match_cache(puuid, res, now);
        }

        if self.loading_active && !self.live_match_started {
            if let Some(session) = self.loading_session.clone() {
                let snapshot = self.build_loading_screen_snapshot(&session, Some(&fresh_results));
                let _ = self.event_tx.send(AppEvent::LoadingScreenUpdated(snapshot));
            }
            self.maybe_refresh_loading_ratings(channels);
        }
    }

    fn handle_local_puuid_resolved(&mut self, puuid: Option<String>) {
        if let Some(p) = puuid.filter(|s| !s.is_empty()) {
            let changed = self.local_puuid.as_deref() != Some(p.as_str());
            self.local_puuid = Some(p);
            if changed && self.loading_active && !self.live_match_started {
                if let Some(session) = self.loading_session.clone() {
                    let snapshot = self.build_loading_screen_snapshot(&session, None);
                    let _ = self.event_tx.send(AppEvent::LoadingScreenUpdated(snapshot));
                }
            }
        }
    }

    fn handle_gameflow_resolved(&mut self, resolved: Option<(String, Option<i32>)>) {
        self.gameflow_fetch_in_flight = false;
        if let Some((mode, queue_id)) = resolved {
            self.cached_game_mode = Some(mode.clone());
            if queue_id.is_some() {
                self.cached_queue_id = queue_id;
            }
            if self.in_champ_select {
                if let Some(mut session) = self.last_champ_select.clone() {
                    let mut changed = false;
                    if session.game_mode.is_empty() {
                        session.game_mode = mode;
                        changed = true;
                    }
                    if session.queue_id.is_none() && queue_id.is_some() {
                        session.queue_id = queue_id;
                        changed = true;
                    }
                    if changed {
                        self.last_champ_select = Some(session.clone());
                        let _ = self.event_tx.send(AppEvent::ChampSelectUpdated(session));
                    }
                }
            }
        }
    }
}

fn is_loading_phase(phase: &str) -> bool {
    matches!(phase, "GameStart" | "InProgress" | "Reconnect")
}

fn is_post_or_lobby_phase(phase: &str) -> bool {
    matches!(
        phase,
        "None" | "Lobby" | "Matchmaking" | "EndOfGame" | "PreEndOfGame" | "WaitingForStats"
    )
}

fn determine_local_side(
    local_puuid: Option<&str>,
    last_ally_puuids: &[String],
    team_one: &[LoadingPlayerSlot],
    team_two: &[LoadingPlayerSlot],
) -> LoadingTeamSide {
    if let Some(lp) = local_puuid.filter(|s| !s.is_empty()) {
        if team_one.iter().any(|s| s.puuid == lp) {
            return LoadingTeamSide::Blue;
        }
        if team_two.iter().any(|s| s.puuid == lp) {
            return LoadingTeamSide::Red;
        }
    }
    if !last_ally_puuids.is_empty() {
        let one_hits = team_one
            .iter()
            .filter(|s| !s.puuid.is_empty() && last_ally_puuids.iter().any(|a| a == &s.puuid))
            .count();
        let two_hits = team_two
            .iter()
            .filter(|s| !s.puuid.is_empty() && last_ally_puuids.iter().any(|a| a == &s.puuid))
            .count();
        if two_hits > one_hits {
            return LoadingTeamSide::Red;
        }
        if one_hits > 0 {
            return LoadingTeamSide::Blue;
        }
    }
    LoadingTeamSide::Blue
}

fn loading_slots_to_team_slots(slots: &[LoadingPlayerSlot]) -> Vec<TeamSlot> {
    slots
        .iter()
        .map(|s| TeamSlot {
            cell_id: s.slot_index as i32,
            champion_id: s.champion_id,
            pick_intent: 0,
            assigned_position: String::new(),
            puuid: s.puuid.clone(),
        })
        .collect()
}

fn spawn_sgp_match_fetch(
    client: LcuClient,
    creds_region: Option<String>,
    creds_rso: Option<String>,
    puuids: Vec<String>,
    max_players: usize,
    tx: mpsc::UnboundedSender<Vec<(String, MatchFetchResult)>>,
) {
    let to_fetch: Vec<String> = puuids.into_iter().take(max_players).collect();
    tokio::spawn(async move {
        let mut region = creds_region;
        let mut rso = creds_rso;
        let need_locale = region.is_none()
            || (region
                .as_deref()
                .map(|r| r.eq_ignore_ascii_case("TENCENT"))
                .unwrap_or(false)
                && rso.is_none());
        if need_locale {
            if let Ok(Some((loc_region, loc_rso))) = client.get_region_locale().await {
                if region.is_none() {
                    region = Some(loc_region);
                }
                if rso.is_none() {
                    rso = loc_rso;
                }
            }
        }

        let sgp_base = region
            .as_deref()
            .and_then(|r| get_sgp_server_id(r, rso.as_deref()))
            .and_then(|sid| get_sgp_match_history_url(&sid));
        let token = if sgp_base.is_some() {
            client.get_entitlements_token().await.ok().flatten()
        } else {
            None
        };
        let sgp_client = if sgp_base.is_some() && token.is_some() {
            SgpClient::new().ok()
        } else {
            None
        };

        let futs = to_fetch.into_iter().map(|puuid| {
            let lcu = client.clone();
            let sgp = sgp_client.clone();
            let tok = token.clone();
            async move {
                let res = if let (Some(base), Some(sgp_c), Some(access_tok)) =
                    (sgp_base, sgp.as_ref(), tok.as_deref())
                {
                    match sgp_c.fetch_kiwi_matches(base, access_tok, &puuid, 20).await {
                        MatchFetchResult::Unavailable(reason) => {
                            debug!(%puuid, %reason, "SGP unavailable; falling back to LCU match history");
                            lcu.get_kiwi_matches_fallback(&puuid).await
                        }
                        other => other,
                    }
                } else {
                    lcu.get_kiwi_matches_fallback(&puuid).await
                };
                (puuid, res)
            }
        });

        let results = join_all(futs).await;
        let _ = tx.send(results);
    });
}

async fn emit_initial_champ_select(
    client: &LcuClient,
    ws_event_tx: &mpsc::UnboundedSender<LcuWsEvent>,
    allow_phase_fallback: bool,
) {
    let mode_and_queue = match client.get_current_game_mode_and_queue().await {
        Ok(info) => info,
        Err(err) => {
            debug!(error = %err, "failed to fetch initial LCU game mode");
            None
        }
    };

    match client.get_champ_select_session().await {
        Ok(Some(mut session)) => {
            if let Some((mode, queue_id)) = mode_and_queue {
                if session.game_mode.is_empty() {
                    session.game_mode = mode;
                }
                if session.queue_id.is_none() {
                    session.queue_id = queue_id;
                }
            }
            if session.local_champion_id == 0 {
                let ids = client.get_subset_champion_list().await.unwrap_or_default();
                if !ids.is_empty() {
                    session.subset_champions = ids;
                }
            }
            let _ = ws_event_tx.send(LcuWsEvent::ChampSelectUpdated(session));
        }
        Ok(None) => {
            if allow_phase_fallback {
                if let Ok(Some(phase)) = client.get_gameflow_phase().await {
                    if phase == "ChampSelect" {
                        let _ = ws_event_tx.send(LcuWsEvent::GameflowPhase(phase));
                    } else if is_loading_phase(&phase) {
                        if let Ok(Some(sess)) = client.get_gameflow_session().await {
                            let _ = ws_event_tx.send(LcuWsEvent::GameflowSessionUpdated(sess));
                        } else {
                            let _ = ws_event_tx.send(LcuWsEvent::GameflowPhase(phase));
                        }
                    }
                }
            }
        }
        Err(err) => {
            warn!(error = %err, "failed to fetch initial LCU champ select session");
            let _ = ws_event_tx.send(LcuWsEvent::WsError(format!("LCU REST: {err:#}")));
        }
    }
}

fn compute_mode(in_game: bool, lcu_active: bool, in_champ_select: bool) -> AppMode {
    if in_game {
        AppMode::InGameOverlay
    } else if lcu_active && in_champ_select {
        AppMode::LobbyDocked
    } else {
        AppMode::Dormant
    }
}

async fn run_coordinator(
    event_tx: Sender<AppEvent>,
    mut shutdown_rx: watch::Receiver<bool>,
    shared: Arc<RwLock<CoreSharedState>>,
) {
    let mut monitor = ProcessMonitor::new();
    let live_client = match LiveClient::new() {
        Ok(client) => Some(client),
        Err(err) => {
            warn!(error = %err, "failed to create LiveClient; in-game telemetry disabled");
            None
        }
    };

    let (ws_event_tx, mut ws_event_rx) = mpsc::unbounded_channel::<LcuWsEvent>();
    let (gameflow_tx, mut gameflow_rx) = mpsc::unbounded_channel::<Option<(String, Option<i32>)>>();
    let (subset_tx, mut subset_rx) = mpsc::unbounded_channel::<Vec<i32>>();
    let (ratings_tx, mut ratings_rx) = mpsc::unbounded_channel::<Vec<(String, MatchFetchResult)>>();
    let (loading_session_tx, mut loading_session_rx) =
        mpsc::unbounded_channel::<Option<GameflowSessionSnapshot>>();
    let (loading_ratings_tx, mut loading_ratings_rx) =
        mpsc::unbounded_channel::<Vec<(String, MatchFetchResult)>>();
    let (local_puuid_tx, mut local_puuid_rx) = mpsc::unbounded_channel::<Option<String>>();
    let (champ_poll_tx, mut champ_poll_rx) = mpsc::unbounded_channel::<ChampPollOutcome>();
    let (live_poll_tx, mut live_poll_rx) =
        mpsc::unbounded_channel::<Result<Option<LiveGameState>>>();

    let channels = CoordinatorChannels {
        ws_event_tx,
        gameflow_tx,
        subset_tx,
        ratings_tx,
        loading_session_tx,
        loading_ratings_tx,
        local_puuid_tx,
    };

    let mut state = CoordinatorState::new(event_tx.clone(), shared);

    // Perform an immediate initial process poll so startup doesn't wait 1.5s.
    let initial_snapshot = monitor.poll();
    state.handle_process_snapshot(initial_snapshot, &channels);

    let mut process_tick = interval(PROCESS_POLL_INTERVAL);
    process_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    process_tick.tick().await;

    let mut champ_poll_tick = interval(CHAMP_SELECT_REST_POLL_INTERVAL);
    champ_poll_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    champ_poll_tick.tick().await;

    let mut live_tick = interval(LIVE_API_POLL_INTERVAL);
    live_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    live_tick.tick().await;
    let mut live_tick_counter: u8 = 0;

    loop {
        tokio::select! {
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
            }
            _ = process_tick.tick() => {
                let snapshot = monitor.poll();
                state.handle_process_snapshot(snapshot, &channels);
            }
            _ = champ_poll_tick.tick() => {
                if state.snapshot.game_running
                    && !state.live_match_started
                    && state.loading_session.is_none()
                    && !state.loading_session_fetch_in_flight
                {
                    state.trigger_loading_session_fetch(&channels);
                } else if !state.snapshot.game_running
                    && !state.live_api_active
                    && !state.champ_poll_in_flight
                {
                    if let Some(client) = state.active_lcu.clone() {
                        state.champ_poll_in_flight = true;
                        let tx = champ_poll_tx.clone();
                        tokio::spawn(async move {
                            let mut session = client.get_champ_select_session().await;
                            if let Ok(Some(ref mut s)) = session {
                                if s.local_champion_id == 0 {
                                    let ids = client
                                        .get_subset_champion_list()
                                        .await
                                        .unwrap_or_default();
                                    if !ids.is_empty() {
                                        s.subset_champions = ids;
                                    }
                                }
                            }
                            let phase = if matches!(session, Ok(None)) {
                                client.get_gameflow_phase().await.ok().flatten()
                            } else {
                                None
                            };
                            let _ = tx.send(ChampPollOutcome { session, phase });
                        });
                    }
                }
            }
            _ = live_tick.tick() => {
                live_tick_counter = live_tick_counter.wrapping_add(1);
                let fast_poll = state.loading_active
                    || (state.snapshot.game_running && !state.live_match_started);
                if (fast_poll || live_tick_counter % LIVE_API_IDLE_TICK_DIVIDER == 0)
                    && !state.live_poll_in_flight
                {
                    if let Some(client) = live_client.clone() {
                        state.live_poll_in_flight = true;
                        let tx = live_poll_tx.clone();
                        tokio::spawn(async move {
                            let res = client.fetch_all_game_data().await;
                            let _ = tx.send(res);
                        });
                    }
                }
            }
            Some(ws_event) = ws_event_rx.recv() => {
                state.handle_ws_event(ws_event, &channels);
            }
            Some(champ_res) = champ_poll_rx.recv() => {
                state.handle_champ_poll_result(champ_res, &channels);
            }
            Some(live_res) = live_poll_rx.recv() => {
                state.handle_live_poll_result(live_res);
            }
            Some(resolved) = gameflow_rx.recv() => {
                state.handle_gameflow_resolved(resolved);
            }
            Some(ids) = subset_rx.recv() => {
                state.handle_subset_resolved(ids);
            }
            Some(ratings_res) = ratings_rx.recv() => {
                state.handle_ratings_resolved(ratings_res, &channels);
            }
            Some(loading_sess) = loading_session_rx.recv() => {
                state.handle_loading_session_resolved(loading_sess, &channels);
            }
            Some(loading_ratings) = loading_ratings_rx.recv() => {
                state.handle_loading_ratings_resolved(loading_ratings, &channels);
            }
            Some(local_puuid) = local_puuid_rx.recv() => {
                state.handle_local_puuid_resolved(local_puuid);
            }
        }
    }

    state.stop_lcu_ws();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_loading_slot(slot_index: usize, champion_id: i32, puuid: &str) -> LoadingPlayerSlot {
        LoadingPlayerSlot {
            slot_index,
            champion_id,
            puuid: puuid.to_owned(),
        }
    }

    #[test]
    fn determines_local_side_from_puuid_and_ally_fallback() {
        let team_one = vec![
            make_loading_slot(0, 157, "p1"),
            make_loading_slot(1, 32, "p2"),
        ];
        let team_two = vec![
            make_loading_slot(0, 89, "p6"),
            make_loading_slot(1, 145, "p7"),
        ];

        assert_eq!(
            determine_local_side(Some("p1"), &[], &team_one, &team_two),
            LoadingTeamSide::Blue
        );
        assert_eq!(
            determine_local_side(Some("p7"), &[], &team_one, &team_two),
            LoadingTeamSide::Red
        );
        assert_eq!(
            determine_local_side(
                None,
                &["p6".to_owned(), "p7".to_owned()],
                &team_one,
                &team_two
            ),
            LoadingTeamSide::Red
        );
        assert_eq!(
            determine_local_side(None, &[], &team_one, &team_two),
            LoadingTeamSide::Blue
        );
    }

    #[test]
    fn positive_live_game_time_clears_loading_screen() {
        let (event_tx, event_rx) = unbounded::<AppEvent>();
        let shared = Arc::new(RwLock::new(CoreSharedState::default()));
        let mut state = CoordinatorState::new(event_tx, shared);
        state.loading_active = true;
        state.loading_session = Some(GameflowSessionSnapshot {
            phase: "GameStart".to_owned(),
            game_id: 42,
            game_mode: "KIWI".to_owned(),
            queue_id: Some(2400),
            team_one: vec![make_loading_slot(0, 157, "p1")],
            team_two: vec![make_loading_slot(0, 32, "p6")],
        });

        // game_time_secs == 0.0 during loading must keep loading_active intact
        state.handle_live_poll_result(Ok(Some(LiveGameState {
            game_mode: "KIWI".to_owned(),
            game_time_secs: 0.0,
            ..Default::default()
        })));
        assert!(state.loading_active);
        assert!(!state.live_match_started);

        // Once game_time_secs > 0.0, loading screen clears and emits LoadingScreenCleared
        state.handle_live_poll_result(Ok(Some(LiveGameState {
            game_mode: "KIWI".to_owned(),
            game_time_secs: 1.25,
            ..Default::default()
        })));
        assert!(!state.loading_active);
        assert!(state.loading_session.is_none());
        assert!(state.live_match_started);

        let emitted: Vec<AppEvent> = event_rx.try_iter().collect();
        assert!(emitted
            .iter()
            .any(|ev| matches!(ev, AppEvent::LoadingScreenCleared)));
    }
}
