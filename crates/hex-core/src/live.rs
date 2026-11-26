//! Client for the local League Live Client Data API (`https://127.0.0.1:2999/liveclientdata/allgamedata`).

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::StatusCode;
use serde::Deserialize;

use crate::models::LiveGameState;

const LIVE_ALL_GAME_DATA_URL: &str = "https://127.0.0.1:2999/liveclientdata/allgamedata";

/// Polls the official local Live Client Data API during active games.
#[derive(Clone)]
pub struct LiveClient {
    http: reqwest::Client,
}

impl LiveClient {
    /// Creates a `LiveClient` configured for the local self-signed HTTPS endpoint with a 1.5s timeout.
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_millis(1500))
            .build()
            .context("failed to build Live API reqwest client")?;
        Ok(Self { http })
    }

    /// Fetches `/liveclientdata/allgamedata` and strips player identifiers, retaining only champion/items/gold/mode.
    pub async fn fetch_all_game_data(&self) -> Result<Option<LiveGameState>> {
        let resp = match self.http.get(LIVE_ALL_GAME_DATA_URL).send().await {
            Ok(resp) => resp,
            Err(err) if err.is_connect() || err.is_timeout() => {
                // Port 2999 is unavailable during loading screen or right as the match exits.
                return Ok(None);
            }
            Err(err) => return Err(err).context("Live API request failed"),
        };

        if resp.status() == StatusCode::NOT_FOUND || resp.status() == StatusCode::SERVICE_UNAVAILABLE
        {
            return Ok(None);
        }

        let payload: RawAllGameData = resp
            .error_for_status()
            .context("Live API returned error status")?
            .json()
            .await
            .context("failed to deserialize Live API allgamedata payload")?;

        Ok(Some(into_live_game_state(payload)))
    }
}

fn into_live_game_state(raw: RawAllGameData) -> LiveGameState {
    // Match activePlayer transiently against allPlayers, then discard summoner/riotId PII.
    let matched_player = raw
        .all_players
        .iter()
        .find(|player| player_matches_active(player, &raw.active_player))
        .or_else(|| raw.all_players.first());

    let active_champion_name = matched_player
        .map(|p| p.champion_name.clone())
        .unwrap_or_default();

    let owned_item_ids = matched_player
        .map(|p| {
            p.items
                .iter()
                .map(|item| item.item_id)
                .filter(|&id| id > 0)
                .collect()
        })
        .unwrap_or_default();

    let is_dead = matched_player.map(|p| p.is_dead).unwrap_or(false);
    let respawn_timer = matched_player.map(|p| p.respawn_timer).unwrap_or(0.0);

    LiveGameState {
        game_mode: raw.game_data.game_mode,
        game_time_secs: raw.game_data.game_time,
        active_champion_name,
        active_champion_id: None,
        level: raw.active_player.level,
        current_gold: raw.active_player.current_gold,
        owned_item_ids,
        is_dead,
        respawn_timer,
    }
}

fn player_matches_active(player: &RawLivePlayer, active: &RawActivePlayer) -> bool {
    if !active.riot_id.is_empty() && player.riot_id == active.riot_id {
        return true;
    }
    if !active.riot_id_game_name.is_empty()
        && player.riot_id_game_name == active.riot_id_game_name
    {
        return true;
    }
    !active.summoner_name.is_empty() && player.summoner_name == active.summoner_name
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawAllGameData {
    #[serde(rename = "activePlayer")]
    active_player: RawActivePlayer,
    #[serde(rename = "allPlayers")]
    all_players: Vec<RawLivePlayer>,
    #[serde(rename = "gameData")]
    game_data: RawGameData,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawActivePlayer {
    level: u32,
    #[serde(rename = "currentGold")]
    current_gold: f64,
    #[serde(rename = "summonerName")]
    summoner_name: String,
    #[serde(rename = "riotId")]
    riot_id: String,
    #[serde(rename = "riotIdGameName")]
    riot_id_game_name: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawLivePlayer {
    #[serde(rename = "championName")]
    champion_name: String,
    #[serde(rename = "summonerName")]
    summoner_name: String,
    #[serde(rename = "riotId")]
    riot_id: String,
    #[serde(rename = "riotIdGameName")]
    riot_id_game_name: String,
    #[serde(rename = "isDead")]
    is_dead: bool,
    #[serde(rename = "respawnTimer")]
    respawn_timer: f64,
    items: Vec<RawLiveItem>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawLiveItem {
    #[serde(rename = "itemID")]
    item_id: i32,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawGameData {
    #[serde(rename = "gameMode")]
    game_mode: String,
    #[serde(rename = "gameTime")]
    game_time: f64,
}
