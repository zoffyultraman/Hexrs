//! SQLite schema definitions and index initialization for `meta.db`.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

/// Current SQLite metadata schema version stored in `meta_info`.
pub const SCHEMA_VERSION: &str = "1";

/// Creates all metadata tables and performance indexes if they do not yet exist.
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS meta_info (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS champions (
            hero_id  INTEGER PRIMARY KEY,
            name_zh  TEXT NOT NULL,
            title_zh TEXT NOT NULL,
            alias    TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS augment_meta (
            augment_id INTEGER PRIMARY KEY,
            name_zh    TEXT NOT NULL,
            rarity     TEXT NOT NULL,
            phash      TEXT NOT NULL,
            desc_zh    TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS hero_tier_stats (
            hero_id   INTEGER PRIMARY KEY,
            tier      TEXT NOT NULL,
            win_rate  REAL NOT NULL,
            pick_rate REAL NOT NULL,
            tags      TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS hero_duo_synergy (
            hero_a       INTEGER NOT NULL,
            hero_b       INTEGER NOT NULL,
            duo_win_rate REAL NOT NULL,
            lift_score   REAL NOT NULL,
            sample_size  INTEGER NOT NULL,
            PRIMARY KEY (hero_a, hero_b)
        );

        CREATE TABLE IF NOT EXISTS hero_attributes (
            hero_id    INTEGER PRIMARY KEY,
            ad_weight  REAL NOT NULL,
            ap_weight  REAL NOT NULL,
            tank_score REAL NOT NULL,
            cc_score   REAL NOT NULL
        );

        CREATE TABLE IF NOT EXISTS hero_augment_stats (
            hero_id    INTEGER NOT NULL,
            augment_id INTEGER NOT NULL,
            tier       TEXT NOT NULL,
            win_rate   REAL NOT NULL,
            pick_rate  REAL NOT NULL,
            PRIMARY KEY (hero_id, augment_id)
        );

        CREATE TABLE IF NOT EXISTS augment_synergy (
            hero_id     INTEGER NOT NULL,
            aug_a       INTEGER NOT NULL,
            aug_b       INTEGER NOT NULL,
            lift_score  REAL NOT NULL,
            sample_size INTEGER NOT NULL,
            PRIMARY KEY (hero_id, aug_a, aug_b)
        );

        CREATE TABLE IF NOT EXISTS hero_builds (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            hero_id         INTEGER NOT NULL,
            playstyle_tag   TEXT NOT NULL,
            core_items_json TEXT NOT NULL,
            win_rate        REAL NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_champions_alias
            ON champions (alias);
        CREATE INDEX IF NOT EXISTS idx_hero_duo_a
            ON hero_duo_synergy (hero_a, duo_win_rate DESC);
        CREATE INDEX IF NOT EXISTS idx_hero_duo_b
            ON hero_duo_synergy (hero_b, duo_win_rate DESC);
        CREATE INDEX IF NOT EXISTS idx_hero_aug_stats
            ON hero_augment_stats (hero_id, win_rate DESC);
        CREATE INDEX IF NOT EXISTS idx_aug_synergy_a
            ON augment_synergy (hero_id, aug_a);
        CREATE INDEX IF NOT EXISTS idx_aug_synergy_b
            ON augment_synergy (hero_id, aug_b);
        CREATE INDEX IF NOT EXISTS idx_hero_builds_hero
            ON hero_builds (hero_id, win_rate DESC);
        "#,
    )
    .context("failed to initialize meta.db schema")?;

    conn.execute(
        "INSERT OR REPLACE INTO meta_info (key, value) VALUES ('schema_version', ?1)",
        params![SCHEMA_VERSION],
    )
    .context("failed to record schema_version in meta_info")?;

    Ok(())
}
