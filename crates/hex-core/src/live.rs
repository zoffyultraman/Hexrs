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
    // Only fall back to the sole player in 1-player modes (Practice Tool); never grab player 0 in multiplayer.
    let matched_player = raw
        .all_players
        .iter()
        .find(|player| player_matches_active(player, &raw.active_player))
        .or_else(|| {
            if raw.all_players.len() == 1 {
                raw.all_players.first()
            } else {
                None
            }
        });

    let active_champion_name = matched_player
        .map(|p| p.champion_name.clone())
        .unwrap_or_default();
    let active_champion_raw_name = matched_player
        .map(|p| p.raw_champion_name.clone())
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
    let game_started_event = raw
        .events
        .events
        .iter()
        .any(|e| matches!(e.event_name.as_str(), "GameStart" | "MinionsSpawning"));

    LiveGameState {
        game_mode: raw.game_data.game_mode,
        game_time_secs: raw.game_data.game_time,
        active_champion_name,
        active_champion_raw_name,
        active_champion_id: None,
        level: raw.active_player.level,
        current_gold: raw.active_player.current_gold,
        owned_item_ids,
        is_dead,
        respawn_timer,
        game_started_event,
    }
}

fn normalize_identity(raw: &str) -> String {
    raw.trim().to_lowercase()
}

fn strip_riot_tag(normalized: &str) -> &str {
    match normalized.split_once('#') {
        Some((head, _)) => head.trim(),
        None => normalized,
    }
}

fn player_matches_active(player: &RawLivePlayer, active: &RawActivePlayer) -> bool {
    let a_riot = normalize_identity(&active.riot_id);
    let p_riot = normalize_identity(&player.riot_id);
    if !a_riot.is_empty() && !p_riot.is_empty() {
        if a_riot == p_riot {
            return true;
        }
        if a_riot.contains('#') && p_riot.contains('#') {
            return false;
        }
    }

    let a_game = normalize_identity(&active.riot_id_game_name);
    let p_game = normalize_identity(&player.riot_id_game_name);
    let a_summ = normalize_identity(&active.summoner_name);
    let p_summ = normalize_identity(&player.summoner_name);

    let active_full = [a_riot.as_str(), a_game.as_str(), a_summ.as_str()];
    let player_full = [p_riot.as_str(), p_game.as_str(), p_summ.as_str()];

    for &af in &active_full {
        if af.is_empty() {
            continue;
        }
        if player_full.iter().any(|&pf| !pf.is_empty() && pf == af) {
            return true;
        }
    }

    for &af in &active_full {
        let ab = strip_riot_tag(af);
        if ab.is_empty() {
            continue;
        }
        if player_full.iter().any(|&pf| {
            let pb = strip_riot_tag(pf);
            !pb.is_empty() && pb == ab
        }) {
            return true;
        }
    }

    false
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
    #[serde(default)]
    events: RawLiveEvents,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawLiveEvents {
    #[serde(rename = "Events", default)]
    events: Vec<RawLiveEvent>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawLiveEvent {
    #[serde(rename = "EventName", default)]
    event_name: String,
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
    #[serde(rename = "rawChampionName")]
    raw_champion_name: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_game_started_event_from_live_events() {
        let empty_raw: RawAllGameData = serde_json::from_str(
            r#"{"gameData":{"gameMode":"KIWI","gameTime":0.03},"events":{"Events":[]}}"#,
        )
        .expect("parse empty events");
        let empty_state = into_live_game_state(empty_raw);
        assert!(!empty_state.game_started_event);

        let started_raw: RawAllGameData = serde_json::from_str(
            r#"{"gameData":{"gameMode":"KIWI","gameTime":2.4},"events":{"Events":[{"EventName":"GameStart"}]}}"#,
        )
        .expect("parse GameStart event");
        let started_state = into_live_game_state(started_raw);
        assert!(started_state.game_started_event);
    }

    #[test]
    fn matches_active_player_with_tag_stripping_case_folding_and_raw_champion_name() {
        let raw: RawAllGameData = serde_json::from_str(
            r#"{
                "activePlayer": {
                    "level": 9,
                    "currentGold": 1250.0,
                    "summonerName": "  MidPlayer#12345 ",
                    "riotId": "MidPlayer#12345",
                    "riotIdGameName": ""
                },
                "allPlayers": [
                    {
                        "championName": "亚索",
                        "rawChampionName": "game_character_displayname_Yasuo",
                        "summonerName": "TopLaner",
                        "riotId": "TopLaner#11111",
                        "items": [{"itemID": 3031}]
                    },
                    {
                        "championName": "辛德拉",
                        "rawChampionName": "game_character_displayname_Syndra",
                        "summonerName": "midplayer",
                        "riotId": "",
                        "riotIdGameName": "MIDPLAYER",
                        "items": [{"itemID": 3089}, {"itemID": 3020}]
                    }
                ],
                "gameData": {"gameMode": "KIWI", "gameTime": 420.0}
            }"#,
        )
        .expect("parse live game data");

        let state = into_live_game_state(raw);
        assert_eq!(state.active_champion_name, "辛德拉");
        assert_eq!(
            state.active_champion_raw_name,
            "game_character_displayname_Syndra"
        );
        assert_eq!(state.owned_item_ids, vec![3089, 3020]);

        // Distinct `#tag` accounts sharing the same base name must match exact tag only.
        let same_base_raw: RawAllGameData = serde_json::from_str(
            r#"{
                "activePlayer": {"riotId": "SameName#2222"},
                "allPlayers": [
                    {"championName": "亚索", "rawChampionName": "game_character_displayname_Yasuo", "riotId": "SameName#1111", "items": [{"itemID": 3031}]},
                    {"championName": "卡莎", "rawChampionName": "game_character_displayname_Kaisa", "riotId": "samename#2222", "items": [{"itemID": 3124}]}
                ],
                "gameData": {"gameMode": "KIWI", "gameTime": 100.0}
            }"#,
        )
        .expect("parse same base name payload");
        let same_base_state = into_live_game_state(same_base_raw);
        assert_eq!(same_base_state.active_champion_name, "卡莎");
        assert_eq!(
            same_base_state.active_champion_raw_name,
            "game_character_displayname_Kaisa"
        );
        assert_eq!(same_base_state.owned_item_ids, vec![3124]);
    }

    #[test]
    fn does_not_fallback_to_first_player_in_multiplayer_when_unmatched() {
        let multi_raw: RawAllGameData = serde_json::from_str(
            r#"{
                "activePlayer": {"summonerName": "UnmatchedPlayer#9999"},
                "allPlayers": [
                    {"championName": "亚索", "rawChampionName": "game_character_displayname_Yasuo", "summonerName": "PlayerOne", "items": [{"itemID": 3031}]},
                    {"championName": "辛德拉", "rawChampionName": "game_character_displayname_Syndra", "summonerName": "PlayerTwo", "items": [{"itemID": 3089}]}
                ],
                "gameData": {"gameMode": "KIWI", "gameTime": 200.0}
            }"#,
        )
        .expect("parse unmatched multiplayer payload");
        let multi_state = into_live_game_state(multi_raw);
        assert!(multi_state.active_champion_name.is_empty());
        assert!(multi_state.active_champion_raw_name.is_empty());
        assert!(multi_state.owned_item_ids.is_empty());

        // Single-player Practice Tool still falls back to the only player.
        let solo_raw: RawAllGameData = serde_json::from_str(
            r#"{
                "activePlayer": {},
                "allPlayers": [
                    {"championName": "亚索", "rawChampionName": "game_character_displayname_Yasuo", "summonerName": "SoloTester", "items": [{"itemID": 1055}]}
                ],
                "gameData": {"gameMode": "PRACTICETOOL", "gameTime": 15.0}
            }"#,
        )
        .expect("parse solo payload");
        let solo_state = into_live_game_state(solo_raw);
        assert_eq!(solo_state.active_champion_name, "亚索");
        assert_eq!(
            solo_state.active_champion_raw_name,
            "game_character_displayname_Yasuo"
        );
        assert_eq!(solo_state.owned_item_ids, vec![1055]);
    }
}
