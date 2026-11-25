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
use rustls::crypto::{ring::default_provider, verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as RustlsError, SignatureScheme};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use tokio::sync::{mpsc, watch};
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION as WS_AUTHORIZATION;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{connect_async_tls_with_config, Connector};
use tracing::{debug, warn};

use crate::models::{ChampSelectState, EndOfGameParticipant, EndOfGameStats, TeamSlot};

const WAMP_EVENT_OPCODE: i64 = 8;
const EVENT_CHAMP_SELECT: &str = "OnJsonApiEvent_lol-champ-select_v1_session";
const EVENT_EOG_STATS: &str = "OnJsonApiEvent_lol-end-of-game_v1_eog-stats-block";

/// Events produced by the LCU WebSocket subscription loop.
#[derive(Debug, Clone)]
pub enum LcuWsEvent {
    Connected,
    Disconnected,
    ChampSelectUpdated(ChampSelectState),
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

    /// Maintains a reconnecting WebSocket subscription for champ select and EOG events until shutdown.
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

            let outcome = self
                .connect_and_stream(&event_tx, &mut shutdown_rx)
                .await;

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

        let sub_champ = format!(r#"[5, "{EVENT_CHAMP_SELECT}"]"#);
        let sub_eog = format!(r#"[5, "{EVENT_EOG_STATS}"]"#);

        write
            .send(Message::text(sub_champ))
            .await
            .context("failed to subscribe to champ select WS topic")
            .map_err(|e| (false, e))?;
        write
            .send(Message::text(sub_eog))
            .await
            .context("failed to subscribe to EOG WS topic")
            .map_err(|e| (false, e))?;

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
                let raw: RawChampSelectSession = serde_json::from_value(envelope.data).ok()?;
                Some(LcuWsEvent::ChampSelectUpdated(into_champ_select_state(raw)))
            }
        }
        EVENT_EOG_STATS => {
            if envelope.event_type.eq_ignore_ascii_case("Delete") {
                None
            } else {
                let raw: RawEogStatsBlock = serde_json::from_value(envelope.data).ok()?;
                Some(LcuWsEvent::EndOfGameCaptured(into_eog_stats(raw)))
            }
        }
        _ => None,
    }
}

fn into_champ_select_state(raw: RawChampSelectSession) -> ChampSelectState {
    let mut local_champion_id = 0;
    let mut local_intent_id = 0;

    let my_team = raw
        .my_team
        .into_iter()
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
            }
        })
        .collect();

    let bench_champions = raw
        .bench_champions
        .into_iter()
        .map(|entry| match entry {
            RawBenchEntry::Id(id) => id,
            RawBenchEntry::Obj { champion_id } => champion_id,
        })
        .filter(|&id| id > 0)
        .collect();

    ChampSelectState {
        phase: raw.timer.phase,
        game_mode: raw.game_mode,
        queue_id: raw.queue_id,
        local_player_cell_id: raw.local_player_cell_id,
        local_champion_id,
        local_intent_id,
        my_team,
        bench_champions,
        rerolls_remaining: raw.rerolls_remaining,
    }
}

fn into_eog_stats(raw: RawEogStatsBlock) -> EndOfGameStats {
    let mut participants = Vec::new();

    for team in raw.teams {
        for player in team.players {
            let win = stat_bool(&player.stats, "WIN").unwrap_or(team.is_winning_team);
            let placement = player
                .subteam_placement
                .or_else(|| stat_u8(&player.stats, "SUBTEAM_PLACEMENT"))
                .or_else(|| stat_u8(&player.stats, "PLACEMENT"))
                .unwrap_or(if win { 1 } else { 2 });

            let items = if !player.items.is_empty() {
                player.items.into_iter().filter(|&id| id > 0).collect()
            } else {
                extract_numbered_stat_ids(&player.stats, "ITEM", 0..=6)
            };

            let augments = if !player.augments.is_empty() {
                player.augments.into_iter().filter(|&id| id > 0).collect()
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

#[derive(Debug, Deserialize)]
struct RawWampEnvelope {
    #[serde(rename = "eventType", default)]
    event_type: String,
    #[serde(default)]
    data: serde_json::Value,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawChampSelectSession {
    timer: RawChampSelectTimer,
    #[serde(rename = "gameMode")]
    game_mode: String,
    #[serde(rename = "queueId")]
    queue_id: Option<i32>,
    #[serde(rename = "localPlayerCellId")]
    local_player_cell_id: i32,
    #[serde(rename = "myTeam")]
    my_team: Vec<RawTeamSlot>,
    #[serde(rename = "benchChampions")]
    bench_champions: Vec<RawBenchEntry>,
    #[serde(rename = "rerollsRemaining")]
    rerolls_remaining: u32,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawChampSelectTimer {
    phase: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawTeamSlot {
    #[serde(rename = "cellId")]
    cell_id: i32,
    #[serde(rename = "championId")]
    champion_id: i32,
    #[serde(rename = "championPickIntent")]
    champion_pick_intent: i32,
    #[serde(rename = "assignedPosition")]
    assigned_position: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawBenchEntry {
    Id(i32),
    Obj {
        #[serde(rename = "championId")]
        champion_id: i32,
    },
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawGameflowSession {
    #[serde(rename = "gameData")]
    game_data: RawGameflowGameData,
    map: RawGameflowMap,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawGameflowGameData {
    queue: RawGameflowQueue,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawGameflowQueue {
    id: Option<i32>,
    #[serde(rename = "gameMode")]
    game_mode: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawGameflowMap {
    #[serde(rename = "gameMode")]
    game_mode: String,
}

/// Private EOG DTO deliberately omitting all PII fields (`summonerName`, `riotId`, `puuid`, `gameId`, etc.).
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawEogStatsBlock {
    #[serde(rename = "gameMode")]
    game_mode: String,
    #[serde(rename = "queueId")]
    queue_id: Option<i32>,
    teams: Vec<RawEogTeam>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawEogTeam {
    #[serde(rename = "isWinningTeam")]
    is_winning_team: bool,
    players: Vec<RawEogPlayer>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawEogPlayer {
    #[serde(rename = "championId")]
    champion_id: i32,
    #[serde(rename = "subteamPlacement")]
    subteam_placement: Option<u8>,
    items: Vec<i32>,
    augments: Vec<i32>,
    stats: HashMap<String, serde_json::Value>,
}
