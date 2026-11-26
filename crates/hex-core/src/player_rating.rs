//! ARAM Mayhem (`queueId == 2400` / `KIWI`) 6-dimension player rating,
//! premade party detection (`TeamUpGraph` + `UnionFind`), and per-group last-game MVP scoring.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::models::{
    PremadeGroupSummary, RatingTitlesConfig, SummonerRatingStatus, SummonerRatingSummary,
    TeamRatingsSnapshot, TeamSlot,
};

const MIN_VALID_KIWI_DURATION_SECS: u64 = 300;
const MIN_READY_SAMPLE_GAMES: usize = 3;
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
    pub heals_on_teammates: u64,
    pub shielded_on_teammates: u64,
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
    kda_pts: f64,
    output_pts: f64,
    taken_pts: f64,
    heal_shield_pts: f64,
    kp_gold_pts: f64,
}

impl MatchSubScores {
    fn sum_5dim(self) -> f64 {
        self.kda_pts + self.output_pts + self.taken_pts + self.heal_shield_pts + self.kp_gold_pts
    }
}

fn kda_ratio(kills: u64, deaths: u64, assists: u64) -> f64 {
    (kills + assists) as f64 / (deaths.max(1) as f64)
}

fn kda_points(kda: f64) -> f64 {
    ((kda - 1.2).max(0.0).sqrt() / 4.8_f64.sqrt()).clamp(0.0, 1.0) * 20.0
}

fn compute_match_subscores(m: &NormalizedMatch, me: &NormalizedParticipant) -> MatchSubScores {
    let duration_mins = (m.game_duration as f64 / 60.0).max(1.0);
    let team_mates: Vec<&NormalizedParticipant> = m
        .participants
        .iter()
        .filter(|p| p.team_id == me.team_id)
        .collect();
    let has_full_team = team_mates.len() >= 2;

    let kda = kda_ratio(u64::from(me.kills), u64::from(me.deaths), u64::from(me.assists));
    let kda_pts = kda_points(kda);
    let heal_shield = (me.heals_on_teammates + me.shielded_on_teammates) as f64;

    let (output_pts, taken_pts, heal_shield_pts, kp_gold_pts) = if has_full_team {
        let count = team_mates.len() as f64;
        let team_avg_dmg = team_mates.iter().map(|p| p.damage_dealt as f64).sum::<f64>() / count;
        let team_avg_taken =
            team_mates.iter().map(|p| p.damage_taken as f64).sum::<f64>() / count;
        let team_avg_gold = team_mates.iter().map(|p| p.gold_earned as f64).sum::<f64>() / count;
        let team_kills: u64 = team_mates.iter().map(|p| u64::from(p.kills)).sum();

        let out = if team_avg_dmg > 0.0 {
            map_range(me.damage_dealt as f64 / team_avg_dmg, 0.6, 1.6, 18.0)
        } else {
            0.0
        };
        let tkn = if team_avg_taken > 0.0 {
            map_range(me.damage_taken as f64 / team_avg_taken, 0.6, 1.6, 10.0)
        } else {
            0.0
        };
        let hs = if team_avg_taken > 0.0 {
            map_range(heal_shield / team_avg_taken, 0.15, 1.20, 7.0)
        } else {
            0.0
        };
        let kp = if team_kills > 0 {
            let ratio = ((me.kills + me.assists) as f64 / team_kills as f64).min(1.0);
            map_range(ratio, 0.35, 0.85, 10.0)
        } else {
            map_range((me.kills + me.assists) as f64 / duration_mins, 0.6, 2.2, 10.0)
        };
        let gold = if team_avg_gold > 0.0 {
            map_range(me.gold_earned as f64 / team_avg_gold, 0.75, 1.25, 5.0)
        } else {
            0.0
        };
        (out, tkn, hs, kp + gold)
    } else {
        let out = map_range(me.damage_dealt as f64 / duration_mins, 1200.0, 3800.0, 18.0);
        let tkn = map_range(me.damage_taken as f64 / duration_mins, 900.0, 3000.0, 10.0);
        let hs = map_range(heal_shield / duration_mins, 150.0, 1400.0, 7.0);
        let kp = map_range((me.kills + me.assists) as f64 / duration_mins, 0.6, 2.2, 10.0);
        let gold = map_range(me.gold_earned as f64 / duration_mins, 550.0, 1050.0, 5.0);
        (out, tkn, hs, kp + gold)
    };

    MatchSubScores {
        kda_pts,
        output_pts,
        taken_pts,
        heal_shield_pts,
        kp_gold_pts,
    }
}

/// Computes the 5-dimension single-match performance score (`0.0..=70.0`, excluding win rate).
pub fn score_single_match_performance(m: &NormalizedMatch, me: &NormalizedParticipant) -> f64 {
    compute_match_subscores(m, me).sum_5dim()
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

            let wr_pts = ((win_rate - 0.25) / 0.55).clamp(0.0, 1.0) * 30.0;
            let agg_kda_pts = kda_points(avg_kda);

            let mut out_sum = 0.0;
            let mut tkn_sum = 0.0;
            let mut hs_sum = 0.0;
            let mut kpg_sum = 0.0;
            for (m, me) in &valid {
                let sub = compute_match_subscores(m, me);
                out_sum += sub.output_pts;
                tkn_sum += sub.taken_pts;
                hs_sum += sub.heal_shield_pts;
                kpg_sum += sub.kp_gold_pts;
            }
            let n_f = games_n as f64;
            let raw_score = wr_pts
                + agg_kda_pts
                + (out_sum / n_f)
                + (tkn_sum / n_f)
                + (hs_sum / n_f)
                + (kpg_sum / n_f);
            let score = raw_score.round().clamp(0.0, 100.0) as u8;
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
/// and per-group most recent shared game 5-dimension MVP.
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

    // Build undirected co-occurrence graph across valid shared matches on the same teamId.
    let mut edge_weights: HashMap<(usize, usize), usize> = HashMap::new();
    // Retain per-match allied team cohorts for fast per-group shared game & last-MVP scan.
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
                Some((prev_m, _)) => (m.game_creation, m.game_id) > (prev_m.game_creation, prev_m.game_id),
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
                    let kda = kda_ratio(u64::from(p.kills), u64::from(p.deaths), u64::from(p.assists));
                    let dmg = p.damage_dealt;
                    let replace = match best {
                        None => true,
                        Some((b_slot, b_perf, b_kda, b_dmg)) => {
                            perf.total_cmp(&b_perf)
                                .then_with(|| kda.total_cmp(&b_kda))
                                .then_with(|| dmg.cmp(&b_dmg))
                                .then_with(|| b_slot.cmp(&slot_idx))
                                .is_gt()
                        }
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

    fn make_part(
        puuid: &str,
        team_id: i32,
        win: bool,
        k: u32,
        d: u32,
        a: u32,
        dmg: u64,
        taken: u64,
        heal: u64,
        shield: u64,
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
            heals_on_teammates: heal,
            shielded_on_teammates: shield,
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

    fn balanced_team_match(
        game_id: u64,
        creation: i64,
        duration: u64,
        queue_id: Option<i32>,
        mode: &str,
        me: NormalizedParticipant,
        ally: NormalizedParticipant,
    ) -> NormalizedMatch {
        NormalizedMatch {
            game_id,
            game_creation: creation,
            game_duration: duration,
            game_mode: mode.to_owned(),
            queue_id,
            participants: vec![me, ally],
        }
    }

    #[test]
    fn parses_sgp_and_lcu_matches_with_kiwi_and_remake_filters() {
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
                                "totalHealsOnTeammates": 1200,
                                "totalDamageShieldedOnTeammates": 800,
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
        assert_eq!(sgp_matches.len(), 2);
        let valid_sgp: Vec<_> = sgp_matches.into_iter().filter(is_valid_kiwi_match).collect();
        assert_eq!(valid_sgp.len(), 1);
        assert_eq!(valid_sgp[0].game_id, 1001);
        assert_eq!(valid_sgp[0].participants[0].heals_on_teammates, 1200);

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
                                    "totalHealsOnTeammates": 0,
                                    "totalDamageShieldedOnTeammates": 2500,
                                    "goldEarned": 12000
                                }
                            }
                        ]
                    },
                    {
                        "gameId": 2002,
                        "gameCreation": 1700000400000i64,
                        "gameDuration": 1000,
                        "gameMode": "ARAM",
                        "queueId": 450,
                        "participantIdentities": [],
                        "participants": []
                    },
                    {
                        "gameId": 2003,
                        "gameCreation": 1700000500000i64,
                        "gameDuration": 250,
                        "gameMode": "KIWI",
                        "queueId": 2400,
                        "participantIdentities": [],
                        "participants": []
                    }
                ]
            }
        });
        let lcu_matches = parse_lcu_matches_json(&lcu_json);
        assert_eq!(lcu_matches.len(), 1);
        assert_eq!(lcu_matches[0].game_id, 2001);
        assert_eq!(lcu_matches[0].participants[0].puuid, "p1");
        assert_eq!(lcu_matches[0].participants[0].shielded_on_teammates, 2500);
    }

    #[test]
    fn composite_scoring_boundaries_and_statuses() {
        let cfg = RatingTitlesConfig::default();

        // Carry with strong stats across 4 games -> 夯 (>= 85)
        let carry_matches: Vec<NormalizedMatch> = (1..=4)
            .map(|i| {
                balanced_team_match(
                    100 + i,
                    1000 + i as i64,
                    900,
                    Some(2400),
                    "KIWI",
                    make_part("carry", 100, true, 16, 4, 16, 42000, 18000, 0, 0, 17500),
                    make_part("mate", 100, true, 4, 6, 12, 16000, 18000, 0, 0, 12500),
                )
            })
            .collect();

        // Tank with high damage taken across 4 games -> 顶级 / 人上人
        let tank_matches: Vec<NormalizedMatch> = (1..=4)
            .map(|i| {
                balanced_team_match(
                    200 + i,
                    2000 + i as i64,
                    900,
                    Some(2400),
                    "KIWI",
                    make_part("tank", 100, i <= 3, 5, 6, 22, 18000, 42000, 2000, 3000, 13000),
                    make_part("mate2", 100, i <= 3, 12, 6, 8, 22000, 16000, 0, 0, 13000),
                )
            })
            .collect();

        // Enchanter with heavy healing/shielding -> high heal+shield contribution
        let ench_matches: Vec<NormalizedMatch> = (1..=4)
            .map(|i| {
                balanced_team_match(
                    300 + i,
                    3000 + i as i64,
                    900,
                    Some(2400),
                    "KIWI",
                    make_part("ench", 100, i <= 3, 3, 4, 28, 12000, 12000, 18000, 16000, 12500),
                    make_part("mate3", 100, i <= 3, 18, 5, 10, 28000, 24000, 0, 0, 14000),
                )
            })
            .collect();

        // Low samples (< 3 valid matches)
        let low_matches = vec![balanced_team_match(
            401,
            4001,
            900,
            Some(2400),
            "KIWI",
            make_part("low", 100, true, 10, 3, 10, 25000, 15000, 0, 0, 13000),
            make_part("m4", 100, true, 5, 5, 5, 20000, 15000, 0, 0, 12000),
        )];

        let mut map = HashMap::new();
        map.insert("carry".to_owned(), MatchFetchResult::Ok(carry_matches));
        map.insert("tank".to_owned(), MatchFetchResult::Ok(tank_matches));
        map.insert("ench".to_owned(), MatchFetchResult::Ok(ench_matches));
        map.insert("low".to_owned(), MatchFetchResult::Ok(low_matches));
        map.insert("priv".to_owned(), MatchFetchResult::Private);

        let slots = vec![
            make_slot(0, "carry"),
            make_slot(1, "tank"),
            make_slot(2, "ench"),
            make_slot(3, "low"),
            make_slot(4, "priv"),
        ];

        let snap = analyze_team_ratings(&slots, &map, &cfg);
        assert_eq!(snap.slots[0].status, SummonerRatingStatus::Ready);
        assert!(snap.slots[0].score >= 85, "carry score {}", snap.slots[0].score);
        assert_eq!(snap.slots[0].title, "夯");
        assert_eq!(snap.slots[0].streak, 4);

        assert_eq!(snap.slots[1].status, SummonerRatingStatus::Ready);
        assert!(snap.slots[1].score >= 55, "tank score {}", snap.slots[1].score);

        assert_eq!(snap.slots[2].status, SummonerRatingStatus::Ready);
        assert!(snap.slots[2].score >= 60, "enchanter score {}", snap.slots[2].score);

        assert_eq!(snap.slots[3].status, SummonerRatingStatus::InsufficientSamples);
        assert_eq!(snap.slots[3].title, "样本少");
        assert_eq!(snap.slots[3].games_count, 1);

        assert_eq!(snap.slots[4].status, SummonerRatingStatus::Private);
        assert_eq!(snap.slots[4].title, "隐藏");
    }

    #[test]
    fn rating_titles_config_load_or_init_and_fallback() {
        let dir = std::env::temp_dir().join(format!("hexrs_titles_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cfg_path = dir.join("rating_titles.json");

        let init_cfg = RatingTitlesConfig::load_or_init(&cfg_path);
        assert_eq!(init_cfg, RatingTitlesConfig::default());
        assert_eq!(init_cfg.resolve(SummonerRatingStatus::Ready, 88), (0, "夯".to_owned()));
        assert_eq!(init_cfg.resolve(SummonerRatingStatus::Ready, 75), (1, "顶级".to_owned()));
        assert_eq!(init_cfg.resolve(SummonerRatingStatus::Ready, 60), (2, "人上人".to_owned()));
        assert_eq!(init_cfg.resolve(SummonerRatingStatus::Ready, 42), (3, "NPC".to_owned()));
        assert_eq!(init_cfg.resolve(SummonerRatingStatus::Ready, 20), (4, "拉完了".to_owned()));

        std::fs::write(
            &cfg_path,
            r#"{"tiers":[{"min_score":90,"label":"神"},{"min_score":0,"label":"菜"}],"insufficient_samples":"缺","private_profile":"密"}"#,
        )
        .unwrap();
        let custom = RatingTitlesConfig::load_or_init(&cfg_path);
        assert_eq!(custom.resolve(SummonerRatingStatus::Ready, 91), (0, "神".to_owned()));
        assert_eq!(custom.resolve(SummonerRatingStatus::Private, 0), (4, "密".to_owned()));

        std::fs::write(&cfg_path, "{broken json").unwrap();
        let fallback = RatingTitlesConfig::load_or_init(&cfg_path);
        assert_eq!(fallback, RatingTitlesConfig::default());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn premade_two_plus_three_and_private_penetration_and_streaks() {
        let cfg = RatingTitlesConfig::default();

        // Lobby: [0(A: private), 1(B: public)], [2(C), 3(D), 4(E)] -> 2+3 lobby
        // Plus check that a single random encounter would NOT create a group.
        // Group 1 (A & B): 2 shared games; newest game (id=9002) has A carrying harder than B.
        let ab_old = NormalizedMatch {
            game_id: 9001,
            game_creation: 1000,
            game_duration: 800,
            game_mode: "KIWI".to_owned(),
            queue_id: Some(2400),
            participants: vec![
                make_part("A", 100, false, 4, 8, 6, 15000, 15000, 0, 0, 10000),
                make_part("B", 100, false, 10, 4, 12, 30000, 18000, 0, 0, 13000),
            ],
        };
        let ab_new = NormalizedMatch {
            game_id: 9002,
            game_creation: 2000,
            game_duration: 850,
            game_mode: "KIWI".to_owned(),
            queue_id: Some(2400),
            participants: vec![
                make_part("A", 100, true, 18, 2, 14, 42000, 18000, 0, 0, 16000),
                make_part("B", 100, true, 5, 7, 10, 14000, 15000, 0, 0, 11000),
            ],
        };
        // B also has 3 consecutive losses most recently (to test -3 streak) + 1 encounter with C (< 2 shared)
        let b_loss1 = balanced_team_match(
            9003,
            3000,
            700,
            Some(2400),
            "KIWI",
            make_part("B", 100, false, 3, 6, 5, 14000, 14000, 0, 0, 10000),
            make_part("C", 100, false, 4, 6, 6, 16000, 14000, 0, 0, 10500),
        );
        let b_loss2 = balanced_team_match(
            9004,
            4000,
            700,
            Some(2400),
            "KIWI",
            make_part("B", 100, false, 2, 7, 4, 12000, 14000, 0, 0, 9500),
            make_part("X1", 100, false, 4, 6, 6, 16000, 14000, 0, 0, 10500),
        );
        let b_loss3 = balanced_team_match(
            9005,
            5000,
            700,
            Some(2400),
            "KIWI",
            make_part("B", 100, false, 2, 7, 4, 12000, 14000, 0, 0, 9500),
            make_part("X2", 100, false, 4, 6, 6, 16000, 14000, 0, 0, 10500),
        );

        // Group 2 (C, D, E): C+D played twice, D+E played twice -> UnionFind forms {2, 3, 4}
        // Newest shared match for Group 2 is 9102 where D is the highest 5-dim scorer.
        let cd_match = NormalizedMatch {
            game_id: 9100,
            game_creation: 1500,
            game_duration: 800,
            game_mode: "KIWI".to_owned(),
            queue_id: Some(2400),
            participants: vec![
                make_part("C", 200, true, 10, 4, 10, 24000, 18000, 0, 0, 13000),
                make_part("D", 200, true, 8, 5, 12, 22000, 18000, 0, 0, 12500),
            ],
        };
        let cd_match2 = NormalizedMatch {
            game_id: 9101,
            game_creation: 1600,
            game_duration: 800,
            game_mode: "KIWI".to_owned(),
            queue_id: Some(2400),
            participants: vec![
                make_part("C", 200, true, 12, 3, 14, 28000, 18000, 0, 0, 14000),
                make_part("D", 200, true, 6, 5, 10, 19000, 18000, 0, 0, 12000),
            ],
        };
        let de_match1 = NormalizedMatch {
            game_id: 9101,
            game_creation: 1600,
            game_duration: 800,
            game_mode: "KIWI".to_owned(),
            queue_id: Some(2400),
            participants: vec![
                make_part("C", 200, true, 12, 3, 14, 28000, 18000, 0, 0, 14000),
                make_part("D", 200, true, 6, 5, 10, 19000, 18000, 0, 0, 12000),
                make_part("E", 200, true, 4, 6, 15, 15000, 24000, 0, 0, 11500),
            ],
        };
        let de_match2 = NormalizedMatch {
            game_id: 9102,
            game_creation: 6000,
            game_duration: 900,
            game_mode: "KIWI".to_owned(),
            queue_id: Some(2400),
            participants: vec![
                make_part("D", 200, true, 19, 2, 16, 45000, 22000, 0, 0, 17000),
                make_part("E", 200, true, 4, 6, 12, 14000, 18000, 0, 0, 11000),
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

        // Group 1: slots [0, 1], private A penetrated via B & A won last-game MVP!
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

        // Group 2: slots [2, 3, 4], D won last-game MVP!
        assert_eq!(snap.premade_groups[1].group_id, 2);
        assert_eq!(snap.premade_groups[1].slot_indices, vec![2, 3, 4]);
        assert_eq!(snap.premade_groups[1].shared_games, 3);
        assert_eq!(snap.premade_groups[1].last_mvp_slot_index, Some(3));
        assert!(snap.slots[3].is_premade_last_mvp);
        assert!(!snap.slots[2].is_premade_last_mvp);
        assert!(!snap.slots[4].is_premade_last_mvp);
    }
}
