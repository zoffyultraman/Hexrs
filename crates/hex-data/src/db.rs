//! In-memory SQLite query engine (`< 1ms` lookups) and dual-layer Arena/ARAM recommender.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use hex_core::HeroTier;
use rusqlite::backup::Backup;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::schema::init_schema;
use crate::seed::{lookup_item_name_zh, populate_seed_data};

const SYNERGY_LIFT_THRESHOLD: f64 = 1.15;
const SYNERGY_MIN_SAMPLES: i32 = 30;

/// Returns the local application data directory (`%LOCALAPPDATA%/Hexrs` on Windows).
pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Hexrs")
}

/// Returns the default path to the local `meta.db` database file.
pub fn db_path() -> PathBuf {
    data_dir().join("meta.db")
}

/// Computes Bayesian-smoothed win rate per `code_artifact.md` Section 4.2.
///
/// Formula: `(wins + prior_c * base_rate) / (games + prior_c)`.
pub fn bayesian_smoothed_win_rate(wins: f64, games: f64, base_rate: f64, prior_c: f64) -> f64 {
    let denom = games + prior_c;
    if denom <= 0.0 {
        base_rate
    } else {
        (wins + prior_c * base_rate) / denom
    }
}

/// Champion strength and metadata entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeroStats {
    pub hero_id: i32,
    pub name_zh: String,
    pub title_zh: String,
    pub alias: String,
    pub tier: HeroTier,
    pub win_rate: f64,
    pub pick_rate: f64,
    pub tags: Vec<String>,
}

/// Highlighted bench champion card sorted by tier & win rate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchHighlight {
    pub hero_id: i32,
    pub name_zh: String,
    pub tier: HeroTier,
    pub win_rate: f64,
    pub tags: Vec<String>,
}

/// Real-time team composition balance diagnosis.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TeamDiagnosis {
    pub ad_ratio: f64,
    pub ap_ratio: f64,
    pub tank_total: f64,
    pub cc_total: f64,
    pub warnings: Vec<String>,
}

/// Recommended champion combining duo synergy and comp balance adjustment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SynergyRecommendation {
    pub hero_id: i32,
    pub name_zh: String,
    pub tier: HeroTier,
    pub win_rate: f64,
    pub tags: Vec<String>,
    pub composite_score: f64,
    pub synergy_reason: Option<String>,
    pub comp_reason: Option<String>,
}

/// Combined team diagnosis and ranked champion recommendations.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TeamRecommendations {
    pub diagnosis: TeamDiagnosis,
    pub top_picks: Vec<SynergyRecommendation>,
}

/// Single item entry in a recommended build path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildItemInfo {
    pub id: i32,
    pub name_zh: String,
    pub already_owned: bool,
}

/// Recommended build path tailored to playstyle and live item ownership.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynamicBuildPlan {
    pub playstyle_tag: String,
    pub win_rate: f64,
    pub items: Vec<BuildItemInfo>,
}

/// Golden synergy alert badge triggered when `lift_score > 1.15 && sample_size >= 30`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SynergyAlert {
    pub partner_augment_id: i32,
    pub partner_name_zh: String,
    pub lift_score: f64,
    pub sample_size: i32,
    pub badge: String,
}

/// Evaluation of an offered hextech augment card for the active champion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AugmentEvaluation {
    pub augment_id: i32,
    pub name_zh: String,
    pub rarity: String,
    pub desc_zh: String,
    pub tier: HeroTier,
    pub win_rate: f64,
    pub pick_rate: f64,
    pub synergy_alert: Option<SynergyAlert>,
}

/// Pre-match preview card containing top 3 augments and core 3-item builds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeroPreview {
    pub hero_id: i32,
    pub name_zh: String,
    pub tier: HeroTier,
    pub win_rate: f64,
    pub tags: Vec<String>,
    pub top_augments: Vec<AugmentEvaluation>,
    pub core_builds: Vec<DynamicBuildPlan>,
}

/// Perceptual hash dictionary entry used by `hex-vision` for `< 2ms` icon matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AugmentHashEntry {
    pub augment_id: i32,
    pub phash: u64,
}

/// Thread-safe in-memory SQLite engine serving sub-millisecond meta queries.
pub struct MetaEngine {
    conn: Mutex<Connection>,
}

impl MetaEngine {
    /// Opens the default `%LOCALAPPDATA%/Hexrs/meta.db`, seeding it if missing/empty, and loads into RAM.
    pub fn open_default() -> Result<Self> {
        let dir = data_dir();
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create data directory {}", dir.display()))?;
        let path = db_path();

        let needs_seed = if !path.exists() {
            true
        } else {
            let check_conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .with_context(|| format!("failed to inspect existing {}", path.display()))?;
            let count: i64 = check_conn
                .query_row("SELECT COUNT(*) FROM hero_tier_stats", [], |row| row.get(0))
                .unwrap_or(0);
            count == 0
        };

        if needs_seed {
            info!(path = %path.display(), "initializing cold-start meta.db seed dataset");
            let disk_conn = Connection::open(&path)
                .with_context(|| format!("failed to create/open {}", path.display()))?;
            init_schema(&disk_conn)?;
            populate_seed_data(&disk_conn)?;
        }

        Self::open_from_path(&path)
    }

    /// Loads an existing SQLite `meta.db` file read-only into an in-memory SQLite connection.
    pub fn open_from_path(path: &Path) -> Result<Self> {
        let mem_conn = load_path_into_memory(path)?;
        Ok(Self {
            conn: Mutex::new(mem_conn),
        })
    }

    /// Atomically reloads the in-memory database from a newly hot-updated SQLite file on disk.
    pub fn reload_from_path(&mut self, path: &Path) -> Result<()> {
        let next_conn = load_path_into_memory(path)?;
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| anyhow!("MetaEngine connection mutex poisoned"))?;
        *guard = next_conn;
        Ok(())
    }

    /// Resolves a champion ID from a Live API `championName`, alias, or Chinese name.
    pub fn resolve_champion_id_by_alias(&self, alias_or_name: &str) -> Option<i32> {
        let target = normalize_identifier(alias_or_name);
        if target.is_empty() {
            return None;
        }

        let conn = self.conn.lock().ok()?;
        let mut stmt = conn
            .prepare("SELECT hero_id, alias, name_zh FROM champions")
            .ok()?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i32>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .ok()?;

        for row in rows.flatten() {
            if normalize_identifier(&row.1) == target || row.2.trim() == alias_or_name.trim() {
                return Some(row.0);
            }
        }
        None
    }

    /// Fetches tier, win rate, pick rate, and playstyle tags for a single champion.
    pub fn get_hero_stats(&self, hero_id: i32) -> Option<HeroStats> {
        let conn = self.conn.lock().ok()?;
        query_hero_stats(&conn, hero_id)
    }

    /// Returns up to `limit` highest-priority bench champions sorted by tier then win rate.
    pub fn highlight_bench(&self, bench_ids: &[i32], limit: usize) -> Vec<BenchHighlight> {
        if bench_ids.is_empty() || limit == 0 {
            return Vec::new();
        }
        let Ok(conn) = self.conn.lock() else {
            return Vec::new();
        };

        let mut seen = HashSet::new();
        let mut highlights = Vec::new();
        for &hero_id in bench_ids {
            if hero_id <= 0 || !seen.insert(hero_id) {
                continue;
            }
            if let Some(stats) = query_hero_stats(&conn, hero_id) {
                highlights.push(BenchHighlight {
                    hero_id: stats.hero_id,
                    name_zh: stats.name_zh,
                    tier: stats.tier,
                    win_rate: stats.win_rate,
                    tags: stats.tags,
                });
            }
        }

        highlights.sort_by(|a, b| {
            a.tier
                .sort_rank()
                .cmp(&b.tier.sort_rank())
                .then_with(|| b.win_rate.total_cmp(&a.win_rate))
        });
        highlights.truncate(limit);
        highlights
    }

    /// Runs the two-layer team recommender: duo synergy lift + team composition gap diagnosis.
    pub fn recommend_for_team(
        &self,
        teammate_ids: &[i32],
        candidate_ids: &[i32],
        top_n: usize,
    ) -> TeamRecommendations {
        let Ok(conn) = self.conn.lock() else {
            return TeamRecommendations::default();
        };

        let diagnosis = compute_team_diagnosis(&conn, teammate_ids);
        if top_n == 0 {
            return TeamRecommendations {
                diagnosis,
                top_picks: Vec::new(),
            };
        }

        let teammate_set: HashSet<i32> = teammate_ids.iter().copied().filter(|&id| id > 0).collect();
        let pool: Vec<i32> = if candidate_ids.is_empty() {
            list_all_hero_ids(&conn)
        } else {
            candidate_ids.to_vec()
        };

        let need_ap = diagnosis.warnings.iter().any(|w| w == "缺魔法伤害");
        let need_ad = diagnosis.warnings.iter().any(|w| w == "缺物理伤害");
        let need_tank = diagnosis.warnings.iter().any(|w| w == "缺前排开团");
        let need_cc = diagnosis.warnings.iter().any(|w| w == "缺硬控");

        let mut seen_candidates = HashSet::new();
        let mut recs = Vec::new();

        for cand_id in pool {
            if cand_id <= 0 || teammate_set.contains(&cand_id) || !seen_candidates.insert(cand_id) {
                continue;
            }
            let Some(stats) = query_hero_stats(&conn, cand_id) else {
                continue;
            };

            let mut score = stats.win_rate + f64::from(5 - stats.tier.sort_rank()) * 1.4;
            let mut best_synergy: Option<(f64, String)> = None;

            for &mate_id in &teammate_set {
                if let Some((mate_name, duo_wr, lift)) = query_duo_synergy(&conn, cand_id, mate_id) {
                    let bonus = (duo_wr - 50.0).max(0.0) * 0.45 + (lift - 1.0).max(0.0) * 18.0;
                    let replace = best_synergy
                        .as_ref()
                        .map(|(best_bonus, _)| bonus > *best_bonus)
                        .unwrap_or(true);
                    if replace {
                        best_synergy = Some((
                            bonus,
                            format!("与{mate_name}搭配胜率 {duo_wr:.1}%"),
                        ));
                    }
                }
            }

            let synergy_reason = best_synergy.map(|(bonus, reason)| {
                score += bonus;
                reason
            });

            let mut comp_reasons = Vec::new();
            if let Some(attrs) = query_hero_attributes(&conn, cand_id) {
                if need_ap && attrs.ap_weight >= 0.60 {
                    score += attrs.ap_weight * 4.5;
                    comp_reasons.push("补魔法伤害");
                }
                if need_ad && attrs.ad_weight >= 0.60 {
                    score += attrs.ad_weight * 4.5;
                    comp_reasons.push("补物理伤害");
                }
                if need_tank && attrs.tank_score >= 0.60 {
                    score += attrs.tank_score * 5.0;
                    comp_reasons.push("补前排开团");
                }
                if need_cc && attrs.cc_score >= 0.60 {
                    score += attrs.cc_score * 4.0;
                    comp_reasons.push("补硬控");
                }
            }

            let comp_reason = if comp_reasons.is_empty() {
                None
            } else {
                Some(comp_reasons.join(" · "))
            };

            recs.push(SynergyRecommendation {
                hero_id: stats.hero_id,
                name_zh: stats.name_zh,
                tier: stats.tier,
                win_rate: stats.win_rate,
                tags: stats.tags,
                composite_score: score,
                synergy_reason,
                comp_reason,
            });
        }

        recs.sort_by(|a, b| {
            b.composite_score
                .total_cmp(&a.composite_score)
                .then_with(|| b.win_rate.total_cmp(&a.win_rate))
        });
        recs.truncate(top_n);

        TeamRecommendations {
            diagnosis,
            top_picks: recs,
        }
    }

    /// Returns a champion's pre-match preview (top 3 augments + core 3-item builds).
    pub fn get_hero_preview(&self, hero_id: i32) -> Option<HeroPreview> {
        let conn = self.conn.lock().ok()?;
        let stats = query_hero_stats(&conn, hero_id)?;

        let mut aug_stmt = conn
            .prepare(
                "SELECT m.augment_id, m.name_zh, m.rarity, m.desc_zh, s.tier, s.win_rate, s.pick_rate
                 FROM hero_augment_stats s
                 JOIN augment_meta m ON m.augment_id = s.augment_id
                 WHERE s.hero_id = ?1
                 ORDER BY s.win_rate DESC
                 LIMIT 3",
            )
            .ok()?;

        let top_augments = aug_stmt
            .query_map(params![hero_id], |row| {
                let tier_str: String = row.get(4)?;
                let win_rate: f64 = row.get(5)?;
                Ok(AugmentEvaluation {
                    augment_id: row.get(0)?,
                    name_zh: row.get(1)?,
                    rarity: row.get(2)?,
                    desc_zh: row.get(3)?,
                    tier: HeroTier::from_label(&tier_str).unwrap_or_else(|| tier_from_win_rate(win_rate)),
                    win_rate,
                    pick_rate: row.get(6)?,
                    synergy_alert: None,
                })
            })
            .ok()?
            .flatten()
            .collect();

        let core_builds = query_hero_builds(&conn, hero_id, &[]);

        Some(HeroPreview {
            hero_id: stats.hero_id,
            name_zh: stats.name_zh,
            tier: stats.tier,
            win_rate: stats.win_rate,
            tags: stats.tags,
            top_augments,
            core_builds,
        })
    }

    /// Lists all augment perceptual hashes (`u64`) for `hex-vision` matching.
    pub fn list_augment_hashes(&self) -> Vec<AugmentHashEntry> {
        let Ok(conn) = self.conn.lock() else {
            return Vec::new();
        };
        let Ok(mut stmt) = conn.prepare("SELECT augment_id, phash FROM augment_meta") else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map([], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?))
        }) else {
            return Vec::new();
        };

        rows.flatten()
            .filter_map(|(augment_id, hex)| {
                let cleaned = hex.trim().trim_start_matches("0x");
                u64::from_str_radix(cleaned, 16)
                    .ok()
                    .map(|phash| AugmentHashEntry { augment_id, phash })
            })
            .collect()
    }

    /// Evaluates 3 offered augments for `hero_id`, flagging golden `⚡ 完美协同` against `owned_aug_ids`.
    pub fn evaluate_augment_offer(
        &self,
        hero_id: i32,
        offered_aug_ids: &[i32],
        owned_aug_ids: &[i32],
    ) -> Vec<AugmentEvaluation> {
        let Ok(conn) = self.conn.lock() else {
            return Vec::new();
        };

        let mut results = Vec::with_capacity(offered_aug_ids.len());
        for &aug_id in offered_aug_ids {
            let Some((name_zh, rarity, desc_zh)) = conn
                .query_row(
                    "SELECT name_zh, rarity, desc_zh FROM augment_meta WHERE augment_id = ?1",
                    params![aug_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
                )
                .optional()
                .ok()
                .flatten()
            else {
                continue;
            };

            let hero_stat = conn
                .query_row(
                    "SELECT tier, win_rate, pick_rate
                     FROM hero_augment_stats
                     WHERE hero_id = ?1 AND augment_id = ?2",
                    params![hero_id, aug_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?, row.get::<_, f64>(2)?)),
                )
                .optional()
                .ok()
                .flatten();

            let (tier, win_rate, pick_rate) = match hero_stat {
                Some((tier_str, wr, pr)) => (
                    HeroTier::from_label(&tier_str).unwrap_or_else(|| tier_from_win_rate(wr)),
                    wr,
                    pr,
                ),
                None => {
                    let avg: Option<(f64, f64)> = conn
                        .query_row(
                            "SELECT AVG(win_rate), AVG(pick_rate) FROM hero_augment_stats WHERE augment_id = ?1",
                            params![aug_id],
                            |row| Ok((row.get::<_, Option<f64>>(0)?, row.get::<_, Option<f64>>(1)?)),
                        )
                        .optional()
                        .ok()
                        .flatten()
                        .and_then(|(w, p)| Some((w?, p?)));
                    let (wr, pr) = avg.unwrap_or((50.0, 5.0));
                    (tier_from_win_rate(wr), (wr * 10.0).round() / 10.0, pr)
                }
            };

            let synergy_alert = find_best_augment_synergy(&conn, hero_id, aug_id, owned_aug_ids);

            results.push(AugmentEvaluation {
                augment_id: aug_id,
                name_zh,
                rarity,
                desc_zh,
                tier,
                win_rate,
                pick_rate,
                synergy_alert,
            });
        }

        results
    }

    /// Selects the highest-fit core item build plan given owned augments and already-purchased items.
    pub fn get_dynamic_build(
        &self,
        hero_id: i32,
        owned_aug_ids: &[i32],
        owned_item_ids: &[i32],
    ) -> Option<DynamicBuildPlan> {
        let conn = self.conn.lock().ok()?;
        let builds = query_hero_builds(&conn, hero_id, owned_item_ids);
        if builds.is_empty() {
            return None;
        }

        if owned_aug_ids.is_empty() {
            return builds.into_iter().next();
        }

        builds.into_iter().max_by(|a, b| {
            let score_a = score_build_for_augments(a, owned_aug_ids);
            let score_b = score_build_for_augments(b, owned_aug_ids);
            score_a.total_cmp(&score_b)
        })
    }
}

fn load_path_into_memory(path: &Path) -> Result<Connection> {
    let disk_conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("failed to open SQLite file read-only at {}", path.display()))?;
    let mut mem_conn =
        Connection::open_in_memory().context("failed to allocate in-memory SQLite database")?;
    {
        let backup = Backup::new(&disk_conn, &mut mem_conn)
            .context("failed to initialize SQLite backup into memory")?;
        backup
            .run_to_completion(256, Duration::ZERO, None)
            .context("failed to load meta.db into memory")?;
    }
    Ok(mem_conn)
}

#[derive(Debug, Clone, Copy)]
struct HeroAttrRow {
    ad_weight: f64,
    ap_weight: f64,
    tank_score: f64,
    cc_score: f64,
}

fn query_hero_stats(conn: &Connection, hero_id: i32) -> Option<HeroStats> {
    conn.query_row(
        "SELECT c.hero_id, c.name_zh, c.title_zh, c.alias, t.tier, t.win_rate, t.pick_rate, t.tags
         FROM champions c
         JOIN hero_tier_stats t ON t.hero_id = c.hero_id
         WHERE c.hero_id = ?1",
        params![hero_id],
        |row| {
            let tier_str: String = row.get(4)?;
            let win_rate: f64 = row.get(5)?;
            let tags_raw: String = row.get(7)?;
            Ok(HeroStats {
                hero_id: row.get(0)?,
                name_zh: row.get(1)?,
                title_zh: row.get(2)?,
                alias: row.get(3)?,
                tier: HeroTier::from_label(&tier_str).unwrap_or_else(|| tier_from_win_rate(win_rate)),
                win_rate,
                pick_rate: row.get(6)?,
                tags: parse_tags(&tags_raw),
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

fn query_hero_attributes(conn: &Connection, hero_id: i32) -> Option<HeroAttrRow> {
    conn.query_row(
        "SELECT ad_weight, ap_weight, tank_score, cc_score FROM hero_attributes WHERE hero_id = ?1",
        params![hero_id],
        |row| {
            Ok(HeroAttrRow {
                ad_weight: row.get(0)?,
                ap_weight: row.get(1)?,
                tank_score: row.get(2)?,
                cc_score: row.get(3)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

fn query_duo_synergy(conn: &Connection, hero_x: i32, hero_y: i32) -> Option<(String, f64, f64)> {
    let (a, b) = if hero_x <= hero_y {
        (hero_x, hero_y)
    } else {
        (hero_y, hero_x)
    };

    conn.query_row(
        "SELECT c.name_zh, d.duo_win_rate, d.lift_score
         FROM hero_duo_synergy d
         JOIN champions c ON c.hero_id = ?3
         WHERE d.hero_a = ?1 AND d.hero_b = ?2",
        params![a, b, hero_y],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?, row.get::<_, f64>(2)?)),
    )
    .optional()
    .ok()
    .flatten()
}

fn list_all_hero_ids(conn: &Connection) -> Vec<i32> {
    let Ok(mut stmt) = conn.prepare("SELECT hero_id FROM hero_tier_stats") else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |row| row.get::<_, i32>(0)) else {
        return Vec::new();
    };
    rows.flatten().collect()
}

fn compute_team_diagnosis(conn: &Connection, teammate_ids: &[i32]) -> TeamDiagnosis {
    let mut ad_sum = 0.0;
    let mut ap_sum = 0.0;
    let mut tank_total = 0.0;
    let mut cc_total = 0.0;
    let mut count = 0usize;

    for &hero_id in teammate_ids {
        if hero_id <= 0 {
            continue;
        }
        if let Some(attrs) = query_hero_attributes(conn, hero_id) {
            ad_sum += attrs.ad_weight;
            ap_sum += attrs.ap_weight;
            tank_total += attrs.tank_score;
            cc_total += attrs.cc_score;
            count += 1;
        }
    }

    if count == 0 {
        return TeamDiagnosis {
            ad_ratio: 0.5,
            ap_ratio: 0.5,
            tank_total: 0.0,
            cc_total: 0.0,
            warnings: Vec::new(),
        };
    }

    let total_dmg = ad_sum + ap_sum;
    let (ad_ratio, ap_ratio) = if total_dmg > 0.0 {
        (ad_sum / total_dmg, ap_sum / total_dmg)
    } else {
        (0.5, 0.5)
    };

    let mut warnings = Vec::new();
    if ap_ratio < 0.25 {
        warnings.push("缺魔法伤害".to_owned());
    }
    if ad_ratio < 0.25 {
        warnings.push("缺物理伤害".to_owned());
    }
    if tank_total < 0.80 {
        warnings.push("缺前排开团".to_owned());
    }
    if cc_total < 0.80 {
        warnings.push("缺硬控".to_owned());
    }

    TeamDiagnosis {
        ad_ratio,
        ap_ratio,
        tank_total,
        cc_total,
        warnings,
    }
}

fn find_best_augment_synergy(
    conn: &Connection,
    hero_id: i32,
    offered_aug_id: i32,
    owned_aug_ids: &[i32],
) -> Option<SynergyAlert> {
    let mut best: Option<SynergyAlert> = None;

    for &owned_id in owned_aug_ids {
        if owned_id <= 0 || owned_id == offered_aug_id {
            continue;
        }
        let (a, b) = if offered_aug_id <= owned_id {
            (offered_aug_id, owned_id)
        } else {
            (owned_id, offered_aug_id)
        };

        let found = conn
            .query_row(
                "SELECT s.lift_score, s.sample_size, m.name_zh
                 FROM augment_synergy s
                 JOIN augment_meta m ON m.augment_id = ?4
                 WHERE s.aug_a = ?1
                   AND s.aug_b = ?2
                   AND (s.hero_id = ?3 OR s.hero_id = 0)
                   AND s.lift_score > ?5
                   AND s.sample_size >= ?6
                 ORDER BY CASE WHEN s.hero_id = ?3 THEN 0 ELSE 1 END, s.lift_score DESC
                 LIMIT 1",
                params![
                    a,
                    b,
                    hero_id,
                    owned_id,
                    SYNERGY_LIFT_THRESHOLD,
                    SYNERGY_MIN_SAMPLES
                ],
                |row| {
                    Ok(SynergyAlert {
                        partner_augment_id: owned_id,
                        partner_name_zh: row.get(2)?,
                        lift_score: row.get(0)?,
                        sample_size: row.get(1)?,
                        badge: "⚡ 完美协同".to_owned(),
                    })
                },
            )
            .optional()
            .ok()
            .flatten();

        if let Some(alert) = found {
            let replace = best
                .as_ref()
                .map(|curr| alert.lift_score > curr.lift_score)
                .unwrap_or(true);
            if replace {
                best = Some(alert);
            }
        }
    }

    best
}

fn query_hero_builds(
    conn: &Connection,
    hero_id: i32,
    owned_item_ids: &[i32],
) -> Vec<DynamicBuildPlan> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT playstyle_tag, core_items_json, win_rate
         FROM hero_builds
         WHERE hero_id = ?1
         ORDER BY win_rate DESC",
    ) else {
        return Vec::new();
    };

    let Ok(rows) = stmt.query_map(params![hero_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, f64>(2)?,
        ))
    }) else {
        return Vec::new();
    };

    rows.flatten()
        .map(|(playstyle_tag, items_json, win_rate)| DynamicBuildPlan {
            playstyle_tag,
            win_rate,
            items: parse_core_items(&items_json, owned_item_ids),
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawCoreItemEntry {
    Id(i32),
    Obj {
        id: i32,
        #[serde(default)]
        name_zh: String,
    },
}

fn parse_core_items(raw_json: &str, owned_item_ids: &[i32]) -> Vec<BuildItemInfo> {
    let Ok(entries) = serde_json::from_str::<Vec<RawCoreItemEntry>>(raw_json) else {
        return Vec::new();
    };

    entries
        .into_iter()
        .map(|entry| match entry {
            RawCoreItemEntry::Id(id) => BuildItemInfo {
                id,
                name_zh: lookup_item_name_zh(id).to_owned(),
                already_owned: owned_item_ids.contains(&id),
            },
            RawCoreItemEntry::Obj { id, name_zh } => BuildItemInfo {
                id,
                name_zh: if name_zh.is_empty() {
                    lookup_item_name_zh(id).to_owned()
                } else {
                    name_zh
                },
                already_owned: owned_item_ids.contains(&id),
            },
        })
        .collect()
}

fn score_build_for_augments(build: &DynamicBuildPlan, owned_aug_ids: &[i32]) -> f64 {
    let mut score = build.win_rate;
    let tag = build.playstyle_tag.as_str();

    let has_onhit_aug = owned_aug_ids
        .iter()
        .any(|id| matches!(id, 1003 | 1015 | 1020 | 1022));
    let has_ap_aug = owned_aug_ids.iter().any(|id| matches!(id, 1001 | 1010));
    let has_tank_aug = owned_aug_ids
        .iter()
        .any(|id| matches!(id, 1014 | 1017 | 1009 | 1021));

    if has_onhit_aug && (tag.contains("特效") || tag.contains("攻速") || tag.contains("暴击")) {
        score += 3.0;
    }
    if has_ap_aug && (tag.contains("AP") || tag.contains("法") || tag.contains("灼烧")) {
        score += 3.0;
    }
    if has_tank_aug && (tag.contains("坦") || tag.contains("重装") || tag.contains("半肉")) {
        score += 3.0;
    }

    score
}

fn parse_tags(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Ok(list) = serde_json::from_str::<Vec<String>>(trimmed) {
        return list;
    }
    trimmed
        .split([',', '，', '|', '/'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn normalize_identifier(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn tier_from_win_rate(win_rate: f64) -> HeroTier {
    if win_rate >= 57.0 {
        HeroTier::SPlus
    } else if win_rate >= 54.5 {
        HeroTier::S
    } else if win_rate >= 52.5 {
        HeroTier::A
    } else if win_rate >= 50.0 {
        HeroTier::B
    } else if win_rate >= 47.5 {
        HeroTier::C
    } else {
        HeroTier::D
    }
}
