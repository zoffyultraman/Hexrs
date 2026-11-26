//! Riot / Tencent SGP match history client for ARAM Mayhem (`tag=q_2400`).

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::StatusCode;

use crate::player_rating::{MatchFetchResult, NormalizedMatch, NormalizedParticipant};

/// Maps LCU `--region` and `--rso_platform_id` into LeagueAkari-compatible SGP server IDs.
pub fn get_sgp_server_id(region: &str, rso_platform_id: Option<&str>) -> Option<String> {
    let region_up = region.trim().to_ascii_uppercase();
    if region_up.is_empty() {
        return None;
    }
    if region_up == "TENCENT" {
        let rso = rso_platform_id.map(str::trim).filter(|s| !s.is_empty())?;
        return Some(format!("TENCENT_{}", rso.to_ascii_uppercase()));
    }
    let normalized = match region_up.as_str() {
        "NA" => "NA1",
        "BR" => "BR1",
        "TR" => "TR1",
        "LAN" => "LA1",
        "LAS" => "LA2",
        "OCE" => "OC1",
        "EUW1" => "EUW",
        "JP1" => "JP",
        other => other,
    };
    Some(normalized.to_owned())
}

/// Returns the SGP match-history base URL for Tencent CN and global Riot servers.
pub fn get_sgp_match_history_url(server_id: &str) -> Option<&'static str> {
    match server_id.trim().to_ascii_uppercase().as_str() {
        "TENCENT_HN1" => Some("https://hn1-k8s-sgp.lol.qq.com:21019"),
        "TENCENT_HN10" => Some("https://hn10-k8s-sgp.lol.qq.com:21019"),
        "TENCENT_TJ100" => Some("https://tj100-sgp.lol.qq.com:21019"),
        "TENCENT_TJ101" => Some("https://tj101-sgp.lol.qq.com:21019"),
        "TENCENT_NJ100" => Some("https://nj100-sgp.lol.qq.com:21019"),
        "TENCENT_GZ100" => Some("https://gz100-sgp.lol.qq.com:21019"),
        "TENCENT_CQ100" => Some("https://cq100-sgp.lol.qq.com:21019"),
        "TENCENT_BGP2" => Some("https://bgp2-k8s-sgp.lol.qq.com:21019"),
        "TENCENT_PBE" => Some("https://pbe-sgp.lol.qq.com:21019"),
        "TENCENT_PREPBE" => Some("https://prepbe-sgp.lol.qq.com:21019"),
        "NA1" | "BR1" | "LA1" | "LA2" | "PBE" | "PBE1" => {
            Some("https://usw2-red.lol.sgp.pvp.net")
        }
        "EUW" | "EUN1" | "TR1" | "RU" => Some("https://euc1-red.lol.sgp.pvp.net"),
        "KR" | "JP" => Some("https://apne1-red.lol.sgp.pvp.net"),
        "TW2" | "SG2" | "PH2" | "VN2" | "TH2" | "OC1" => {
            Some("https://apse1-red.lol.sgp.pvp.net")
        }
        _ => None,
    }
}

/// Lightweight SGP HTTP client (`3.5s` request timeout).
#[derive(Clone)]
pub struct SgpClient {
    http: reqwest::Client,
}

impl SgpClient {
    /// Creates an SGP client with a `3.5s` per-request timeout.
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(3500))
            .build()
            .context("failed to construct SGP reqwest client")?;
        Ok(Self { http })
    }

    /// Queries SGP `/match-history-query/v1/products/lol/player/{puuid}/SUMMARY?startIndex=0&count={count}&tag=q_2400`.
    pub async fn fetch_kiwi_matches(
        &self,
        base_url: &str,
        access_token: &str,
        puuid: &str,
        count: usize,
    ) -> MatchFetchResult {
        let trimmed_base = base_url.trim_end_matches('/');
        let url = format!(
            "{trimmed_base}/match-history-query/v1/products/lol/player/{puuid}/SUMMARY?startIndex=0&count={count}&tag=q_2400"
        );

        let resp = match self.http.get(&url).bearer_auth(access_token).send().await {
            Ok(r) => r,
            Err(err) => return MatchFetchResult::Unavailable(format!("SGP request error: {err}")),
        };

        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return MatchFetchResult::Private;
        }
        if !status.is_success() {
            return MatchFetchResult::Unavailable(format!("SGP HTTP {status}"));
        }

        let val: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(err) => return MatchFetchResult::Unavailable(format!("SGP JSON error: {err}")),
        };

        match parse_sgp_matches_json(&val) {
            Ok(matches) => MatchFetchResult::Ok(matches),
            Err(reason) if reason.eq_ignore_ascii_case("private") => MatchFetchResult::Private,
            Err(reason) => MatchFetchResult::Unavailable(reason),
        }
    }
}

/// Parses an SGP match-history `SUMMARY` JSON response into `Vec<NormalizedMatch>`.
pub fn parse_sgp_matches_json(val: &serde_json::Value) -> std::result::Result<Vec<NormalizedMatch>, String> {
    if let Some(http_status) = val.get("httpStatus").and_then(|v| v.as_i64()) {
        if http_status == 401 || http_status == 403 {
            return Err("private".to_owned());
        }
    }
    if let Some(err_code) = val.get("errorCode").and_then(|v| v.as_str()) {
        let lower = err_code.to_ascii_lowercase();
        if lower.contains("private") || lower.contains("forbidden") || lower.contains("unauthorized") {
            return Err("private".to_owned());
        }
        if val.get("games").is_none() {
            return Err(format!("SGP errorCode: {err_code}"));
        }
    }

    let Some(games) = val.get("games").and_then(|v| v.as_array()) else {
        return Err("missing SGP games array".to_owned());
    };

    let mut parsed = Vec::with_capacity(games.len());
    for entry in games {
        let game = entry.get("json").unwrap_or(entry);
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

        let mut participants = Vec::new();
        if let Some(parts) = game.get("participants").and_then(|v| v.as_array()) {
            for p in parts {
                let puuid = p
                    .get("puuid")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                if puuid.is_empty() {
                    continue;
                }
                participants.push(NormalizedParticipant {
                    puuid,
                    team_id: p
                        .get("teamId")
                        .and_then(|v| v.as_i64())
                        .and_then(|v| i32::try_from(v).ok())
                        .unwrap_or(0),
                    win: p.get("win").and_then(|v| v.as_bool()).unwrap_or(false),
                    kills: p.get("kills").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    deaths: p.get("deaths").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    assists: p.get("assists").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    damage_dealt: p
                        .get("totalDamageDealtToChampions")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    damage_taken: p
                        .get("totalDamageTaken")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    damage_self_mitigated: p
                        .get("damageSelfMitigated")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    heals_on_teammates: p
                        .get("totalHealsOnTeammates")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    shielded_on_teammates: p
                        .get("totalDamageShieldedOnTeammates")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    time_ccing_others: p
                        .get("timeCCingOthers")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    gold_earned: p
                        .get("goldEarned")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                });
            }
        }

        parsed.push(NormalizedMatch {
            game_id,
            game_creation,
            game_duration,
            game_mode,
            queue_id,
            participants,
        });
    }

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_sgp_server_ids_and_urls() {
        assert_eq!(
            get_sgp_server_id("TENCENT", Some("HN1")).as_deref(),
            Some("TENCENT_HN1")
        );
        assert_eq!(
            get_sgp_match_history_url("TENCENT_HN1"),
            Some("https://hn1-k8s-sgp.lol.qq.com:21019")
        );
        assert_eq!(
            get_sgp_match_history_url("TENCENT_TJ100"),
            Some("https://tj100-sgp.lol.qq.com:21019")
        );
        assert_eq!(get_sgp_server_id("EUW1", None).as_deref(), Some("EUW"));
        assert_eq!(get_sgp_server_id("OCE", None).as_deref(), Some("OC1"));
        assert_eq!(get_sgp_server_id("TENCENT", None), None);
        assert!(get_sgp_match_history_url("KR").is_some());
    }
}
