//! ARAM Mayhem (`queueId == 2400` / `KIWI`) V2 5-dimension player rating,
//! premade party detection (`TeamUpGraph` + `UnionFind`), and per-group last-game MVP scoring.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::models::{
    PremadeGroupSummary, RatingTitlesConfig, SummonerRatingStatus, SummonerRatingSummary,
    TeamRatingsSnapshot, TeamSlot,
};

const MIN_VALID_KIWI_DURATION_SECS: u64 = 300;
const MIN_READY_SAMPLE_GAMES: usize = 3;
const MAX_EVALUATED_MATCHES: usize = 20;
const PREMADE_MIN_SHARED_GAMES: usize = 2;

/// Normalized participant telemetry from SGP or LCU match history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedParticipant {
    pub puuid: String,
    pub team_id: i32,
    pub win: bool,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub damage_dealt: u64,
    pub damage_taken: u64,
    #[serde(default)]
    pub damage_self_mitigated: u64,
    pub heals_on_teammates: u64,
    pub shielded_on_teammates: u64,
    #[serde(default)]
    pub time_ccing_others: u64,
    pub gold_earned: u64,
}

/// Normalized match record filtered to ARAM Mayhem (`KIWI` / `queueId 2400`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedMatch {
    pub game_id: u64,
    pub game_creation: i64,
    pub game_duration: u64,
    pub game_mode: String,
    pub queue_id: Option<i32>,
    pub participants: Vec<NormalizedParticipant>,
}

/// Result of querying a player's match history via SGP or LCU fallback.
#[derive(Debug, Clone, PartialEq)]
pub enum MatchFetchResult {
    Ok(Vec<NormalizedMatch>),
    Private,
    Unavailable(String),
}

/// Returns `true` for completed, non-remake ARAM Mayhem matches (`queueId == 2400 || gameMode == "KIWI"`, `duration >= 300s`).
pub fn is_valid_kiwi_match(m: &NormalizedMatch) -> bool {
    let is_kiwi = m.queue_id == Some(2400) || m.game_mode.eq_ignore_ascii_case("KIWI");
    is_kiwi && m.game_duration >= MIN_VALID_KIWI_DURATION_SECS
}

fn map_range(val: f64, lo: f64, hi: f64, max_pts: f64) -> f64 {
    if hi <= lo {
        return 0.0;
    }
    ((val - lo) / (hi - lo)).clamp(0.0, 1.0) * max_pts
}

#[derive(Debug, Clone, Copy)]
struct MatchSubScores {
    role_pts: f64,
    kp_pts: f64,
    gold_pts: f64,
}

fn kda_ratio(kills: u64, deaths: u64, assists: u64) -> f64 {
    (kills + assists) as f64 / (deaths.max(1) as f64)
}

fn compute_match_subscores(m: &NormalizedMatch, me: &NormalizedParticipant) -> MatchSubScores {
    let duration_mins = (m.game_duration as f64 / 60.0).max(1.0);
    let team_mates: Vec<&NormalizedParticipant> = m
        .participants
        .iter()
        .filter(|p| p.team_id == me.team_id)
        .collect();
    let has_full_team = team_mates.len() >= 2;
    let heal_shield = (me.heals_on_teammates + me.shielded_on_teammates) as f64;

    let (s_dmg, s_tank, s_util, kp_pts, gold_pts) = if has_full_team {
        let count = team_mates.len() as f64;
        let team_avg_dmg = team_mates.iter().map(|p| p.damage_dealt as f64).sum::<f64>() / count;
        let team_avg_taken =
            team_mates.iter().map(|p| p.damage_taken as f64).sum::<f64>() / count;
        let team_avg_cc =
            team_mates.iter().map(|p| p.time_ccing_others as f64).sum::<f64>() / count;
        let team_avg_gold = team_mates.iter().map(|p| p.gold_earned as f64).sum::<f64>() / count;
        let team_kills: u64 = team_mates.iter().map(|p| u64::from(p.kills)).sum();

        let dmg = if team_avg_dmg > 0.0 {
            map_range(me.damage_dealt as f64 / team_avg_dmg, 0.45, 1.50, 1.0)
        } else {
            0.5
        };

        let has_mitigated = team_mates.iter().any(|p| p.damage_self_mitigated > 0);
        let tank = if has_mitigated {
            let my_t = (me.damage_taken + me.damage_self_mitigated) as f64;
            let team_avg_t = team_mates
                .iter()
                .map(|p| (p.damage_taken + p.damage_self_mitigated) as f64)
                .sum::<f64>()
                / count;
            if team_avg_t > 0.0 {
                map_range(my_t / team_avg_t, 0.45, 1.55, 1.0)
            } else {
                0.5
            }
        } else if team_avg_taken > 0.0 {
            map_range(me.damage_taken as f64 / team_avg_taken, 0.45, 1.55, 1.0)
        } else {
            0.5
        };

        let s_heal = if team_avg_taken > 0.0 {
            map_range(heal_shield / team_avg_taken, 0.10, 0.90, 1.0)
        } else {
            0.5
        };
        let s_cc = if team_avg_cc > 0.0 {
            map_range(me.time_ccing_others as f64 / team_avg_cc, 0.60, 1.80, 1.0)
        } else {
            0.5
        };
        let util = s_heal.max(s_cc);

        let kp = if team_kills > 0 {
            let ratio = ((me.kills + me.assists) as f64 / team_kills as f64).min(1.0);
            map_range(ratio, 0.38, 0.85, 15.0)
        } else {
            0.5 * 15.0
        };

        let gold = if team_avg_gold > 0.0 {
            map_range(me.gold_earned as f64 / team_avg_gold, 0.82, 1.18, 10.0)
        } else {
            0.5 * 10.0
        };

        (dmg, tank, util, kp, gold)
    } else {
        let dmg = map_range(me.damage_dealt as f64 / duration_mins, 1100.0, 3600.0, 1.0);
        let tank = if me.damage_self_mitigated > 0 {
            map_range(
                (me.damage_taken + me.damage_self_mitigated) as f64 / duration_mins,
                1500.0,
                5500.0,
                1.0,
            )
        } else {
            map_range(me.damage_taken as f64 / duration_mins, 900.0, 3000.0, 1.0)
        };
        let s_heal = map_range(heal_shield / duration_mins, 120.0, 1100.0, 1.0);
        let s_cc = map_range(me.time_ccing_others as f64 / duration_mins, 1.5, 6.0, 1.0);
        let util = s_heal.max(s_cc);

        let kp = map_range(
            (me.kills + me.assists) as f64 / duration_mins,
            0.70,
            2.20,
            15.0,
        );
        let gold = map_range(me.gold_earned as f64 / duration_mins, 600.0, 1050.0, 10.0);

        (dmg, tank, util, kp, gold)
    };

    let mut roles = [s_dmg, s_tank, s_util];
    roles.sort_by(|a, b| b.total_cmp(a));
    let role_pts = (0.70 * roles[0] + 0.30 * roles[1]) * 35.0;

    MatchSubScores {
        role_pts,
        kp_pts,
        gold_pts,
    }
}

/// Computes the V2 single-match performance score (`0.0..=75.0`, excluding win rate).
pub fn score_single_match_performance(m: &NormalizedMatch, me: &NormalizedParticipant) -> f64 {
    let sub = compute_match_subscores(m, me);
    let single_kda = kda_ratio(u64::from(me.kills), u64::from(me.deaths), u64::from(me.assists));
    let kda_pts = map_range(single_kda, 1.0, 4.5, 15.0);
    sub.role_pts + sub.kp_pts + kda_pts + sub.gold_pts
}

fn compute_streak(newest_first_wins: &[bool]) -> i8 {
    let Some(&first) = newest_first_wins.first() else {
        return 0;
    };
    let streak_len = newest_first_wins
        .iter()
        .take_while(|&&w| w == first)
        .count();
    if streak_len < 3 {
        return 0;
    }
    let capped = streak_len.min(127) as i8;
    if first {
        capped
    } else {
        -capped
    }
}

fn evaluate_single_player(
    cell_id: i32,
    puuid: &str,
    fetch_res: Option<&MatchFetchResult>,
    config: &RatingTitlesConfig,
) -> SummonerRatingSummary {
    match fetch_res {
        None => {
            let (tier_index, title) = config.resolve(SummonerRatingStatus::Loading, 0);
            SummonerRatingSummary {
                cell_id,
                status: SummonerRatingStatus::Loading,
                score: 0,
                tier_index,
                title,
                games_count: 0,
                win_rate: 0.0,
                avg_kda: 0.0,
                streak: 0,
                premade_group_id: None,
                is_premade_last_mvp: false,
            }
        }
        Some(MatchFetchResult::Private) => {
            let (tier_index, title) = config.resolve(SummonerRatingStatus::Private, 0);
            SummonerRatingSummary {
                cell_id,
                status: SummonerRatingStatus::Private,
                score: 0,
                tier_index,
                title,
                games_count: 0,
                win_rate: 0.0,
                avg_kda: 0.0,
                streak: 0,
                premade_group_id: None,
                is_premade_last_mvp: false,
            }
        }
        Some(MatchFetchResult::Unavailable(_)) => {
            let (tier_index, title) = config.resolve(SummonerRatingStatus::InsufficientSamples, 0);
            SummonerRatingSummary {
                cell_id,
                status: SummonerRatingStatus::InsufficientSamples,
                score: 0,
                tier_index,
                title,
                games_count: 0,
                win_rate: 0.0,
                avg_kda: 0.0,
                streak: 0,
                premade_group_id: None,
                is_premade_last_mvp: false,
            }
        }
        Some(MatchFetchResult::Ok(raw_matches)) => {
            let mut valid: Vec<(&NormalizedMatch, &NormalizedParticipant)> = Vec::new();
            let mut seen_games: HashSet<u64> = HashSet::new();
            for m in raw_matches {
                if !is_valid_kiwi_match(m) || !seen_games.insert(m.game_id) {
                    continue;
                }
                if let Some(me) = m.participants.iter().find(|p| p.puuid == puuid) {
                    valid.push((m, me));
                }
            }

            valid.sort_by(|a, b| {
                b.0.game_creation
                    .cmp(&a.0.game_creation)
                    .then_with(|| b.0.game_id.cmp(&a.0.game_id))
            });
            valid.truncate(MAX_EVALUATED_MATCHES);

            let games_n = valid.len();
            if games_n == 0 {
                let (tier_index, title) =
                    config.resolve(SummonerRatingStatus::InsufficientSamples, 0);
                return SummonerRatingSummary {
                    cell_id,
                    status: SummonerRatingStatus::InsufficientSamples,
                    score: 0,
                    tier_index,
                    title,
                    games_count: 0,
                    win_rate: 0.0,
                    avg_kda: 0.0,
                    streak: 0,
                    premade_group_id: None,
                    is_premade_last_mvp: false,
                };
            }

            let wins_count = valid.iter().filter(|(_, p)| p.win).count();
            let win_rate = wins_count as f64 / games_n as f64;
            let total_k: u64 = valid.iter().map(|(_, p)| u64::from(p.kills)).sum();
            let total_d: u64 = valid.iter().map(|(_, p)| u64::from(p.deaths)).sum();
            let total_a: u64 = valid.iter().map(|(_, p)| u64::from(p.assists)).sum();
            let avg_kda = kda_ratio(total_k, total_d, total_a);
            let newest_wins: Vec<bool> = valid.iter().map(|(_, p)| p.win).collect();
            let streak = compute_streak(&newest_wins);
            let games_count = games_n.min(u8::MAX as usize) as u8;

            if games_n < MIN_READY_SAMPLE_GAMES {
                let (tier_index, title) =
                    config.resolve(SummonerRatingStatus::InsufficientSamples, 0);
                return SummonerRatingSummary {
                    cell_id,
                    status: SummonerRatingStatus::InsufficientSamples,
                    score: 0,
                    tier_index,
                    title,
                    games_count,
                    win_rate,
                    avg_kda,
                    streak,
                    premade_group_id: None,
                    is_premade_last_mvp: false,
                };
            }

            let n_f = games_n as f64;
            let wr_smooth = (wins_count as f64 + 2.0) / (n_f + 4.0);
            let s_wr = map_range(wr_smooth, 0.25, 0.75, 25.0);
            let s_kda = map_range(avg_kda, 1.0, 4.5, 15.0);

            let mut role_sum = 0.0;
            let mut kp_sum = 0.0;
            let mut gold_sum = 0.0;
            for (m, me) in &valid {
                let sub = compute_match_subscores(m, me);
                role_sum += sub.role_pts;
                kp_sum += sub.kp_pts;
                gold_sum += sub.gold_pts;
            }

            let raw_score = s_wr + (role_sum / n_f) + (kp_sum / n_f) + s_kda + (gold_sum / n_f);
            let score = if raw_score.is_finite() {
                raw_score.round().clamp(0.0, 100.0) as u8
            } else {
                0
            };
            let (tier_index, title) = config.resolve(SummonerRatingStatus::Ready, score);

            SummonerRatingSummary {
                cell_id,
                status: SummonerRatingStatus::Ready,
                score,
                tier_index,
                title,
                games_count,
                win_rate,
                avg_kda,
                streak,
                premade_group_id: None,
                is_premade_last_mvp: false,
            }
        }
    }
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
        }
    }

    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]);
        }
        self.parent[x]
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            if ra < rb {
                self.parent[rb] = ra;
            } else {
                self.parent[ra] = rb;
            }
        }
    }
}

/// Evaluates all allied champ select slots, premade party groups (`>= 2` shared valid games),
/// and per-group most recent shared game V2 5-dimension MVP.
pub fn analyze_team_ratings(
    team_slots: &[TeamSlot],
    matches_by_puuid: &HashMap<String, MatchFetchResult>,
    config: &RatingTitlesConfig,
) -> TeamRatingsSnapshot {
    let mut slots: Vec<SummonerRatingSummary> = team_slots
        .iter()
        .map(|slot| {
            let fetch_res = if slot.puuid.is_empty() {
                None
            } else {
                matches_by_puuid.get(&slot.puuid)
            };
            evaluate_single_player(slot.cell_id, &slot.puuid, fetch_res, config)
        })
        .collect();

    let mut puuid_to_slot: HashMap<&str, usize> = HashMap::new();
    for (idx, slot) in team_slots.iter().enumerate() {
        if !slot.puuid.is_empty() {
            puuid_to_slot.entry(slot.puuid.as_str()).or_insert(idx);
        }
    }

    let mut unique_matches: HashMap<u64, &NormalizedMatch> = HashMap::new();
    for res in matches_by_puuid.values() {
        if let MatchFetchResult::Ok(matches) = res {
            for m in matches {
                if !is_valid_kiwi_match(m) {
                    continue;
                }
                unique_matches
                    .entry(m.game_id)
                    .and_modify(|existing| {
                        if m.participants.len() > existing.participants.len() {
                            *existing = m;
                        }
                    })
                    .or_insert(m);
            }
        }
    }

    let mut edge_weights: HashMap<(usize, usize), usize> = HashMap::new();
    let mut match_allied_cohorts: Vec<(&NormalizedMatch, HashMap<usize, &NormalizedParticipant>)> =
        Vec::new();

    for m in unique_matches.values() {
        let mut by_team: HashMap<i32, HashMap<usize, &NormalizedParticipant>> = HashMap::new();
        for p in &m.participants {
            if let Some(&slot_idx) = puuid_to_slot.get(p.puuid.as_str()) {
                by_team
                    .entry(p.team_id)
                    .or_default()
                    .entry(slot_idx)
                    .or_insert(p);
            }
        }

        for (_team_id, cohort) in by_team {
            if cohort.len() < 2 {
                continue;
            }
            let mut indices: Vec<usize> = cohort.keys().copied().collect();
            indices.sort_unstable();
            for i in 0..indices.len() {
                for j in (i + 1)..indices.len() {
                    *edge_weights.entry((indices[i], indices[j])).or_insert(0) += 1;
                }
            }
            match_allied_cohorts.push((*m, cohort));
        }
    }

    let mut uf = UnionFind::new(team_slots.len());
    for (&(u, v), &shared) in &edge_weights {
        if shared >= PREMADE_MIN_SHARED_GAMES {
            uf.union(u, v);
        }
    }

    let mut comps: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for idx in 0..team_slots.len() {
        if !team_slots[idx].puuid.is_empty() {
            let root = uf.find(idx);
            comps.entry(root).or_default().push(idx);
        }
    }

    let mut raw_groups: Vec<Vec<usize>> = comps
        .into_values()
        .filter(|members| members.len() >= 2)
        .map(|mut members| {
            members.sort_unstable();
            members
        })
        .collect();
    raw_groups.sort_by_key(|members| members[0]);

    let mut premade_groups = Vec::with_capacity(raw_groups.len());
    for (g_idx, members) in raw_groups.into_iter().enumerate() {
        let group_id = (g_idx + 1) as u8;
        let member_set: HashSet<usize> = members.iter().copied().collect();

        for &slot_idx in &members {
            slots[slot_idx].premade_group_id = Some(group_id);
        }

        let mut shared_game_ids: HashSet<u64> = HashSet::new();
        let mut latest_match: Option<(&NormalizedMatch, Vec<(usize, &NormalizedParticipant)>)> =
            None;

        for (m, cohort) in &match_allied_cohorts {
            let participants_in_group: Vec<(usize, &NormalizedParticipant)> = cohort
                .iter()
                .filter(|&(&slot_idx, _)| member_set.contains(&slot_idx))
                .map(|(&slot_idx, &p)| (slot_idx, p))
                .collect();
            if participants_in_group.len() < 2 {
                continue;
            }
            shared_game_ids.insert(m.game_id);

            let is_newer = match &latest_match {
                None => true,
                Some((prev_m, _)) => {
                    (m.game_creation, m.game_id) > (prev_m.game_creation, prev_m.game_id)
                }
            };
            if is_newer {
                latest_match = Some((*m, participants_in_group));
            }
        }

        let mut last_mvp_slot_index = None;
        if let Some((m, group_participants)) = latest_match {
            if group_participants.len() >= 2 {
                let mut best: Option<(usize, f64, f64, u64)> = None;
                for (slot_idx, p) in group_participants {
                    let perf = score_single_match_performance(m, p);
                    let kda =
                        kda_ratio(u64::from(p.kills), u64::from(p.deaths), u64::from(p.assists));
                    let dmg = p.damage_dealt;
                    let replace = match best {
                        None => true,
                        Some((b_slot, b_perf, b_kda, b_dmg)) => perf
                            .total_cmp(&b_perf)
                            .then_with(|| kda.total_cmp(&b_kda))
                            .then_with(|| dmg.cmp(&b_dmg))
                            .then_with(|| b_slot.cmp(&slot_idx))
                            .is_gt(),
                    };
                    if replace {
                        best = Some((slot_idx, perf, kda, dmg));
                    }
                }
                if let Some((best_slot, _, _, _)) = best {
                    last_mvp_slot_index = Some(best_slot);
                    slots[best_slot].is_premade_last_mvp = true;
                }
            }
        }

        premade_groups.push(PremadeGroupSummary {
            group_id,
            slot_indices: members,
            shared_games: shared_game_ids.len(),
            last_mvp_slot_index,
        });
    }

    TeamRatingsSnapshot {
        slots,
        premade_groups,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lcu::parse_lcu_matches_json;
    use crate::sgp::parse_sgp_matches_json;

    #[allow(clippy::too_many_arguments)]
    fn make_part(
        puuid: &str,
        team_id: i32,
        win: bool,
        k: u32,
        d: u32,
        a: u32,
        dmg: u64,
        taken: u64,
        mitigated: u64,
        heal: u64,
        shield: u64,
        cc: u64,
        gold: u64,
    ) -> NormalizedParticipant {
        NormalizedParticipant {
            puuid: puuid.to_owned(),
            team_id,
            win,
            kills: k,
            deaths: d,
            assists: a,
            damage_dealt: dmg,
            damage_taken: taken,
            damage_self_mitigated: mitigated,
            heals_on_teammates: heal,
            shielded_on_teammates: shield,
            time_ccing_others: cc,
            gold_earned: gold,
        }
    }

    fn make_slot(cell_id: i32, puuid: &str) -> TeamSlot {
        TeamSlot {
            cell_id,
            champion_id: 157,
            pick_intent: 0,
            assigned_position: String::new(),
            puuid: puuid.to_owned(),
        }
    }

    #[test]
    fn parses_sgp_and_lcu_v2_telemetry_with_remake_filter() {
        let sgp_json = serde_json::json!({
            "games": [
                {
                    "json": {
                        "gameId": 1001,
                        "gameCreation": 1700000100000i64,
                        "gameDuration": 720,
                        "gameMode": "KIWI",
                        "queueId": 2400,
                        "participants": [
                            {
                                "puuid": "p1",
                                "teamId": 100,
                                "win": true,
                                "kills": 12,
                                "deaths": 4,
                                "assists": 18,
                                "totalDamageDealtToChampions": 32000,
                                "totalDamageTaken": 18000,
                                "damageSelfMitigated": 24000,
                                "totalHealsOnTeammates": 1200,
                                "totalDamageShieldedOnTeammates": 800,
                                "timeCCingOthers": 48,
                                "goldEarned": 14000
                            }
                        ]
                    }
                },
                {
                    "json": {
                        "gameId": 1002,
                        "gameCreation": 1700000200000i64,
                        "gameDuration": 180,
                        "gameMode": "KIWI",
                        "queueId": 2400,
                        "participants": []
                    }
                }
            ]
        });
        let sgp_matches = parse_sgp_matches_json(&sgp_json).expect("sgp parse");
        let valid_sgp: Vec<_> = sgp_matches.into_iter().filter(is_valid_kiwi_match).collect();
        assert_eq!(valid_sgp.len(), 1);
        assert_eq!(valid_sgp[0].participants[0].damage_self_mitigated, 24000);
        assert_eq!(valid_sgp[0].participants[0].time_ccing_others, 48);

        let lcu_json = serde_json::json!({
            "games": {
                "games": [
                    {
                        "gameId": 2001,
                        "gameCreation": 1700000300000i64,
                        "gameDuration": 900,
                        "gameMode": "KIWI",
                        "queueId": 2400,
                        "participantIdentities": [
                            { "participantId": 1, "player": { "puuid": "p1" } }
                        ],
                        "participants": [
                            {
                                "participantId": 1,
                                "teamId": 200,
                                "stats": {
                                    "win": false,
                                    "kills": 8,
                                    "deaths": 5,
                                    "assists": 14,
                                    "totalDamageDealtToChampions": 24000,
                                    "totalDamageTaken": 22000,
                                    "damageSelfMitigated": 19500,
                                    "totalHealsOnTeammates": 0,
                                    "totalDamageShieldedOnTeammates": 2500,
                                    "timeCCingOthers": 36,
                                    "goldEarned": 12000
                                }
                            }
                        ]
                    }
                ]
            }
        });
        let lcu_matches = parse_lcu_matches_json(&lcu_json);
        assert_eq!(lcu_matches.len(), 1);
        assert_eq!(lcu_matches[0].participants[0].damage_self_mitigated, 19500);
        assert_eq!(lcu_matches[0].participants[0].time_ccing_others, 36);
    }

    #[test]
    fn all_remake_and_zero_denominator_safety() {
        let cfg = RatingTitlesConfig::default();

        // All matches < 300s -> InsufficientSamples, zero panic / zero NaN
        let remakes: Vec<NormalizedMatch> = (1..=5)
            .map(|i| NormalizedMatch {
                game_id: i,
                game_creation: i as i64 * 1000,
                game_duration: 240,
                game_mode: "KIWI".to_owned(),
                queue_id: Some(2400),
                participants: vec![make_part(
                    "remaker", 100, true, 1, 0, 1, 1000, 500, 0, 0, 0, 0, 2000,
                )],
            })
            .collect();

        // Full zero stats across 3 valid games -> tests every denominator == 0 branch
        let zero_matches: Vec<NormalizedMatch> = (1..=3)
            .map(|i| NormalizedMatch {
                game_id: 50 + i,
                game_creation: 5000 + i as i64,
                game_duration: 600,
                game_mode: "KIWI".to_owned(),
                queue_id: Some(2400),
                participants: vec![
                    make_part("zeros", 100, true, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0),
                    make_part("zmate", 100, true, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0),
                ],
            })
            .collect();

        let mut map = HashMap::new();
        map.insert("remaker".to_owned(), MatchFetchResult::Ok(remakes));
        map.insert("zeros".to_owned(), MatchFetchResult::Ok(zero_matches));

        let snap = analyze_team_ratings(
            &[make_slot(0, "remaker"), make_slot(1, "zeros")],
            &map,
            &cfg,
        );

        assert_eq!(snap.slots[0].status, SummonerRatingStatus::InsufficientSamples);
        assert_eq!(snap.slots[0].games_count, 0);
        assert!(!snap.slots[0].win_rate.is_nan());
        assert!(!snap.slots[0].avg_kda.is_nan());

        assert_eq!(snap.slots[1].status, SummonerRatingStatus::Ready);
        assert!(!snap.slots[1].win_rate.is_nan());
        assert!(!snap.slots[1].avg_kda.is_nan());
    }

    #[test]
    fn global_kda_prevents_zero_death_match_spike() {
        let cfg = RatingTitlesConfig::default();

        // Both players have exact total K/D/A = 10/5/10 (global KDA = 4.0) across 3 matches,
        // and identical per-match dmg/taken/gold/KP totals.
        let p_spike: Vec<NormalizedMatch> = vec![
            NormalizedMatch {
                game_id: 1,
                game_creation: 1000,
                game_duration: 900,
                game_mode: "KIWI".to_owned(),
                queue_id: Some(2400),
                participants: vec![
                    make_part("spike", 100, true, 10, 0, 10, 30000, 18000, 12000, 0, 0, 20, 12000),
                    make_part("m1", 100, true, 10, 4, 10, 30000, 18000, 12000, 0, 0, 20, 12000),
                ],
            },
            NormalizedMatch {
                game_id: 2,
                game_creation: 2000,
                game_duration: 900,
                game_mode: "KIWI".to_owned(),
                queue_id: Some(2400),
                participants: vec![
                    make_part("spike", 100, true, 0, 3, 0, 30000, 18000, 12000, 0, 0, 20, 12000),
                    make_part("m1", 100, true, 0, 3, 0, 30000, 18000, 12000, 0, 0, 20, 12000),
                ],
            },
            NormalizedMatch {
                game_id: 3,
                game_creation: 3000,
                game_duration: 900,
                game_mode: "KIWI".to_owned(),
                queue_id: Some(2400),
                participants: vec![
                    make_part("spike", 100, false, 0, 2, 0, 30000, 18000, 12000, 0, 0, 20, 12000),
                    make_part("m1", 100, false, 0, 3, 0, 30000, 18000, 12000, 0, 0, 20, 12000),
                ],
            },
        ];

        // Same team kill proportions per match, but steady deaths [2, 2, 1] -> global KDA 4.0
        let p_steady: Vec<NormalizedMatch> = vec![
            NormalizedMatch {
                game_id: 11,
                game_creation: 1000,
                game_duration: 900,
                game_mode: "KIWI".to_owned(),
                queue_id: Some(2400),
                participants: vec![
                    make_part("steady", 100, true, 4, 2, 4, 30000, 18000, 12000, 0, 0, 20, 12000),
                    make_part("m2", 100, true, 4, 2, 4, 30000, 18000, 12000, 0, 0, 20, 12000),
                ],
            },
            NormalizedMatch {
                game_id: 12,
                game_creation: 2000,
                game_duration: 900,
                game_mode: "KIWI".to_owned(),
                queue_id: Some(2400),
                participants: vec![
                    make_part("steady", 100, true, 3, 2, 3, 30000, 18000, 12000, 0, 0, 20, 12000),
                    make_part("m2", 100, true, 3, 2, 3, 30000, 18000, 12000, 0, 0, 20, 12000),
                ],
            },
            NormalizedMatch {
                game_id: 13,
                game_creation: 3000,
                game_duration: 900,
                game_mode: "KIWI".to_owned(),
                queue_id: Some(2400),
                participants: vec![
                    make_part("steady", 100, false, 3, 1, 3, 30000, 18000, 12000, 0, 0, 20, 12000),
                    make_part("m2", 100, false, 3, 2, 3, 30000, 18000, 12000, 0, 0, 20, 12000),
                ],
            },
        ];

        let mut map = HashMap::new();
        map.insert("spike".to_owned(), MatchFetchResult::Ok(p_spike));
        map.insert("steady".to_owned(), MatchFetchResult::Ok(p_steady));

        let snap = analyze_team_ratings(
            &[make_slot(0, "spike"), make_slot(1, "steady")],
            &map,
            &cfg,
        );
        assert!((snap.slots[0].avg_kda - 4.0).abs() < 1e-6);
        assert!((snap.slots[1].avg_kda - 4.0).abs() < 1e-6);
        // `spike` had 0 KP in 2 games vs `steady` having 100% KP across all 3 -> global KDA matches, never spikes above steady!
        assert!(snap.slots[0].score <= snap.slots[1].score);
    }

    #[test]
    fn v2_role_fairness_carry_tank_enchanter_and_average_player() {
        let cfg = RatingTitlesConfig::default();

        // Equal 60% WR across 10 games (6W 4L)
        let carry_matches: Vec<NormalizedMatch> = (1..=10)
            .map(|i| NormalizedMatch {
                game_id: 100 + i,
                game_creation: i as i64 * 1000,
                game_duration: 900,
                game_mode: "KIWI".to_owned(),
                queue_id: Some(2400),
                participants: vec![
                    make_part(
                        "carry", 100, i <= 6, 15, 5, 12, 45000, 14000, 8000, 0, 0, 20, 14000,
                    ),
                    make_part(
                        "mate1", 100, i <= 6, 12, 6, 10, 15000, 26000, 18000, 0, 0, 20, 12000,
                    ),
                ],
            })
            .collect();

        let tank_matches: Vec<NormalizedMatch> = (1..=10)
            .map(|i| NormalizedMatch {
                game_id: 200 + i,
                game_creation: i as i64 * 1000,
                game_duration: 900,
                game_mode: "KIWI".to_owned(),
                queue_id: Some(2400),
                participants: vec![
                    make_part(
                        "tank", 100, i <= 6, 6, 6, 22, 18000, 34000, 32000, 2000, 3000, 65, 13000,
                    ),
                    make_part(
                        "mate2", 100, i <= 6, 18, 6, 6, 30000, 12000, 8000, 0, 0, 10, 12000,
                    ),
                ],
            })
            .collect();

        let ench_matches: Vec<NormalizedMatch> = (1..=10)
            .map(|i| NormalizedMatch {
                game_id: 300 + i,
                game_creation: i as i64 * 1000,
                game_duration: 900,
                game_mode: "KIWI".to_owned(),
                queue_id: Some(2400),
                participants: vec![
                    make_part(
                        "ench", 100, i <= 6, 4, 5, 25, 14000, 13000, 5000, 14000, 12000, 25, 12000,
                    ),
                    make_part(
                        "mate3", 100, i <= 6, 20, 6, 6, 34000, 24000, 18000, 0, 0, 15, 13000,
                    ),
                ],
            })
            .collect();

        // Average 50% player (5W 5L across 10 games) should land cleanly in 54..=60
        let avg_matches: Vec<NormalizedMatch> = (1..=10)
            .map(|i| NormalizedMatch {
                game_id: 400 + i,
                game_creation: i as i64 * 1000,
                game_duration: 900,
                game_mode: "KIWI".to_owned(),
                queue_id: Some(2400),
                participants: vec![
                    make_part(
                        "avg", 100, i <= 5, 9, 6, 12, 29000, 22000, 16000, 0, 0, 22, 12000,
                    ),
                    make_part(
                        "mate4", 100, i <= 5, 25, 6, 8, 25000, 22000, 16000, 0, 0, 18, 12000,
                    ),
                ],
            })
            .collect();

        let mut map = HashMap::new();
        map.insert("carry".to_owned(), MatchFetchResult::Ok(carry_matches));
        map.insert("tank".to_owned(), MatchFetchResult::Ok(tank_matches));
        map.insert("ench".to_owned(), MatchFetchResult::Ok(ench_matches));
        map.insert("avg".to_owned(), MatchFetchResult::Ok(avg_matches));
        map.insert("priv".to_owned(), MatchFetchResult::Private);

        let snap = analyze_team_ratings(
            &[
                make_slot(0, "carry"),
                make_slot(1, "tank"),
                make_slot(2, "ench"),
                make_slot(3, "avg"),
                make_slot(4, "priv"),
            ],
            &map,
            &cfg,
        );

        assert!(snap.slots[0].score >= 70, "carry score {}", snap.slots[0].score);
        assert!(snap.slots[1].score >= 70, "tank score {}", snap.slots[1].score);
        assert!(snap.slots[2].score >= 70, "ench score {}", snap.slots[2].score);
        assert!(
            (54..=60).contains(&snap.slots[3].score),
            "avg 50% player score {} expected in 54..=60",
            snap.slots[3].score
        );
        assert_eq!(snap.slots[4].status, SummonerRatingStatus::Private);
        assert_eq!(snap.slots[4].title, "隐藏");
    }

    #[test]
    fn sgp_vs_lcu_single_participant_fallback_parity() {
        let cfg = RatingTitlesConfig::default();

        // 5 valid 15-min matches (3W 2L) with matching per-minute and team-relative performance
        let sgp_matches: Vec<NormalizedMatch> = (1..=5)
            .map(|i| NormalizedMatch {
                game_id: 500 + i,
                game_creation: i as i64 * 1000,
                game_duration: 900,
                game_mode: "KIWI".to_owned(),
                queue_id: Some(2400),
                participants: vec![
                    make_part(
                        "sgp_u", 100, i <= 3, 10, 5, 17, 39000, 28000, 27000, 0, 0, 45, 12800,
                    ),
                    make_part(
                        "sgp_m", 100, i <= 3, 28, 5, 6, 33000, 26000, 24000, 0, 0, 35, 11950,
                    ),
                ],
            })
            .collect();

        // Exact same player telemetry, but single participant (simulating LCU fallback)
        let lcu_matches: Vec<NormalizedMatch> = sgp_matches
            .iter()
            .map(|m| {
                let mut cloned = m.clone();
                let mut only_me = m.participants[0].clone();
                only_me.puuid = "lcu_u".to_owned();
                cloned.participants = vec![only_me];
                cloned.game_id += 1000;
                cloned
            })
            .collect();

        let mut map = HashMap::new();
        map.insert("sgp_u".to_owned(), MatchFetchResult::Ok(sgp_matches));
        map.insert("lcu_u".to_owned(), MatchFetchResult::Ok(lcu_matches));

        let snap = analyze_team_ratings(
            &[make_slot(0, "sgp_u"), make_slot(1, "lcu_u")],
            &map,
            &cfg,
        );

        let diff = snap.slots[0].score.abs_diff(snap.slots[1].score);
        assert!(
            diff <= 5,
            "SGP ({}) vs LCU fallback ({}) diff {} > 5",
            snap.slots[0].score,
            snap.slots[1].score,
            diff
        );
    }

    #[test]
    fn rating_titles_v2_thresholds_and_load_or_init() {
        let dir = std::env::temp_dir().join(format!("hexrs_titles_v2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cfg_path = dir.join("rating_titles.json");

        let init_cfg = RatingTitlesConfig::load_or_init(&cfg_path);
        assert_eq!(init_cfg, RatingTitlesConfig::default());
        assert_eq!(init_cfg.resolve(SummonerRatingStatus::Ready, 82), (0, "夯".to_owned()));
        assert_eq!(init_cfg.resolve(SummonerRatingStatus::Ready, 70), (1, "顶级".to_owned()));
        assert_eq!(init_cfg.resolve(SummonerRatingStatus::Ready, 58), (2, "人上人".to_owned()));
        assert_eq!(init_cfg.resolve(SummonerRatingStatus::Ready, 45), (3, "NPC".to_owned()));
        assert_eq!(init_cfg.resolve(SummonerRatingStatus::Ready, 44), (4, "拉完了".to_owned()));

        std::fs::write(&cfg_path, "{invalid json").unwrap();
        let fallback = RatingTitlesConfig::load_or_init(&cfg_path);
        assert_eq!(fallback, RatingTitlesConfig::default());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn premade_two_plus_three_and_private_penetration_and_streaks() {
        let cfg = RatingTitlesConfig::default();

        let ab_old = NormalizedMatch {
            game_id: 9001,
            game_creation: 1000,
            game_duration: 800,
            game_mode: "KIWI".to_owned(),
            queue_id: Some(2400),
            participants: vec![
                make_part("A", 100, false, 4, 8, 6, 15000, 15000, 8000, 0, 0, 10, 10000),
                make_part("B", 100, false, 10, 4, 12, 30000, 18000, 12000, 0, 0, 20, 13000),
            ],
        };
        let ab_new = NormalizedMatch {
            game_id: 9002,
            game_creation: 2000,
            game_duration: 850,
            game_mode: "KIWI".to_owned(),
            queue_id: Some(2400),
            participants: vec![
                make_part("A", 100, true, 18, 2, 14, 42000, 18000, 15000, 0, 0, 30, 16000),
                make_part("B", 100, true, 5, 7, 10, 14000, 15000, 8000, 0, 0, 10, 11000),
            ],
        };
        let b_loss1 = NormalizedMatch {
            game_id: 9003,
            game_creation: 3000,
            game_duration: 700,
            game_mode: "KIWI".to_owned(),
            queue_id: Some(2400),
            participants: vec![
                make_part("B", 100, false, 3, 6, 5, 14000, 14000, 6000, 0, 0, 8, 10000),
                make_part("C", 100, false, 4, 6, 6, 16000, 14000, 6000, 0, 0, 8, 10500),
            ],
        };
        let b_loss2 = NormalizedMatch {
            game_id: 9004,
            game_creation: 4000,
            game_duration: 700,
            game_mode: "KIWI".to_owned(),
            queue_id: Some(2400),
            participants: vec![
                make_part("B", 100, false, 2, 7, 4, 12000, 14000, 6000, 0, 0, 8, 9500),
                make_part("X1", 100, false, 4, 6, 6, 16000, 14000, 6000, 0, 0, 8, 10500),
            ],
        };
        let b_loss3 = NormalizedMatch {
            game_id: 9005,
            game_creation: 5000,
            game_duration: 700,
            game_mode: "KIWI".to_owned(),
            queue_id: Some(2400),
            participants: vec![
                make_part("B", 100, false, 2, 7, 4, 12000, 14000, 6000, 0, 0, 8, 9500),
                make_part("X2", 100, false, 4, 6, 6, 16000, 14000, 6000, 0, 0, 8, 10500),
            ],
        };

        let cd_match = NormalizedMatch {
            game_id: 9100,
            game_creation: 1500,
            game_duration: 800,
            game_mode: "KIWI".to_owned(),
            queue_id: Some(2400),
            participants: vec![
                make_part("C", 200, true, 10, 4, 10, 24000, 18000, 10000, 0, 0, 15, 13000),
                make_part("D", 200, true, 8, 5, 12, 22000, 18000, 10000, 0, 0, 15, 12500),
            ],
        };
        let cd_match2 = NormalizedMatch {
            game_id: 9101,
            game_creation: 1600,
            game_duration: 800,
            game_mode: "KIWI".to_owned(),
            queue_id: Some(2400),
            participants: vec![
                make_part("C", 200, true, 12, 3, 14, 28000, 18000, 10000, 0, 0, 15, 14000),
                make_part("D", 200, true, 6, 5, 10, 19000, 18000, 10000, 0, 0, 15, 12000),
            ],
        };
        let de_match1 = NormalizedMatch {
            game_id: 9101,
            game_creation: 1600,
            game_duration: 800,
            game_mode: "KIWI".to_owned(),
            queue_id: Some(2400),
            participants: vec![
                make_part("C", 200, true, 12, 3, 14, 28000, 18000, 10000, 0, 0, 15, 14000),
                make_part("D", 200, true, 6, 5, 10, 19000, 18000, 10000, 0, 0, 15, 12000),
                make_part("E", 200, true, 4, 6, 15, 15000, 24000, 12000, 0, 0, 20, 11500),
            ],
        };
        let de_match2 = NormalizedMatch {
            game_id: 9102,
            game_creation: 6000,
            game_duration: 900,
            game_mode: "KIWI".to_owned(),
            queue_id: Some(2400),
            participants: vec![
                make_part("D", 200, true, 19, 2, 16, 45000, 22000, 18000, 0, 0, 35, 17000),
                make_part("E", 200, true, 4, 6, 12, 14000, 18000, 10000, 0, 0, 15, 11000),
            ],
        };

        let mut map = HashMap::new();
        map.insert("A".to_owned(), MatchFetchResult::Private);
        map.insert(
            "B".to_owned(),
            MatchFetchResult::Ok(vec![ab_old, ab_new, b_loss1.clone(), b_loss2, b_loss3]),
        );
        map.insert(
            "C".to_owned(),
            MatchFetchResult::Ok(vec![b_loss1, cd_match, cd_match2]),
        );
        map.insert(
            "D".to_owned(),
            MatchFetchResult::Ok(vec![de_match1.clone(), de_match2.clone()]),
        );
        map.insert("E".to_owned(), MatchFetchResult::Ok(vec![de_match1, de_match2]));

        let slots = vec![
            make_slot(0, "A"),
            make_slot(1, "B"),
            make_slot(2, "C"),
            make_slot(3, "D"),
            make_slot(4, "E"),
        ];

        let snap = analyze_team_ratings(&slots, &map, &cfg);
        assert_eq!(snap.premade_groups.len(), 2);

        assert_eq!(snap.premade_groups[0].group_id, 1);
        assert_eq!(snap.premade_groups[0].slot_indices, vec![0, 1]);
        assert_eq!(snap.premade_groups[0].shared_games, 2);
        assert_eq!(snap.premade_groups[0].last_mvp_slot_index, Some(0));
        assert_eq!(snap.slots[0].status, SummonerRatingStatus::Private);
        assert_eq!(snap.slots[0].premade_group_id, Some(1));
        assert!(snap.slots[0].is_premade_last_mvp);
        assert_eq!(snap.slots[1].premade_group_id, Some(1));
        assert!(!snap.slots[1].is_premade_last_mvp);
        assert_eq!(snap.slots[1].streak, -3);

        assert_eq!(snap.premade_groups[1].group_id, 2);
        assert_eq!(snap.premade_groups[1].slot_indices, vec![2, 3, 4]);
        assert_eq!(snap.premade_groups[1].shared_games, 3);
        assert_eq!(snap.premade_groups[1].last_mvp_slot_index, Some(3));
        assert!(snap.slots[3].is_premade_last_mvp);
    }
}
