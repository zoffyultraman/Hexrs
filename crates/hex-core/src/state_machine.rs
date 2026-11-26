//! Background Tokio runtime & state machine coordinating process polling, LCU WS/REST, and Live API.

use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle as TokioJoinHandle;
use tokio::time::{interval, MissedTickBehavior};
use tracing::{debug, info, warn};

use crate::lcu::{LcuClient, LcuWsEvent};
use crate::live::LiveClient;
use crate::models::{AppEvent, AppMode, ChampSelectState, LiveGameState};
use crate::process::{LcuCredentials, ProcessMonitor, ProcessSnapshot};

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(1500);
const CHAMP_SELECT_REST_POLL_INTERVAL: Duration = Duration::from_millis(1200);
const LIVE_API_POLL_INTERVAL: Duration = Duration::from_millis(2000);

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
    gameflow_fetch_in_flight: bool,
    subset_fetch_in_flight: bool,
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
            gameflow_fetch_in_flight: false,
            subset_fetch_in_flight: false,
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
        ws_event_tx: &mpsc::UnboundedSender<LcuWsEvent>,
    ) {
        let creds_changed = next.lcu != self.active_creds;
        self.snapshot = next;

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
            self.gameflow_fetch_in_flight = false;
            self.subset_fetch_in_flight = false;
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
                        let tx = ws_event_tx.clone();
                        let ws_task = tokio::spawn(async move {
                            ws_client.run_ws_loop(tx, ws_stop_rx).await;
                        });
                        self.ws_shutdown_tx = Some(ws_stop_tx);
                        self.ws_task = Some(ws_task);

                        let fetch_client = client.clone();
                        let tx_init = ws_event_tx.clone();
                        tokio::spawn(async move {
                            emit_initial_champ_select(&fetch_client, &tx_init, true).await;
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
    }

    fn handle_ws_event(
        &mut self,
        event: LcuWsEvent,
        ws_event_tx: &mpsc::UnboundedSender<LcuWsEvent>,
        gameflow_tx: &mpsc::UnboundedSender<Option<(String, Option<i32>)>>,
        subset_tx: &mpsc::UnboundedSender<Vec<i32>>,
    ) {
        match event {
            LcuWsEvent::Connected => {
                self.last_lcu_error = None;
                if !self.lcu_connected {
                    self.lcu_connected = true;
                    let _ = self.event_tx.send(AppEvent::LcuConnectionChanged(true));
                }
                if let Some(client) = self.active_lcu.clone() {
                    let tx = ws_event_tx.clone();
                    tokio::spawn(async move {
                        emit_initial_champ_select(&client, &tx, true).await;
                    });
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
                    if self.last_champ_select.is_none() {
                        if let Some(client) = self.active_lcu.clone() {
                            let tx = ws_event_tx.clone();
                            tokio::spawn(async move {
                                emit_initial_champ_select(&client, &tx, false).await;
                            });
                        }
                    }
                } else if matches!(
                    phase.as_str(),
                    "None" | "Lobby" | "Matchmaking" | "EndOfGame"
                ) {
                    self.clear_champ_select_state();
                }
            }
            LcuWsEvent::ChampSelectUpdated(raw_state) => {
                self.in_champ_select = true;
                self.last_lcu_error = None;
                let state = self.normalize_champ_select(raw_state);

                if state.game_mode.is_empty() && !self.gameflow_fetch_in_flight {
                    if let Some(client) = self.active_lcu.clone() {
                        self.gameflow_fetch_in_flight = true;
                        let gf_tx = gameflow_tx.clone();
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
                        let stx = subset_tx.clone();
                        tokio::spawn(async move {
                            let ids = client.get_subset_champion_list().await.unwrap_or_default();
                            let _ = stx.send(ids);
                        });
                    }
                }

                self.last_champ_select = Some(state.clone());
                let _ = self.event_tx.send(AppEvent::ChampSelectUpdated(state));
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
        ws_event_tx: &mpsc::UnboundedSender<LcuWsEvent>,
        gameflow_tx: &mpsc::UnboundedSender<Option<(String, Option<i32>)>>,
        subset_tx: &mpsc::UnboundedSender<Vec<i32>>,
    ) {
        self.champ_poll_in_flight = false;
        match outcome.session {
            Ok(Some(raw_session)) => {
                self.last_lcu_error = None;
                let normalized = self.normalize_champ_select(raw_session);
                if Some(&normalized) != self.last_champ_select.as_ref() {
                    self.handle_ws_event(
                        LcuWsEvent::ChampSelectUpdated(normalized),
                        ws_event_tx,
                        gameflow_tx,
                        subset_tx,
                    );
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
                            self.sync_shared_and_mode();
                            synced = true;
                        }
                    }
                    Some("None" | "Lobby" | "Matchmaking" | "EndOfGame") => {
                        if self.in_champ_select {
                            self.handle_ws_event(
                                LcuWsEvent::ChampSelectEnded,
                                ws_event_tx,
                                gameflow_tx,
                                subset_tx,
                            );
                            synced = true;
                        }
                    }
                    _ => {
                        if self.in_champ_select && outcome.phase.is_none() {
                            self.handle_ws_event(
                                LcuWsEvent::ChampSelectEnded,
                                ws_event_tx,
                                gameflow_tx,
                                subset_tx,
                            );
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
                let _ = self.event_tx.send(AppEvent::LiveGameUpdated(live_state));
                self.sync_shared_and_mode();
            }
            Ok(None) => {
                if self.live_api_active {
                    self.live_api_active = false;
                    self.sync_shared_and_mode();
                }
            }
            Err(err) => {
                debug!(error = %err, "Live API poll failed");
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
    let (champ_poll_tx, mut champ_poll_rx) = mpsc::unbounded_channel::<ChampPollOutcome>();
    let (live_poll_tx, mut live_poll_rx) =
        mpsc::unbounded_channel::<Result<Option<LiveGameState>>>();

    let mut state = CoordinatorState::new(event_tx.clone(), shared);

    // Perform an immediate initial process poll so startup doesn't wait 1.5s.
    let initial_snapshot = monitor.poll();
    state.handle_process_snapshot(initial_snapshot, &ws_event_tx);

    let mut process_tick = interval(PROCESS_POLL_INTERVAL);
    process_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    process_tick.tick().await;

    let mut champ_poll_tick = interval(CHAMP_SELECT_REST_POLL_INTERVAL);
    champ_poll_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    champ_poll_tick.tick().await;

    let mut live_tick = interval(LIVE_API_POLL_INTERVAL);
    live_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    live_tick.tick().await;

    loop {
        tokio::select! {
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
            }
            _ = process_tick.tick() => {
                let snapshot = monitor.poll();
                state.handle_process_snapshot(snapshot, &ws_event_tx);
            }
            _ = champ_poll_tick.tick() => {
                if !state.snapshot.game_running
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
                if !state.live_poll_in_flight {
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
                state.handle_ws_event(ws_event, &ws_event_tx, &gameflow_tx, &subset_tx);
            }
            Some(champ_res) = champ_poll_rx.recv() => {
                state.handle_champ_poll_result(champ_res, &ws_event_tx, &gameflow_tx, &subset_tx);
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
        }
    }

    state.stop_lcu_ws();
}
