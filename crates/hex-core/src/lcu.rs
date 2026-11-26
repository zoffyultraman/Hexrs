//! Local LCU REST and WebSocket client (`127.0.0.1:{port}`) with strict PII stripping.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use reqwest::header::AUTHORIZATION;
use reqwest::StatusCode;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{
    ring::default_provider, verify_tls12_signature, verify_tls13_signature, CryptoProvider,
};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as RustlsError, SignatureScheme};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use tokio::sync::{mpsc, watch};
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION as WS_AUTHORIZATION;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{connect_async_tls_with_config, Connector};
use tracing::{debug, warn};

use crate::models::{ChampSelectState, EndOfGameParticipant, EndOfGameStats, TeamSlot};
use crate::player_rating::{
    is_valid_kiwi_match, MatchFetchResult, NormalizedMatch, NormalizedParticipant,
};

const WAMP_EVENT_OPCODE: i64 = 8;
const EVENT_CHAMP_SELECT: &str = "OnJsonApiEvent_lol-champ-select_v1_session";
const EVENT_SUBSET_CHAMPIONS_TEAM_BUILDER: &str =
    "OnJsonApiEvent_lol-lobby-team-builder_champ-select_v1_subset-champion-list";
const EVENT_SUBSET_CHAMPIONS_LEGACY: &str =
    "OnJsonApiEvent_lol-champ-select_v1_subset-champion-list";
const EVENT_EOG_STATS: &str = "OnJsonApiEvent_lol-end-of-game_v1_eog-stats-block";
const EVENT_GAMEFLOW_PHASE: &str = "OnJsonApiEvent_lol-gameflow_v1_gameflow-phase";

/// Events produced by the LCU WebSocket subscription loop.
#[derive(Debug, Clone)]
pub enum LcuWsEvent {
    Connected,
    Disconnected,
    WsError(String),
    GameflowPhase(String),
    ChampSelectUpdated(ChampSelectState),
    SubsetChampionsUpdated(Vec<i32>),
    ChampSelectEnded,
    EndOfGameCaptured(EndOfGameStats),
}

/// REST and WebSocket client for the local League Client UX (`LeagueClientUx.exe`).
#[derive(Clone)]
pub struct LcuClient {
    port: u16,
    auth_header: String,
    http: reqwest::Client,
}

impl fmt::Debug for LcuClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LcuClient")
            .field("port", &self.port)
            .field("auth_header", &"<redacted>")
            .finish()
    }
}

impl LcuClient {
    /// Builds an LCU client authenticating as `riot:<token>` over HTTPS/WSS on loopback.
    pub fn new(port: u16, token: &str) -> Result<Self> {
        let encoded = STANDARD.encode(format!("riot:{token}"));
        let auth_header = format!("Basic {encoded}");
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(3))
            .build()
            .context("failed to build LCU reqwest client")?;

        Ok(Self {
            port,
            auth_header,
            http,
        })
    }

    /// Returns the local LCU HTTPS port.
    pub const fn port(&self) -> u16 {
        self.port
    }

    fn rest_url(&self, path: &str) -> String {
        format!("https://127.0.0.1:{}{path}", self.port)
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<Option<T>> {
        let resp = self
            .http
            .get(self.rest_url(path))
            .header(AUTHORIZATION, &self.auth_header)
            .send()
            .await
            .with_context(|| format!("LCU GET {path} failed"))?;

        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let parsed = resp
            .error_for_status()
            .with_context(|| format!("LCU GET {path} returned error status"))?
            .json::<T>()
            .await
            .with_context(|| format!("failed to deserialize LCU response from {path}"))?;

        Ok(Some(parsed))
    }

    /// Queries `/lol-champ-select/v1/session`. Returns `Ok(None)` when not in champ select.
    pub async fn get_champ_select_session(&self) -> Result<Option<ChampSelectState>> {
        let raw = self
            .get_json::<RawChampSelectSession>("/lol-champ-select/v1/session")
            .await?;
        Ok(raw.map(into_champ_select_state))
    }

    /// Queries ARAM Champion Cards (`subset-champion-list`), trying team-builder first then legacy champ-select.
    pub async fn get_subset_champion_list(&self) -> Result<Vec<i32>> {
        if let Ok(Some(val)) = self
            .get_json::<serde_json::Value>(
                "/lol-lobby-team-builder/champ-select/v1/subset-champion-list",
            )
            .await
        {
            let ids = parse_subset_champion_ids(&val);
            if !ids.is_empty() {
                return Ok(ids);
            }
        }

        let fallback = self
            .get_json::<serde_json::Value>("/lol-champ-select/v1/subset-champion-list")
            .await?;
        Ok(fallback
            .map(|val| parse_subset_champion_ids(&val))
            .unwrap_or_default())
    }

    /// Queries `/lol-gameflow/v1/gameflow-phase` (e.g. `"None"`, `"Lobby"`, `"ChampSelect"`, `"InProgress"`).
    pub async fn get_gameflow_phase(&self) -> Result<Option<String>> {
        let raw = self
            .get_json::<serde_json::Value>("/lol-gameflow/v1/gameflow-phase")
            .await?;
        Ok(raw.and_then(|val| {
            val.as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        }))
    }

    /// Queries `/lol-gameflow/v1/session` for the active queue's game mode (e.g. `CHERRY`, `ARAM`, `KIWI`).
    pub async fn get_current_game_mode(&self) -> Result<Option<String>> {
        Ok(self
            .get_current_game_mode_and_queue()
            .await?
            .map(|(mode, _)| mode))
    }

    /// Queries `/lol-gameflow/v1/session` for `(game_mode, queue_id)`.
    pub async fn get_current_game_mode_and_queue(&self) -> Result<Option<(String, Option<i32>)>> {
        let raw = self
            .get_json::<RawGameflowSession>("/lol-gameflow/v1/session")
            .await?;
        Ok(raw.and_then(|session| {
            let queue_id = session.game_data.queue.id;
            let mode = if !session.game_data.queue.game_mode.is_empty() {
                session.game_data.queue.game_mode
            } else if !session.map.game_mode.is_empty() {
                session.map.game_mode
            } else {
                return None;
            };
            Some((mode, queue_id))
        }))
    }

    /// Queries `/lol-end-of-game/v1/eog-stats-block` and strips all player PII during deserialization.
    pub async fn get_eog_stats_block(&self) -> Result<Option<EndOfGameStats>> {
        let raw = self
            .get_json::<RawEogStatsBlock>("/lol-end-of-game/v1/eog-stats-block")
            .await?;
        Ok(raw.map(into_eog_stats))
    }

    /// Queries `/entitlements/v1/token` for SGP Bearer access token.
    pub async fn get_entitlements_token(&self) -> Result<Option<String>> {
        let raw = self
            .get_json::<serde_json::Value>("/entitlements/v1/token")
            .await?;
        Ok(raw.and_then(|val| {
            val.get("accessToken")
                .or_else(|| val.get("token"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        }))
    }

    /// Queries `/riotclient/region-locale` returning `(region, optional rso_platform_id/web_region)`.
    pub async fn get_region_locale(&self) -> Result<Option<(String, Option<String>)>> {
        let raw = self
            .get_json::<serde_json::Value>("/riotclient/region-locale")
            .await?;
        Ok(raw.and_then(|val| {
            let region = val
                .get("region")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)?;
            let rso = val
                .get("rsoPlatformId")
                .or_else(|| val.get("webRegion"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            Some((region, rso))
        }))
    }

    /// Queries LCU `/lol-match-history/v1/products/lol/{puuid}/matches?begIndex=0&endIndex=39`
    /// as fallback when SGP is unavailable, filtering to up to 20 valid ARAM Mayhem (`KIWI`) matches.
    pub async fn get_kiwi_matches_fallback(&self, puuid: &str) -> MatchFetchResult {
        let path = format!("/lol-match-history/v1/products/lol/{puuid}/matches?begIndex=0&endIndex=39");
        let resp = match self
            .http
            .get(self.rest_url(&path))
            .header(AUTHORIZATION, &self.auth_header)
            .send()
            .await
        {
            Ok(r) => r,
            Err(err) => return MatchFetchResult::Unavailable(format!("LCU match history error: {err}")),
        };

        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return MatchFetchResult::Private;
        }
        if status == StatusCode::NOT_FOUND {
            return MatchFetchResult::Ok(Vec::new());
        }
        if !status.is_success() {
            return MatchFetchResult::Unavailable(format!("LCU match history HTTP {status}"));
        }

        let val: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(err) => return MatchFetchResult::Unavailable(format!("LCU match history JSON error: {err}")),
        };

        MatchFetchResult::Ok(parse_lcu_matches_json(&val))
    }

    /// Maintains a reconnecting WebSocket subscription for champ select, gameflow phase, and EOG events until shutdown.
    pub async fn run_ws_loop(
        &self,
        event_tx: mpsc::UnboundedSender<LcuWsEvent>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) {
        let mut backoff_secs = 1u64;

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            let outcome = self.connect_and_stream(&event_tx, &mut shutdown_rx).await;

            match outcome {
                Ok(was_connected) => {
                    if was_connected {
                        let _ = event_tx.send(LcuWsEvent::Disconnected);
                        backoff_secs = 1;
                    }
                }
                Err((was_connected, err)) => {
                    if was_connected {
                        let _ = event_tx.send(LcuWsEvent::Disconnected);
                        backoff_secs = 1;
                    }
                    let _ = event_tx.send(LcuWsEvent::WsError(format!("LCU WS: {err:#}")));
                    debug!(error = %err, backoff_secs, "LCU WebSocket disconnected; retrying");
                }
            }

            if *shutdown_rx.borrow() {
                break;
            }

            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
                _ = sleep(Duration::from_secs(backoff_secs)) => {}
            }

            backoff_secs = (backoff_secs * 2).min(15);
        }
    }

    async fn connect_and_stream(
        &self,
        event_tx: &mpsc::UnboundedSender<LcuWsEvent>,
        shutdown_rx: &mut watch::Receiver<bool>,
    ) -> std::result::Result<bool, (bool, anyhow::Error)> {
        let tls_config = build_lcu_tls_config().map_err(|e| (false, e))?;
        let connector = Connector::Rustls(Arc::new(tls_config));

        let url = format!("wss://127.0.0.1:{}/", self.port);
        let mut request = url
            .as_str()
            .into_client_request()
            .context("failed to construct LCU WebSocket request")
            .map_err(|e| (false, e))?;
        let header_val = HeaderValue::from_str(&self.auth_header)
            .context("invalid LCU Authorization header value")
            .map_err(|e| (false, e))?;
        request.headers_mut().insert(WS_AUTHORIZATION, header_val);

        let (ws_stream, _) =
            connect_async_tls_with_config(request, None, false, Some(connector))
                .await
                .context("failed to connect to LCU WebSocket")
                .map_err(|e| (false, e))?;

        let (mut write, mut read) = ws_stream.split();

        for topic in [
            EVENT_CHAMP_SELECT,
            EVENT_SUBSET_CHAMPIONS_TEAM_BUILDER,
            EVENT_SUBSET_CHAMPIONS_LEGACY,
            EVENT_EOG_STATS,
            EVENT_GAMEFLOW_PHASE,
        ] {
            let sub_frame = format!(r#"[5, "{topic}"]"#);
            write
                .send(Message::text(sub_frame))
                .await
                .with_context(|| format!("failed to subscribe to WS topic {topic}"))
                .map_err(|e| (false, e))?;
        }

        let _ = event_tx.send(LcuWsEvent::Connected);

        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        let _ = write.close().await;
                        return Ok(true);
                    }
                }
                next_msg = read.next() => {
                    match next_msg {
                        Some(Ok(msg)) => {
                            if msg.is_text() {
                                match msg.to_text() {
                                    Ok(text) => {
                                        if let Some(ws_event) = parse_wamp_event(text) {
                                            let _ = event_tx.send(ws_event);
                                        }
                                    }
                                    Err(err) => {
                                        warn!(error = %err, "received non-UTF8 text frame from LCU");
                                    }
                                }
                            } else if msg.is_close() {
                                return Ok(true);
                            }
                        }
                        Some(Err(err)) => {
                            return Err((true, anyhow::Error::from(err).context("LCU WebSocket stream error")));
                        }
                        None => return Ok(true),
                    }
                }
            }
        }
    }
}

fn build_lcu_tls_config() -> Result<ClientConfig> {
    let provider = Arc::new(default_provider());
    let verifier = Arc::new(LcuLocalCertVerifier::new(Arc::clone(&provider)));
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("failed to configure TLS protocol versions for LCU")?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Ok(config)
}

/// Custom `rustls` verifier scoped strictly to the local loopback LCU client's self-signed cert.
#[derive(Debug)]
struct LcuLocalCertVerifier {
    provider: Arc<CryptoProvider>,
}

impl LcuLocalCertVerifier {
    fn new(provider: Arc<CryptoProvider>) -> Self {
        Self { provider }
    }
}

impl ServerCertVerifier for LcuLocalCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, RustlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Parses `[8, "<topic>", {"eventType": "...", "data": ...}]` LCU WAMP event frames.
pub fn parse_wamp_event(text: &str) -> Option<LcuWsEvent> {
    if text.trim().is_empty() {
        return None;
    }

    let (opcode, topic, envelope): (i64, String, RawWampEnvelope) =
        serde_json::from_str(text).ok()?;
    if opcode != WAMP_EVENT_OPCODE {
        return None;
    }

    match topic.as_str() {
        EVENT_CHAMP_SELECT => {
            if envelope.event_type.eq_ignore_ascii_case("Delete") {
                Some(LcuWsEvent::ChampSelectEnded)
            } else {
                match serde_json::from_value::<RawChampSelectSession>(envelope.data) {
                    Ok(raw) => Some(LcuWsEvent::ChampSelectUpdated(into_champ_select_state(raw))),
                    Err(err) => {
                        warn!(error = %err, "failed to deserialize LCU WS champ select payload");
                        None
                    }
                }
            }
        }
        EVENT_SUBSET_CHAMPIONS_TEAM_BUILDER | EVENT_SUBSET_CHAMPIONS_LEGACY => {
            if envelope.event_type.eq_ignore_ascii_case("Delete") {
                Some(LcuWsEvent::SubsetChampionsUpdated(Vec::new()))
            } else {
                Some(LcuWsEvent::SubsetChampionsUpdated(
                    parse_subset_champion_ids(&envelope.data),
                ))
            }
        }
        EVENT_GAMEFLOW_PHASE => {
            if envelope.event_type.eq_ignore_ascii_case("Delete") {
                Some(LcuWsEvent::GameflowPhase("None".to_owned()))
            } else {
                envelope
                    .data
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|phase| LcuWsEvent::GameflowPhase(phase.to_owned()))
            }
        }
        EVENT_EOG_STATS => {
            if envelope.event_type.eq_ignore_ascii_case("Delete") {
                None
            } else {
                match serde_json::from_value::<RawEogStatsBlock>(envelope.data) {
                    Ok(raw) => Some(LcuWsEvent::EndOfGameCaptured(into_eog_stats(raw))),
                    Err(err) => {
                        warn!(error = %err, "failed to deserialize LCU WS EOG payload");
                        None
                    }
                }
            }
        }
        _ => None,
    }
}

fn extract_subset_champion_id(item: &serde_json::Value) -> Option<i32> {
    item.as_i64()
        .and_then(|v| i32::try_from(v).ok())
        .or_else(|| {
            item.get("championId")
                .and_then(|v| v.as_i64())
                .and_then(|v| i32::try_from(v).ok())
        })
        .filter(|&id| id > 0)
}

fn parse_subset_champion_ids(val: &serde_json::Value) -> Vec<i32> {
    let Some(items) = val.as_array().or_else(|| {
        val.get("championIds")
            .and_then(|v| v.as_array())
            .or_else(|| val.get("subsetChampionList").and_then(|v| v.as_array()))
    }) else {
        return Vec::new();
    };

    let mut ids = Vec::with_capacity(items.len());
    for item in items {
        if let Some(id) = extract_subset_champion_id(item) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    ids
}

fn into_champ_select_state(raw: RawChampSelectSession) -> ChampSelectState {
    let mut local_champion_id = 0;
    let mut local_intent_id = 0;

    let my_team = raw
        .my_team
        .into_iter()
        .flatten()
        .map(|slot| {
            if slot.cell_id == raw.local_player_cell_id {
                local_champion_id = slot.champion_id;
                local_intent_id = slot.champion_pick_intent;
            }
            TeamSlot {
                cell_id: slot.cell_id,
                champion_id: slot.champion_id,
                pick_intent: slot.champion_pick_intent,
                assigned_position: slot.assigned_position,
                puuid: slot.puuid,
            }
        })
        .collect();

    let bench_champions = raw
        .bench_champions
        .into_iter()
        .flatten()
        .map(|entry| match entry {
            RawBenchEntry::Id(id) => id,
            RawBenchEntry::Obj { champion_id } => champion_id,
        })
        .filter(|&id| id > 0)
        .collect();

    let rerolls_remaining = u32::try_from(raw.rerolls_remaining.max(0)).unwrap_or(u32::MAX);

    ChampSelectState {
        phase: raw.timer.phase,
        game_mode: raw.game_mode,
        queue_id: raw.queue_id,
        local_player_cell_id: raw.local_player_cell_id,
        local_champion_id,
        local_intent_id,
        my_team,
        bench_champions,
        allow_subset_champion_picks: raw.allow_subset_champion_picks,
        subset_champions: Vec::new(),
        rerolls_remaining,
    }
}

fn into_eog_stats(raw: RawEogStatsBlock) -> EndOfGameStats {
    let mut participants = Vec::new();

    for team in raw.teams.into_iter().flatten() {
        for player in team.players.into_iter().flatten() {
            let win = stat_bool(&player.stats, "WIN").unwrap_or(team.is_winning_team);
            let placement = player
                .subteam_placement
                .or_else(|| stat_u8(&player.stats, "SUBTEAM_PLACEMENT"))
                .or_else(|| stat_u8(&player.stats, "PLACEMENT"))
                .unwrap_or(if win { 1 } else { 2 });

            let items: Vec<i32> = if !player.items.is_empty() {
                player
                    .items
                    .into_iter()
                    .flatten()
                    .filter(|&id| id > 0)
                    .collect()
            } else {
                extract_numbered_stat_ids(&player.stats, "ITEM", 0..=6)
            };

            let augments: Vec<i32> = if !player.augments.is_empty() {
                player
                    .augments
                    .into_iter()
                    .flatten()
                    .filter(|&id| id > 0)
                    .collect()
            } else {
                let mut augs = extract_numbered_stat_ids(&player.stats, "PLAYER_AUGMENT_", 1..=4);
                if augs.is_empty() {
                    augs = extract_numbered_stat_ids(&player.stats, "AUGMENT_", 1..=4);
                }
                augs
            };

            if player.champion_id > 0 {
                participants.push(EndOfGameParticipant {
                    champion_id: player.champion_id,
                    placement,
                    win,
                    items,
                    augments,
                });
            }
        }
    }

    EndOfGameStats {
        game_mode: raw.game_mode,
        queue_id: raw.queue_id,
        participants,
    }
}

fn stat_i64(stats: &HashMap<String, serde_json::Value>, key: &str) -> Option<i64> {
    let val = stats.get(key)?;
    val.as_i64()
        .or_else(|| val.as_u64().and_then(|v| i64::try_from(v).ok()))
        .or_else(|| val.as_f64().map(|v| v.round() as i64))
}

fn stat_u8(stats: &HashMap<String, serde_json::Value>, key: &str) -> Option<u8> {
    stat_i64(stats, key).and_then(|v| u8::try_from(v).ok())
}

fn stat_bool(stats: &HashMap<String, serde_json::Value>, key: &str) -> Option<bool> {
    let val = stats.get(key)?;
    val.as_bool()
        .or_else(|| stat_i64(stats, key).map(|n| n > 0))
}

fn extract_numbered_stat_ids(
    stats: &HashMap<String, serde_json::Value>,
    prefix: &str,
    range: std::ops::RangeInclusive<u8>,
) -> Vec<i32> {
    let mut ids = Vec::new();
    for idx in range {
        let key = format!("{prefix}{idx}");
        if let Some(raw_id) = stat_i64(stats, &key) {
            if let Ok(id) = i32::try_from(raw_id) {
                if id > 0 {
                    ids.push(id);
                }
            }
        }
    }
    ids
}

/// Parses LCU `/lol-match-history/v1/products/lol/{puuid}/matches` JSON into up to 20 valid `KIWI` matches.
pub fn parse_lcu_matches_json(val: &serde_json::Value) -> Vec<NormalizedMatch> {
    let Some(games) = val
        .get("games")
        .and_then(|g| g.get("games").or(Some(g)))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };

    let mut parsed = Vec::new();
    for game in games {
        let Some(game_id) = game.get("gameId").and_then(|v| v.as_u64()) else {
            continue;
        };
        let game_creation = game.get("gameCreation").and_then(|v| v.as_i64()).unwrap_or(0);
        let game_duration = game.get("gameDuration").and_then(|v| v.as_u64()).unwrap_or(0);
        let game_mode = game
            .get("gameMode")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let queue_id = game
            .get("queueId")
            .and_then(|v| v.as_i64())
            .and_then(|v| i32::try_from(v).ok());

        let mut id_to_puuid: HashMap<i64, String> = HashMap::new();
        if let Some(identities) = game.get("participantIdentities").and_then(|v| v.as_array()) {
            for ident in identities {
                if let Some(pid) = ident.get("participantId").and_then(|v| v.as_i64()) {
                    if let Some(puuid) = ident
                        .get("player")
                        .and_then(|pl| pl.get("puuid"))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        id_to_puuid.insert(pid, puuid.to_owned());
                    }
                }
            }
        }

        let mut participants = Vec::new();
        if let Some(parts) = game.get("participants").and_then(|v| v.as_array()) {
            for p in parts {
                let puuid = p
                    .get("puuid")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .or_else(|| {
                        p.get("participantId")
                            .and_then(|v| v.as_i64())
                            .and_then(|pid| id_to_puuid.get(&pid).cloned())
                    })
                    .unwrap_or_default();
                if puuid.is_empty() {
                    continue;
                }
                let stats = p.get("stats").unwrap_or(p);
                participants.push(NormalizedParticipant {
                    puuid,
                    team_id: p
                        .get("teamId")
                        .and_then(|v| v.as_i64())
                        .and_then(|v| i32::try_from(v).ok())
                        .unwrap_or(0),
                    win: stats.get("win").and_then(|v| v.as_bool()).unwrap_or(false),
                    kills: stats.get("kills").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    deaths: stats.get("deaths").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    assists: stats.get("assists").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    damage_dealt: stats
                        .get("totalDamageDealtToChampions")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    damage_taken: stats
                        .get("totalDamageTaken")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    damage_self_mitigated: stats
                        .get("damageSelfMitigated")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    heals_on_teammates: stats
                        .get("totalHealsOnTeammates")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    shielded_on_teammates: stats
                        .get("totalDamageShieldedOnTeammates")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    time_ccing_others: stats
                        .get("timeCCingOthers")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    gold_earned: stats
                        .get("goldEarned")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                });
            }
        }

        let normalized = NormalizedMatch {
            game_id,
            game_creation,
            game_duration,
            game_mode,
            queue_id,
            participants,
        };
        if is_valid_kiwi_match(&normalized) {
            parsed.push(normalized);
        }
    }

    parsed.sort_by(|a, b| {
        b.game_creation
            .cmp(&a.game_creation)
            .then_with(|| b.game_id.cmp(&a.game_id))
    });
    parsed.truncate(20);
    parsed
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    T: Default + Deserialize<'de>,
    D: Deserializer<'de>,
{
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

#[derive(Debug, Deserialize)]
struct RawWampEnvelope {
    #[serde(rename = "eventType", default, deserialize_with = "deserialize_null_default")]
    event_type: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    data: serde_json::Value,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawChampSelectSession {
    #[serde(deserialize_with = "deserialize_null_default")]
    timer: RawChampSelectTimer,
    #[serde(rename = "gameMode", deserialize_with = "deserialize_null_default")]
    game_mode: String,
    #[serde(rename = "queueId")]
    queue_id: Option<i32>,
    #[serde(rename = "localPlayerCellId", deserialize_with = "deserialize_null_default")]
    local_player_cell_id: i32,
    #[serde(rename = "myTeam", deserialize_with = "deserialize_null_default")]
    my_team: Vec<Option<RawTeamSlot>>,
    #[serde(rename = "benchChampions", deserialize_with = "deserialize_null_default")]
    bench_champions: Vec<Option<RawBenchEntry>>,
    #[serde(
        rename = "allowSubsetChampionPicks",
        deserialize_with = "deserialize_null_default"
    )]
    allow_subset_champion_picks: bool,
    #[serde(rename = "rerollsRemaining", deserialize_with = "deserialize_null_default")]
    rerolls_remaining: i64,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawChampSelectTimer {
    #[serde(deserialize_with = "deserialize_null_default")]
    phase: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawTeamSlot {
    #[serde(rename = "cellId", deserialize_with = "deserialize_null_default")]
    cell_id: i32,
    #[serde(rename = "championId", deserialize_with = "deserialize_null_default")]
    champion_id: i32,
    #[serde(rename = "championPickIntent", deserialize_with = "deserialize_null_default")]
    champion_pick_intent: i32,
    #[serde(rename = "assignedPosition", deserialize_with = "deserialize_null_default")]
    assigned_position: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    puuid: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawBenchEntry {
    Id(i32),
    Obj {
        #[serde(rename = "championId", default, deserialize_with = "deserialize_null_default")]
        champion_id: i32,
    },
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawGameflowSession {
    #[serde(rename = "gameData", deserialize_with = "deserialize_null_default")]
    game_data: RawGameflowGameData,
    #[serde(deserialize_with = "deserialize_null_default")]
    map: RawGameflowMap,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawGameflowGameData {
    #[serde(deserialize_with = "deserialize_null_default")]
    queue: RawGameflowQueue,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawGameflowQueue {
    id: Option<i32>,
    #[serde(rename = "gameMode", deserialize_with = "deserialize_null_default")]
    game_mode: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawGameflowMap {
    #[serde(rename = "gameMode", deserialize_with = "deserialize_null_default")]
    game_mode: String,
}

/// Private EOG DTO deliberately omitting all PII fields (`summonerName`, `riotId`, `puuid`, `gameId`, etc.).
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawEogStatsBlock {
    #[serde(rename = "gameMode", deserialize_with = "deserialize_null_default")]
    game_mode: String,
    #[serde(rename = "queueId")]
    queue_id: Option<i32>,
    #[serde(deserialize_with = "deserialize_null_default")]
    teams: Vec<Option<RawEogTeam>>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawEogTeam {
    #[serde(rename = "isWinningTeam", deserialize_with = "deserialize_null_default")]
    is_winning_team: bool,
    #[serde(deserialize_with = "deserialize_null_default")]
    players: Vec<Option<RawEogPlayer>>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawEogPlayer {
    #[serde(rename = "championId", deserialize_with = "deserialize_null_default")]
    champion_id: i32,
    #[serde(rename = "subteamPlacement")]
    subteam_placement: Option<u8>,
    #[serde(deserialize_with = "deserialize_null_default")]
    items: Vec<Option<i32>>,
    #[serde(deserialize_with = "deserialize_null_default")]
    augments: Vec<Option<i32>>,
    #[serde(deserialize_with = "deserialize_null_default")]
    stats: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_subset_champion_ws_events() {
        let frame = r#"[8,"OnJsonApiEvent_lol-lobby-team-builder_champ-select_v1_subset-champion-list",{"eventType":"Update","data":[157,432,157,0,-1]}]"#;
        match parse_wamp_event(frame) {
            Some(LcuWsEvent::SubsetChampionsUpdated(ids)) => assert_eq!(ids, vec![157, 432]),
            other => panic!("unexpected parsed event: {other:?}"),
        }

        let legacy_obj_frame = r#"[8,"OnJsonApiEvent_lol-champ-select_v1_subset-champion-list",{"eventType":"Create","data":{"championIds":[89,145,89]}}]"#;
        match parse_wamp_event(legacy_obj_frame) {
            Some(LcuWsEvent::SubsetChampionsUpdated(ids)) => assert_eq!(ids, vec![89, 145]),
            other => panic!("unexpected parsed event: {other:?}"),
        }

        let del_frame = r#"[8,"OnJsonApiEvent_lol-lobby-team-builder_champ-select_v1_subset-champion-list",{"eventType":"Delete","data":null}]"#;
        match parse_wamp_event(del_frame) {
            Some(LcuWsEvent::SubsetChampionsUpdated(ids)) => assert!(ids.is_empty()),
            other => panic!("unexpected parsed event: {other:?}"),
        }
    }

    #[test]
    fn parses_allow_subset_champion_picks_from_session_ws_event() {
        let frame = r#"[8,"OnJsonApiEvent_lol-champ-select_v1_session",{"eventType":"Update","data":{"timer":{"phase":"PLANNING"},"gameMode":"ARAM","queueId":450,"localPlayerCellId":2,"allowSubsetChampionPicks":true,"myTeam":[{"cellId":2,"championId":0,"championPickIntent":0,"assignedPosition":""}],"benchChampions":[32],"rerollsRemaining":1}}]"#;
        match parse_wamp_event(frame) {
            Some(LcuWsEvent::ChampSelectUpdated(state)) => {
                assert!(state.allow_subset_champion_picks);
                assert!(state.subset_champions.is_empty());
                assert_eq!(state.local_champion_id, 0);
                assert_eq!(state.bench_champions, vec![32]);
            }
            other => panic!("unexpected parsed event: {other:?}"),
        }
    }
}
