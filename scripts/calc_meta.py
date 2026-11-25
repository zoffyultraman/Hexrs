#!/usr/bin/env python3
"""Compute Bayesian smoothed win rates + Synergy Lift, build SQLite meta.db, compress via Zstd, and emit manifest.json."""

from __future__ import annotations

import argparse
import hashlib
import json
import sqlite3
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import zstandard as zstd

SCHEMA_VERSION = "1"
PRIOR_WEIGHT_C = 50.0
BASE_WIN_RATE = 0.50
MIN_SYNERGY_SAMPLES = 30
MIN_SYNERGY_LIFT = 1.15

DEFAULT_ROI_MANIFEST: dict[str, Any] = {
    "version": "1.0.0",
    "sha256": "",
    "16_9": {
        "card_icons": [
            {"x": 0.285, "y": 0.315, "w": 0.065, "h": 0.115},
            {"x": 0.467, "y": 0.315, "w": 0.065, "h": 0.115},
            {"x": 0.650, "y": 0.315, "w": 0.065, "h": 0.115},
        ],
        "header_indicators": [
            {"x": 0.410, "y": 0.165, "w": 0.180, "h": 0.045},
        ],
    },
    "16_10": {
        "card_icons": [
            {"x": 0.285, "y": 0.325, "w": 0.065, "h": 0.105},
            {"x": 0.467, "y": 0.325, "w": 0.065, "h": 0.105},
            {"x": 0.650, "y": 0.325, "w": 0.065, "h": 0.105},
        ],
        "header_indicators": [
            {"x": 0.410, "y": 0.180, "w": 0.180, "h": 0.042},
        ],
    },
    "21_9": {
        "card_icons": [
            {"x": 0.338, "y": 0.315, "w": 0.049, "h": 0.115},
            {"x": 0.475, "y": 0.315, "w": 0.049, "h": 0.115},
            {"x": 0.612, "y": 0.315, "w": 0.049, "h": 0.115},
        ],
        "header_indicators": [
            {"x": 0.432, "y": 0.165, "w": 0.136, "h": 0.045},
        ],
    },
}

BASELINE_HERO_ARCHETYPES: dict[int, tuple[str, float, float, float, float, str, list[int]]] = {
    157: ("S+", 0.90, 0.10, 0.25, 0.55, "暴击特效流", [3153, 6673, 3031]),
    777: ("S", 0.72, 0.28, 0.30, 0.65, "暴击特效流", [3153, 6673, 3031]),
    238: ("A", 0.95, 0.05, 0.10, 0.15, "穿甲秒杀流", [6692, 3142, 6694]),
    145: ("S+", 0.58, 0.42, 0.10, 0.05, "双修特效流", [3124, 3115, 3089]),
    67: ("S", 0.92, 0.08, 0.15, 0.25, "攻速特效流", [3153, 3124, 3091]),
    360: ("S", 0.95, 0.05, 0.20, 0.15, "暴击收割流", [6676, 3031, 6673]),
    82: ("S", 0.10, 0.90, 0.78, 0.45, "法坦续航流", [3116, 4633, 3157]),
    875: ("S+", 0.92, 0.08, 0.85, 0.72, "心之钢重装", [3084, 3053, 3748]),
    122: ("A", 0.96, 0.04, 0.72, 0.48, "重装战士流", [3078, 3053, 3742]),
    54: ("S", 0.15, 0.85, 0.95, 0.88, "纯甲开团流", [3068, 3075, 3110]),
    32: ("S+", 0.10, 0.90, 0.90, 0.96, "灼烧法坦流", [3068, 6653, 3075]),
    201: ("A", 0.12, 0.88, 0.92, 0.82, "辅助护卫流", [3190, 3109, 3075]),
    89: ("S", 0.15, 0.85, 0.94, 0.98, "硬控承伤流", [3068, 3075, 3190]),
    63: ("S+", 0.05, 0.95, 0.12, 0.42, "双面具灼烧", [6653, 3116, 3089]),
    112: ("S", 0.05, 0.95, 0.15, 0.50, "法穿爆发流", [6655, 4645, 3089]),
    99: ("A", 0.05, 0.95, 0.10, 0.60, "法穿Poke流", [6655, 4645, 3089]),
    101: ("A", 0.05, 0.95, 0.05, 0.45, "法穿炮台流", [6655, 4645, 3089]),
    876: ("S", 0.08, 0.92, 0.45, 0.68, "持续拉扯流", [6653, 4633, 3157]),
    141: ("A", 0.95, 0.05, 0.55, 0.48, "穿甲刺客流", [3142, 6692, 6694]),
    887: ("S", 0.10, 0.90, 0.48, 0.20, "AP切坦流", [3115, 4633, 3089]),
    498: ("A", 0.95, 0.05, 0.12, 0.45, "暴击收割流", [6672, 6675, 3031]),
    497: ("S", 0.10, 0.90, 0.68, 0.92, "先手开团流", [3190, 3157, 3050]),
    64: ("B", 0.90, 0.10, 0.48, 0.58, "穿甲战士流", [6692, 3071, 3053]),
    55: ("A", 0.35, 0.65, 0.15, 0.05, "双修收割流", [3115, 4645, 3157]),
    119: ("A", 0.98, 0.02, 0.10, 0.22, "高额暴击流", [6676, 3031, 3072]),
}

BASELINE_DUO_SYNERGIES: list[tuple[int, int, float, float, int]] = [
    (54, 157, 59.1, 1.24, 420),
    (157, 777, 56.4, 1.18, 380),
    (32, 157, 57.6, 1.20, 265),
    (89, 157, 56.8, 1.19, 210),
    (497, 498, 59.4, 1.28, 510),
    (89, 145, 56.9, 1.19, 330),
    (89, 360, 58.1, 1.23, 350),
    (67, 201, 57.4, 1.20, 310),
    (32, 63, 58.2, 1.22, 340),
    (54, 112, 56.7, 1.18, 260),
    (99, 101, 56.0, 1.16, 295),
    (82, 876, 57.1, 1.19, 245),
]


def bayesian_smoothed_win_rate(
    wins: float, games: float, base_rate: float = BASE_WIN_RATE, prior_c: float = PRIOR_WEIGHT_C
) -> float:
    """Computes Bayesian smoothed win rate in [0.0, 1.0]."""
    denom = games + prior_c
    if denom <= 0.0:
        return base_rate
    return (wins + prior_c * base_rate) / denom


def tier_from_win_rate_pct(win_rate_pct: float) -> str:
    if win_rate_pct >= 57.0:
        return "S+"
    if win_rate_pct >= 54.5:
        return "S"
    if win_rate_pct >= 52.5:
        return "A"
    if win_rate_pct >= 50.0:
        return "B"
    if win_rate_pct >= 47.5:
        return "C"
    return "D"


def init_sqlite_schema(conn: sqlite3.Connection, data_version: str) -> None:
    conn.executescript(
        """
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

        CREATE INDEX IF NOT EXISTS idx_champions_alias ON champions (alias);
        CREATE INDEX IF NOT EXISTS idx_hero_duo_a ON hero_duo_synergy (hero_a, duo_win_rate DESC);
        CREATE INDEX IF NOT EXISTS idx_hero_duo_b ON hero_duo_synergy (hero_b, duo_win_rate DESC);
        CREATE INDEX IF NOT EXISTS idx_hero_aug_stats ON hero_augment_stats (hero_id, win_rate DESC);
        CREATE INDEX IF NOT EXISTS idx_aug_synergy_a ON augment_synergy (hero_id, aug_a);
        CREATE INDEX IF NOT EXISTS idx_aug_synergy_b ON augment_synergy (hero_id, aug_b);
        CREATE INDEX IF NOT EXISTS idx_hero_builds_hero ON hero_builds (hero_id, win_rate DESC);
        """
    )
    conn.execute(
        "INSERT OR REPLACE INTO meta_info (key, value) VALUES ('schema_version', ?)",
        (SCHEMA_VERSION,),
    )
    conn.execute(
        "INSERT OR REPLACE INTO meta_info (key, value) VALUES ('data_version', ?)",
        (data_version,),
    )


def build_meta_database(
    cdragon_meta: dict[str, Any],
    samples: list[dict[str, Any]],
    db_out: Path,
    data_version: str,
) -> None:
    if db_out.exists():
        db_out.unlink()

    item_names = {int(it["id"]): str(it["name_zh"]) for it in cdragon_meta.get("items", [])}
    champions = cdragon_meta.get("champions", [])
    augments = cdragon_meta.get("augments", [])

    hero_counts: dict[int, list[int]] = defaultdict(lambda: [0, 0])  # [wins, games]
    hero_aug_counts: dict[tuple[int, int], list[int]] = defaultdict(lambda: [0, 0])
    aug_pair_counts: dict[tuple[int, int, int], list[int]] = defaultdict(lambda: [0, 0])
    hero_item_builds: dict[tuple[int, tuple[int, int, int]], list[int]] = defaultdict(
        lambda: [0, 0]
    )

    total_participant_games = 0
    for match in samples:
        for p in match.get("participants", []):
            hero_id = int(p.get("champion_id", 0))
            if hero_id <= 0:
                continue
            win = 1 if p.get("win") else 0
            total_participant_games += 1
            hero_counts[hero_id][0] += win
            hero_counts[hero_id][1] += 1

            augs = sorted({int(a) for a in p.get("augments", []) if int(a) > 0})
            for a in augs:
                hero_aug_counts[(hero_id, a)][0] += win
                hero_aug_counts[(hero_id, a)][1] += 1
            for i in range(len(augs)):
                for j in range(i + 1, len(augs)):
                    aug_pair_counts[(hero_id, augs[i], augs[j])][0] += win
                    aug_pair_counts[(hero_id, augs[i], augs[j])][1] += 1

            items = [int(x) for x in p.get("items", []) if int(x) > 0][:3]
            if len(items) == 3:
                key = (hero_id, (items[0], items[1], items[2]))
                hero_item_builds[key][0] += win
                hero_item_builds[key][1] += 1

    conn = sqlite3.connect(db_out)
    try:
        init_sqlite_schema(conn, data_version)

        for champ in champions:
            hero_id = int(champ["hero_id"])
            conn.execute(
                "INSERT OR REPLACE INTO champions (hero_id, name_zh, title_zh, alias) VALUES (?, ?, ?, ?)",
                (hero_id, champ["name_zh"], champ["title_zh"], champ["alias"]),
            )

            wins, games = hero_counts.get(hero_id, [0, 0])
            arch = BASELINE_HERO_ARCHETYPES.get(hero_id)
            if games > 0:
                wr_pct = round(bayesian_smoothed_win_rate(wins, games) * 100.0, 1)
                pick_rate = round((games / max(total_participant_games, 1)) * 100.0, 2)
                tier = tier_from_win_rate_pct(wr_pct)
            elif arch is not None:
                tier = arch[0]
                wr_pct = {"S+": 57.2, "S": 55.2, "A": 53.0, "B": 50.8}.get(tier, 50.5)
                pick_rate = 8.0
            else:
                tier = "B"
                wr_pct = 50.0
                pick_rate = 3.0

            tag_str = json.dumps([arch[5]] if arch else ["均衡流"], ensure_ascii=False)
            conn.execute(
                "INSERT OR REPLACE INTO hero_tier_stats (hero_id, tier, win_rate, pick_rate, tags) VALUES (?, ?, ?, ?, ?)",
                (hero_id, tier, wr_pct, pick_rate, tag_str),
            )

            ad_w, ap_w, tank_s, cc_s = (
                (arch[1], arch[2], arch[3], arch[4]) if arch else (0.50, 0.50, 0.35, 0.35)
            )
            conn.execute(
                "INSERT OR REPLACE INTO hero_attributes (hero_id, ad_weight, ap_weight, tank_score, cc_score) VALUES (?, ?, ?, ?, ?)",
                (hero_id, ad_w, ap_w, tank_s, cc_s),
            )

            if arch is not None:
                items_payload = [
                    {"id": item_id, "name_zh": item_names.get(item_id, "核心装备")}
                    for item_id in arch[6]
                ]
                conn.execute(
                    "INSERT INTO hero_builds (hero_id, playstyle_tag, core_items_json, win_rate) VALUES (?, ?, ?, ?)",
                    (hero_id, arch[5], json.dumps(items_payload, ensure_ascii=False), wr_pct),
                )

        for (hero_id, item_tuple), (w, g) in hero_item_builds.items():
            if g < 5:
                continue
            wr_pct = round(bayesian_smoothed_win_rate(w, g) * 100.0, 1)
            items_payload = [
                {"id": item_id, "name_zh": item_names.get(item_id, "核心装备")}
                for item_id in item_tuple
            ]
            conn.execute(
                "INSERT INTO hero_builds (hero_id, playstyle_tag, core_items_json, win_rate) VALUES (?, ?, ?, ?)",
                (hero_id, "热门核心流", json.dumps(items_payload, ensure_ascii=False), wr_pct),
            )

        for aug in augments:
            aug_id = int(aug["augment_id"])
            conn.execute(
                "INSERT OR REPLACE INTO augment_meta (augment_id, name_zh, rarity, phash, desc_zh) VALUES (?, ?, ?, ?, ?)",
                (aug_id, aug["name_zh"], aug["rarity"], aug["phash"], aug["desc_zh"]),
            )

        for (hero_id, aug_id), (w, g) in hero_aug_counts.items():
            hw, hg = hero_counts.get(hero_id, [0, 0])
            p_h = bayesian_smoothed_win_rate(hw, hg) if hg > 0 else BASE_WIN_RATE
            wr = bayesian_smoothed_win_rate(w, g, base_rate=p_h)
            wr_pct = round(wr * 100.0, 1)
            pick_rate = round((g / max(hg, 1)) * 100.0, 1)
            conn.execute(
                "INSERT OR REPLACE INTO hero_augment_stats (hero_id, augment_id, tier, win_rate, pick_rate) VALUES (?, ?, ?, ?, ?)",
                (hero_id, aug_id, tier_from_win_rate_pct(wr_pct), wr_pct, pick_rate),
            )

        for (hero_id, aug_a, aug_b), (w_ab, g_ab) in aug_pair_counts.items():
            if g_ab < MIN_SYNERGY_SAMPLES:
                continue
            hw, hg = hero_counts.get(hero_id, [0, 0])
            p_h = bayesian_smoothed_win_rate(hw, hg)
            w_a, g_a = hero_aug_counts[(hero_id, aug_a)]
            w_b, g_b = hero_aug_counts[(hero_id, aug_b)]
            p_ha = bayesian_smoothed_win_rate(w_a, g_a, base_rate=p_h)
            p_hb = bayesian_smoothed_win_rate(w_b, g_b, base_rate=p_h)
            p_hab = bayesian_smoothed_win_rate(w_ab, g_ab, base_rate=p_h)
            denom = (p_ha * p_hb) / max(p_h, 1e-6)
            lift = p_hab / max(denom, 1e-6)
            if lift > MIN_SYNERGY_LIFT:
                conn.execute(
                    "INSERT OR REPLACE INTO augment_synergy (hero_id, aug_a, aug_b, lift_score, sample_size) VALUES (?, ?, ?, ?, ?)",
                    (hero_id, aug_a, aug_b, round(lift, 3), g_ab),
                )

        for a, b, wr, lift, n in BASELINE_DUO_SYNERGIES:
            lo, hi = (a, b) if a <= b else (b, a)
            conn.execute(
                "INSERT OR REPLACE INTO hero_duo_synergy (hero_a, hero_b, duo_win_rate, lift_score, sample_size) VALUES (?, ?, ?, ?, ?)",
                (lo, hi, wr, lift, n),
            )

        conn.commit()
    finally:
        conn.close()


def compress_and_emit_manifest(
    db_path: Path,
    out_dir: Path,
    version: str,
    db_url: str,
) -> tuple[Path, Path]:
    zst_path = out_dir / "meta.db.zst"
    raw_bytes = db_path.read_bytes()
    compressor = zstd.ZstdCompressor(level=12)
    compressed = compressor.compress(raw_bytes)
    zst_path.write_bytes(compressed)

    sha256_hex = hashlib.sha256(compressed).hexdigest()
    roi_manifest = dict(DEFAULT_ROI_MANIFEST)
    roi_manifest["version"] = version
    roi_manifest["sha256"] = sha256_hex

    manifest = {
        "version": version,
        "db_url": db_url,
        "sha256": sha256_hex,
        "roi_manifest": roi_manifest,
    }

    manifest_path = out_dir / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
    return zst_path, manifest_path


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Build meta.db, compress meta.db.zst, and write manifest.json."
    )
    parser.add_argument(
        "--input",
        type=Path,
        default=Path("dist/cdragon_meta.json"),
        help="Input CDragon metadata JSON from fetch_cdragon.py.",
    )
    parser.add_argument(
        "--samples",
        type=Path,
        default=None,
        help="Optional anonymized EOG samples JSON list.",
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=Path("dist"),
        help="Output directory for meta.db, meta.db.zst, and manifest.json.",
    )
    parser.add_argument(
        "--version",
        type=str,
        default=datetime.now(timezone.utc).strftime("%Y.%m.%d.%H%M"),
        help="Release version string for manifest.json.",
    )
    parser.add_argument(
        "--db-url",
        type=str,
        default="https://raw.githubusercontent.com/zoffyultraman/Hexrs/data/meta.db.zst",
        help="Public download URL written into manifest.json.",
    )
    args = parser.parse_args()

    cdragon_meta = json.loads(args.input.read_text(encoding="utf-8"))
    samples: list[dict[str, Any]] = []
    if args.samples is not None and args.samples.exists():
        loaded = json.loads(args.samples.read_text(encoding="utf-8"))
        if isinstance(loaded, list):
            samples = loaded

    args.out_dir.mkdir(parents=True, exist_ok=True)
    db_out = args.out_dir / "meta.db"
    build_meta_database(cdragon_meta, samples, db_out, args.version)
    zst_path, manifest_path = compress_and_emit_manifest(
        db_out, args.out_dir, args.version, args.db_url
    )
    print(
        f"Built {db_out} ({db_out.stat().st_size} bytes) -> "
        f"{zst_path} ({zst_path.stat().st_size} bytes) & {manifest_path}"
    )


if __name__ == "__main__":
    main()
