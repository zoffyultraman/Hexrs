//! In-memory SQLite query engine (`< 1ms` lookups) and dual-layer Arena/ARAM recommender.

use std::collections::HashSet;
use std::fs;
#[cfg(has_bundled_meta_db)]
use std::fs::File;
#[cfg(has_bundled_meta_db)]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
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
#[cfg(has_bundled_meta_db)]
const SQLITE_MAGIC_HEADER: &[u8] = b"SQLite format 3\0";

#[cfg(has_bundled_meta_db)]
const BUNDLED_META_DB_ZST: &[u8] = include_bytes!("../assets/meta.db.zst");

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

fn is_dir_writable(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(format!(".hexrs_write_probe_{}", std::process::id()));
    match fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&probe)
    {
        Ok(file) => {
            drop(file);
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Returns the portable `<exe_dir>/data` directory, falling back to `%LOCALAPPDATA%/Hexrs` if unavailable or read-only.
pub fn data_dir() -> PathBuf {
    DATA_DIR
        .get_or_init(|| {
            if let Some(exe_data_dir) = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join("data")))
            {
                if is_dir_writable(&exe_data_dir) {
                    return exe_data_dir;
                }
            }
            dirs::data_local_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("Hexrs")
        })
        .clone()
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

/// Minimum recorded hero×augment sample games required to show reliable hero stats.
pub const AUGMENT_MIN_HERO_SAMPLES: i32 = 30;

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
    #[serde(default)]
    pub sample_size: i32,
    #[serde(default)]
    pub low_sample: bool,
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

/// Augment display metadata entry returned by `MetaEngine::list_augment_names`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AugmentNameEntry {
    pub augment_id: i32,
    pub name_zh: String,
    pub rarity: String,
}

/// Thread-safe in-memory SQLite engine serving sub-millisecond meta queries.
pub struct MetaEngine {
    conn: Mutex<Connection>,
}

impl MetaEngine {
    /// Opens the default `<exe_dir>/data/meta.db` (or `%LOCALAPPDATA%/Hexrs/meta.db` fallback), installing bundled/seed data if missing or outdated, and loads into RAM.
    pub fn open_default() -> Result<Self> {
        let dir = data_dir();
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create data directory {}", dir.display()))?;
        let path = db_path();

        let needs_init = if !path.exists() {
            true
        } else {
            let check_conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .with_context(|| format!("failed to inspect existing {}", path.display()))?;
            let hero_count: i64 = check_conn
                .query_row("SELECT COUNT(*) FROM hero_tier_stats", [], |row| row.get(0))
                .unwrap_or(0);
            let aug_count: i64 = check_conn
                .query_row("SELECT COUNT(*) FROM augment_meta", [], |row| row.get(0))
                .unwrap_or(0);
            let data_version: String = check_conn
                .query_row(
                    "SELECT value FROM meta_info WHERE key = 'data_version'",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or_default();
            let builds_hero_count: i64 = check_conn
                .query_row(
                    "SELECT COUNT(DISTINCT hero_id) FROM hero_builds",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            hero_count == 0
                || (cfg!(has_bundled_meta_db)
                    && (aug_count < 100
                        || !data_version.starts_with("aramhex7-")
                        || builds_hero_count < 100))
        };

        if needs_init {
            let installed_bundled = try_install_bundled_db(&dir, &path);
            if !installed_bundled {
                info!(path = %path.display(), "initializing cold-start meta.db seed dataset");
                let disk_conn = Connection::open(&path)
                    .with_context(|| format!("failed to create/open {}", path.display()))?;
                init_schema(&disk_conn)?;
                populate_seed_data(&disk_conn)?;
            }
        }

        Self::open_from_path(&path)
    }

    /// Returns `(champion_count, augment_count)` from the active in-memory SQLite database.
    pub fn stats_counts(&self) -> (usize, usize) {
        let Ok(conn) = self.conn.lock() else {
            return (0, 0);
        };
        let champs: i64 = conn
            .query_row("SELECT COUNT(*) FROM champions", [], |row| row.get(0))
            .unwrap_or(0);
        let augs: i64 = conn
            .query_row("SELECT COUNT(*) FROM augment_meta", [], |row| row.get(0))
            .unwrap_or(0);
        (champs.max(0) as usize, augs.max(0) as usize)
    }

    /// Returns the `data_version` stored in `meta_info`, or empty string if missing.
    pub fn data_version(&self) -> String {
        let Ok(conn) = self.conn.lock() else {
            return String::new();
        };
        conn.query_row(
            "SELECT value FROM meta_info WHERE key = 'data_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .unwrap_or_default()
    }

    /// Lists all non-empty augment names and rarities stored in `augment_meta`,
    /// prioritizing augments that have real `hero_id > 0` statistics over duplicate-named Arena IDs.
    pub fn list_augment_names(&self) -> Vec<AugmentNameEntry> {
        let Ok(conn) = self.conn.lock() else {
            return Vec::new();
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT m.augment_id, m.name_zh, m.rarity
             FROM augment_meta m
             LEFT JOIN (
                 SELECT augment_id, COUNT(*) AS hero_cnt
                 FROM hero_augment_stats
                 WHERE hero_id > 0
                 GROUP BY augment_id
             ) h ON h.augment_id = m.augment_id
             WHERE TRIM(m.name_zh) != ''
             ORDER BY COALESCE(h.hero_cnt, 0) DESC, m.augment_id DESC",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map([], |row| {
            Ok(AugmentNameEntry {
                augment_id: row.get(0)?,
                name_zh: row.get(1)?,
                rarity: row.get(2)?,
            })
        }) else {
            return Vec::new();
        };
        rows.flatten().collect()
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

    /// Returns the champion's DataDragon alias (e.g. `"Yasuo"`) for `hero_id`, if present.
    pub fn get_hero_alias(&self, hero_id: i32) -> Option<String> {
        let conn = self.conn.lock().ok()?;
        conn.query_row(
            "SELECT alias FROM champions WHERE hero_id = ?1",
            params![hero_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
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

    /// Returns a champion's pre-match preview (top 3 augments + core 6-item builds).
    pub fn get_hero_preview(&self, hero_id: i32) -> Option<HeroPreview> {
        let conn = self.conn.lock().ok()?;
        let stats = query_hero_stats(&conn, hero_id)?;
        let has_sample_col = has_hero_augment_sample_size_col(&conn);

        let top_augments = if has_sample_col {
            let mut aug_stmt = conn
                .prepare(
                    "SELECT m.augment_id, m.name_zh, m.rarity, m.desc_zh, s.tier, s.win_rate, s.pick_rate, s.sample_size
                     FROM hero_augment_stats s
                     JOIN augment_meta m ON m.augment_id = s.augment_id
                     WHERE s.hero_id = ?1
                     ORDER BY CASE WHEN s.sample_size >= ?2 THEN 0 ELSE 1 END, s.win_rate DESC
                     LIMIT 3",
                )
                .ok()?;

            let mapped = aug_stmt
                .query_map(params![hero_id, AUGMENT_MIN_HERO_SAMPLES], |row| {
                    let tier_str: String = row.get(4)?;
                    let win_rate: f64 = row.get(5)?;
                    let sample_size: i32 = row.get(7)?;
                    Ok(AugmentEvaluation {
                        augment_id: row.get(0)?,
                        name_zh: row.get(1)?,
                        rarity: row.get(2)?,
                        desc_zh: row.get(3)?,
                        tier: HeroTier::from_label(&tier_str)
                            .unwrap_or_else(|| tier_from_win_rate(win_rate)),
                        win_rate,
                        pick_rate: row.get(6)?,
                        sample_size,
                        low_sample: sample_size < AUGMENT_MIN_HERO_SAMPLES,
                        synergy_alert: None,
                    })
                })
                .ok()?
                .flatten()
                .collect::<Vec<_>>();
            mapped
        } else {
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

            let mapped = aug_stmt
                .query_map(params![hero_id], |row| {
                    let tier_str: String = row.get(4)?;
                    let win_rate: f64 = row.get(5)?;
                    Ok(AugmentEvaluation {
                        augment_id: row.get(0)?,
                        name_zh: row.get(1)?,
                        rarity: row.get(2)?,
                        desc_zh: row.get(3)?,
                        tier: HeroTier::from_label(&tier_str)
                            .unwrap_or_else(|| tier_from_win_rate(win_rate)),
                        win_rate,
                        pick_rate: row.get(6)?,
                        sample_size: 0,
                        low_sample: false,
                        synergy_alert: None,
                    })
                })
                .ok()?
                .flatten()
                .collect::<Vec<_>>();
            mapped
        };

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

    /// Evaluates offered augments for `hero_id`, flagging golden `⚡ 完美协同` against `owned_aug_ids`
    /// and marking hero×augment entries missing or below `AUGMENT_MIN_HERO_SAMPLES` as `low_sample`.
    pub fn evaluate_augment_offer(
        &self,
        hero_id: i32,
        offered_aug_ids: &[i32],
        owned_aug_ids: &[i32],
    ) -> Vec<AugmentEvaluation> {
        let Ok(conn) = self.conn.lock() else {
            return Vec::new();
        };
        let has_sample_col = has_hero_augment_sample_size_col(&conn);

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

            let (tier, win_rate, pick_rate, sample_size, low_sample, synergy_alert) = if hero_id > 0 {
                let primary = query_hero_augment_stat(&conn, hero_id, aug_id, has_sample_col);
                let best = match primary {
                    Some((_, _, _, n)) if n >= AUGMENT_MIN_HERO_SAMPLES => {
                        primary.map(|(t, wr, pr, n)| (aug_id, t, wr, pr, n))
                    }
                    _ => {
                        let primary_n = primary.as_ref().map(|(_, _, _, n)| *n).unwrap_or(-1);
                        let sibling = query_sibling_hero_augment_stat(
                            &conn,
                            hero_id,
                            &name_zh,
                            aug_id,
                            has_sample_col,
                        );
                        match (primary, sibling) {
                            (Some(p), Some(s)) => {
                                if s.4 > primary_n {
                                    Some(s)
                                } else {
                                    Some((aug_id, p.0, p.1, p.2, p.3))
                                }
                            }
                            (Some(p), None) => Some((aug_id, p.0, p.1, p.2, p.3)),
                            (None, Some(s)) => Some(s),
                            (None, None) => None,
                        }
                    }
                };

                match best {
                    Some((resolved_aug_id, tier_str, wr, pr, n)) if n >= AUGMENT_MIN_HERO_SAMPLES => (
                        HeroTier::from_label(&tier_str).unwrap_or_else(|| tier_from_win_rate(wr)),
                        wr,
                        pr,
                        n,
                        false,
                        find_best_augment_synergy(&conn, hero_id, resolved_aug_id, owned_aug_ids),
                    ),
                    Some((_, _, wr, pr, n)) => (HeroTier::D, wr, pr, n.max(0), true, None),
                    None => (HeroTier::D, 0.0, 0.0, 0, true, None),
                }
            } else {
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
                (
                    tier_from_win_rate(wr),
                    (wr * 10.0).round() / 10.0,
                    pr,
                    0,
                    false,
                    find_best_augment_synergy(&conn, hero_id, aug_id, owned_aug_ids),
                )
            };

            results.push(AugmentEvaluation {
                augment_id: aug_id,
                name_zh,
                rarity,
                desc_zh,
                tier,
                win_rate,
                pick_rate,
                sample_size,
                low_sample,
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

    /// Returns the champion's dynamic build plan, or a generic 6-item fallback when unknown.
    pub fn get_dynamic_build_or_fallback(
        &self,
        hero_id: Option<i32>,
        owned_aug_ids: &[i32],
        owned_item_ids: &[i32],
    ) -> DynamicBuildPlan {
        if let Some(id) = hero_id {
            if let Some(plan) = self.get_dynamic_build(id, owned_aug_ids, owned_item_ids) {
                return plan;
            }
        }

        DynamicBuildPlan {
            playstyle_tag: "通用推荐".to_owned(),
            win_rate: 50.0,
            items: vec![
                BuildItemInfo {
                    id: 3078,
                    name_zh: "三相之力".to_owned(),
                    already_owned: owned_item_ids.contains(&3078),
                },
                BuildItemInfo {
                    id: 3053,
                    name_zh: "斯特拉克的挑战护手".to_owned(),
                    already_owned: owned_item_ids.contains(&3053),
                },
                BuildItemInfo {
                    id: 6333,
                    name_zh: "死亡之舞".to_owned(),
                    already_owned: owned_item_ids.contains(&6333),
                },
                BuildItemInfo {
                    id: 3111,
                    name_zh: "水银之靴".to_owned(),
                    already_owned: owned_item_ids.contains(&3111),
                },
                BuildItemInfo {
                    id: 3071,
                    name_zh: "黑色切割者".to_owned(),
                    already_owned: owned_item_ids.contains(&3071),
                },
                BuildItemInfo {
                    id: 3065,
                    name_zh: "振奋盔甲".to_owned(),
                    already_owned: owned_item_ids.contains(&3065),
                },
            ],
        }
    }
}

#[cfg(has_bundled_meta_db)]
fn try_install_bundled_db(dir: &Path, target_db: &Path) -> bool {
    use tracing::warn;

    let decompressed = match zstd::stream::decode_all(BUNDLED_META_DB_ZST) {
        Ok(bytes) => bytes,
        Err(err) => {
            warn!(error = %err, "failed to decompress bundled meta.db.zst; falling back to seed");
            return false;
        }
    };

    if !decompressed.starts_with(SQLITE_MAGIC_HEADER) {
        warn!("bundled meta.db.zst did not decode to a valid SQLite 3 header");
        return false;
    }

    let tmp_path = dir.join("meta.db.tmp");
    {
        let mut tmp_file = match File::create(&tmp_path) {
            Ok(f) => f,
            Err(err) => {
                warn!(error = %err, "failed to create meta.db.tmp for bundled DB");
                return false;
            }
        };
        if let Err(err) = tmp_file.write_all(&decompressed).and_then(|_| tmp_file.sync_all()) {
            warn!(error = %err, "failed to write bundled meta.db.tmp");
            let _ = fs::remove_file(&tmp_path);
            return false;
        }
    }

    if let Err(err) = fs::rename(&tmp_path, target_db) {
        warn!(error = %err, "failed to install bundled meta.db");
        let _ = fs::remove_file(&tmp_path);
        return false;
    }

    if let Ok(conn) = Connection::open_with_flags(target_db, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        if let Ok(Some(ver)) = conn
            .query_row(
                "SELECT value FROM meta_info WHERE key = 'data_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
        {
            let _ = fs::write(dir.join("version.txt"), ver.trim());
        }
    }

    info!(path = %target_db.display(), "installed bundled CDragon meta.db");
    true
}

#[cfg(not(has_bundled_meta_db))]
fn try_install_bundled_db(_dir: &Path, _target_db: &Path) -> bool {
    false
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
        .filter(|plan| !plan.items.is_empty())
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

fn has_hero_augment_sample_size_col(conn: &Connection) -> bool {
    let Ok(mut stmt) = conn.prepare("PRAGMA table_info(hero_augment_stats)") else {
        return false;
    };
    let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(1)) else {
        return false;
    };
    rows.flatten().any(|name| name == "sample_size")
}

fn query_hero_augment_stat(
    conn: &Connection,
    hero_id: i32,
    aug_id: i32,
    has_sample_col: bool,
) -> Option<(String, f64, f64, i32)> {
    if has_sample_col {
        conn.query_row(
            "SELECT tier, win_rate, pick_rate, sample_size
             FROM hero_augment_stats
             WHERE hero_id = ?1 AND augment_id = ?2",
            params![hero_id, aug_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .ok()
        .flatten()
    } else {
        conn.query_row(
            "SELECT tier, win_rate, pick_rate
             FROM hero_augment_stats
             WHERE hero_id = ?1 AND augment_id = ?2",
            params![hero_id, aug_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, AUGMENT_MIN_HERO_SAMPLES)),
        )
        .optional()
        .ok()
        .flatten()
    }
}

fn query_sibling_hero_augment_stat(
    conn: &Connection,
    hero_id: i32,
    name_zh: &str,
    exclude_aug_id: i32,
    has_sample_col: bool,
) -> Option<(i32, String, f64, f64, i32)> {
    if has_sample_col {
        conn.query_row(
            "SELECT m.augment_id, s.tier, s.win_rate, s.pick_rate, s.sample_size
             FROM augment_meta m
             JOIN hero_augment_stats s ON s.augment_id = m.augment_id AND s.hero_id = ?1
             WHERE m.name_zh = ?2 AND m.augment_id != ?3
             ORDER BY s.sample_size DESC, s.win_rate DESC
             LIMIT 1",
            params![hero_id, name_zh, exclude_aug_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .optional()
        .ok()
        .flatten()
    } else {
        conn.query_row(
            "SELECT m.augment_id, s.tier, s.win_rate, s.pick_rate
             FROM augment_meta m
             JOIN hero_augment_stats s ON s.augment_id = m.augment_id AND s.hero_id = ?1
             WHERE m.name_zh = ?2 AND m.augment_id != ?3
             ORDER BY s.win_rate DESC
             LIMIT 1",
            params![hero_id, name_zh, exclude_aug_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    AUGMENT_MIN_HERO_SAMPLES,
                ))
            },
        )
        .optional()
        .ok()
        .flatten()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_engine_with_fixture() -> MetaEngine {
        let conn = Connection::open_in_memory().expect("open in-memory sqlite");
        init_schema(&conn).expect("init schema");

        conn.execute(
            "INSERT INTO augment_meta (augment_id, name_zh, rarity, phash, desc_zh) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![6, "全能", "黄金", "0000000000000000", "Arena duplicate"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO augment_meta (augment_id, name_zh, rarity, phash, desc_zh) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![1006, "全能", "黄金", "0000000000000000", "ARAM Mayhem variant"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO augment_meta (augment_id, name_zh, rarity, phash, desc_zh) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![2001, "稀有海克斯", "棱彩", "0000000000000000", "Low sample test"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO augment_meta (augment_id, name_zh, rarity, phash, desc_zh) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![2002, "无统计海克斯", "白银", "0000000000000000", "Missing stats test"],
        )
        .unwrap();

        // 1006 has real hero stats (hero_id=157, sample_size=120 >= 30); 6 has 0 hero stats.
        conn.execute(
            "INSERT INTO hero_augment_stats (hero_id, augment_id, tier, win_rate, pick_rate, sample_size) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![157, 1006, "S", 55.4, 18.2, 120],
        )
        .unwrap();
        // 2001 has < 30 samples for hero 157.
        conn.execute(
            "INSERT INTO hero_augment_stats (hero_id, augment_id, tier, win_rate, pick_rate, sample_size) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![157, 2001, "S+", 61.0, 2.1, 12],
        )
        .unwrap();
        // Global row for 6 has zero hero stats (hero_id = 0).
        conn.execute(
            "INSERT INTO hero_augment_stats (hero_id, augment_id, tier, win_rate, pick_rate, sample_size) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![0, 6, "B", 50.5, 4.0, 0],
        )
        .unwrap();

        MetaEngine {
            conn: Mutex::new(conn),
        }
    }

    #[test]
    fn list_augment_names_prioritizes_real_hero_stats_over_duplicate_names() {
        let engine = test_engine_with_fixture();
        let names = engine.list_augment_names();
        let ids: Vec<i32> = names
            .into_iter()
            .filter(|e| e.name_zh == "全能")
            .map(|e| e.augment_id)
            .collect();
        assert_eq!(ids, vec![1006, 6]);
    }

    #[test]
    fn evaluate_augment_offer_flags_low_sample_and_missing_and_resolves_sibling() {
        let engine = test_engine_with_fixture();
        let evals = engine.evaluate_augment_offer(157, &[6, 2001, 2002], &[]);
        assert_eq!(evals.len(), 3);

        // 6 resolves to sibling 1006's real hero stats (sample_size = 120 >= 30)
        assert!(!evals[0].low_sample);
        assert_eq!(evals[0].sample_size, 120);
        assert_eq!(evals[0].tier, HeroTier::S);
        assert!((evals[0].win_rate - 55.4).abs() < 1e-6);

        // 2001 has 12 < 30 samples -> flagged low_sample
        assert!(evals[1].low_sample);
        assert_eq!(evals[1].sample_size, 12);
        assert_eq!(evals[1].tier, HeroTier::D);
        assert!(evals[1].synergy_alert.is_none());

        // 2002 missing completely -> sample_size == 0 & low_sample == true
        assert!(evals[2].low_sample);
        assert_eq!(evals[2].sample_size, 0);
        assert_eq!(evals[2].tier, HeroTier::D);
        assert!((evals[2].win_rate - 0.0).abs() < 1e-6);
    }
}
