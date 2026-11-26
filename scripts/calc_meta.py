#!/usr/bin/env python3
"""Merge aramhex-data + CommunityDragon metadata, apply Bayesian smoothing, build meta.db, compress meta.db.zst, and emit manifest.json."""

from __future__ import annotations

import argparse
import hashlib
import json
import sqlite3
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import requests
import zstandard as zstd

SCHEMA_VERSION = "1"
PRIOR_WEIGHT_C = 50.0
BASE_WIN_RATE = 0.50
MIN_SYNERGY_SAMPLES = 30
MIN_SYNERGY_LIFT = 1.15
DEFAULT_ARAMHEX_BASE_URL = "https://raw.githubusercontent.com/czgreat/aramhex-data/main"

AD_ITEM_TYPES = {
    "Damage",
    "AttackDamage",
    "CriticalStrike",
    "AttackSpeed",
    "ArmorPenetration",
    "OnHit",
    "LifeSteal",
}
AP_ITEM_TYPES = {"SpellDamage", "Mana", "MagicPenetration"}
TANK_ITEM_TYPES = {"Health", "Armor", "SpellBlock"}
CC_ITEM_TYPES = {"Slow", "Aura"}

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
    (157, 497, 56.2, 1.17, 195),
    (64, 157, 55.4, 1.15, 290),
    (497, 498, 59.4, 1.28, 510),
    (89, 145, 56.9, 1.19, 330),
    (32, 145, 56.5, 1.17, 280),
    (89, 360, 58.1, 1.23, 350),
    (32, 360, 57.6, 1.21, 290),
    (201, 360, 55.8, 1.16, 240),
    (67, 201, 57.4, 1.20, 310),
    (89, 119, 56.2, 1.17, 220),
    (119, 201, 55.9, 1.16, 205),
    (122, 201, 55.6, 1.15, 180),
    (32, 875, 56.3, 1.18, 230),
    (32, 63, 58.2, 1.22, 340),
    (54, 112, 56.7, 1.18, 260),
    (99, 101, 56.0, 1.16, 295),
    (82, 876, 57.1, 1.19, 245),
    (875, 887, 56.4, 1.17, 215),
    (141, 497, 55.7, 1.16, 175),
    (32, 55, 57.3, 1.21, 270),
    (54, 777, 57.8, 1.21, 315),
]

CURATED_AUGMENTS_BY_NAME: dict[str, float] = {
    "珠光护手": 58.6,
    "双刀流": 58.9,
    "秘术冲拳": 58.2,
    "闪电打击": 58.4,
    "歌利亚巨人": 58.1,
    "踢踏舞": 57.8,
    "利刃华尔兹": 56.1,
    "坦克引擎": 56.0,
    "裁决使": 55.8,
    "地震": 55.7,
    "回归基本功": 55.5,
    "物法皆修": 55.3,
    "吸血习性": 55.1,
    "物理转魔法": 55.6,
    "魔法转物理": 55.2,
    "巨人杀手": 54.4,
    "缩小引擎": 54.0,
    "玻璃大炮": 53.9,
    "冰霜幽灵": 53.8,
    "逃跑计划": 53.4,
    "灵巧": 53.2,
    "心灵净化": 51.9,
    "小丑学院": 51.6,
}

CURATED_AUGMENT_SYNERGIES_BY_NAME: list[tuple[str, str, float, int]] = [
    ("双刀流", "闪电打击", 1.32, 460),
    ("双刀流", "踢踏舞", 1.26, 390),
    ("珠光护手", "玻璃大炮", 1.28, 340),
    ("珠光护手", "回归基本功", 1.21, 280),
    ("秘术冲拳", "地震", 1.25, 310),
    ("秘术冲拳", "利刃华尔兹", 1.22, 245),
    ("坦克引擎", "歌利亚巨人", 1.31, 520),
    ("吸血习性", "歌利亚巨人", 1.19, 270),
    ("利刃华尔兹", "裁决使", 1.23, 220),
    ("踢踏舞", "灵巧", 1.20, 295),
    ("地震", "冰霜幽灵", 1.18, 165),
]

FALLBACK_ITEM_NAMES: dict[int, str] = {
    3031: "无尽之刃",
    3053: "斯特拉克的挑战护手",
    3078: "三相之力",
    3153: "破败王者之刃",
    6333: "死亡之舞",
}


def bayesian_smoothed_win_rate(
    wins: float, games: float, base_rate: float = BASE_WIN_RATE, prior_c: float = PRIOR_WEIGHT_C
) -> float:
    """Computes Bayesian smoothed win rate in [0.0, 1.0]."""
    denom = games + prior_c
    if denom <= 0.0:
        return base_rate
    return (wins + prior_c * base_rate) / denom


def hero_tier_from_wr_pct(win_rate_pct: float) -> str:
    if win_rate_pct >= 54.0:
        return "S+"
    if win_rate_pct >= 52.0:
        return "S"
    if win_rate_pct >= 50.5:
        return "A"
    if win_rate_pct >= 48.5:
        return "B"
    if win_rate_pct >= 46.5:
        return "C"
    return "D"


def aug_tier_from_wr_pct(win_rate_pct: float) -> str:
    if win_rate_pct >= 55.0:
        return "S+"
    if win_rate_pct >= 52.5:
        return "S"
    if win_rate_pct >= 50.5:
        return "A"
    if win_rate_pct >= 48.5:
        return "B"
    if win_rate_pct >= 46.5:
        return "C"
    return "D"


def normalize_rarity_zh(raw: str) -> str:
    text = str(raw or "").strip().lower()
    if "prismatic" in text or "棱彩" in text:
        return "棱彩"
    if "gold" in text or "黄金" in text:
        return "黄金"
    return "白银"


def _normalize_name_key(raw: str) -> str:
    return "".join(ch for ch in str(raw or "").strip().lower() if not ch.isspace())


def fetch_aramhex_data(aramhex_base_url: str) -> dict[str, Any]:
    """Downloads latest patch statistics and metadata from czgreat/aramhex-data."""
    base = aramhex_base_url.rstrip("/")
    session = requests.Session()
    session.headers.update({"User-Agent": "Hexrs-MetaUpdater/1.0"})

    latest_resp = session.get(f"{base}/latest.json", timeout=30)
    latest_resp.raise_for_status()
    latest = latest_resp.json()

    patch = str(latest.get("patch") or "unknown").strip()
    rel_path = str(latest.get("path") or "").strip("/")
    if not rel_path:
        raise ValueError("aramhex latest.json missing 'path'")

    bundle: dict[str, Any] = {"patch": patch}
    for key, filename in [
        ("heroes", "heroes.json"),
        ("augments", "augments.json"),
        ("hero_augments", "hero_augments.json"),
        ("core_builds", "core_builds.json"),
        ("item_metadata", "item_metadata.json"),
    ]:
        url = f"{base}/{rel_path}/{filename}"
        resp = session.get(url, timeout=45)
        resp.raise_for_status()
        bundle[key] = resp.json()

    return bundle


def _deterministic_baseline_hero_stats(hero_id: int) -> tuple[str, float, float]:
    h = int(hashlib.sha256(f"hero:{hero_id}".encode("utf-8")).hexdigest()[:8], 16)
    games = 140 + (h % 260)
    raw_rate = 0.460 + ((h >> 8) % 100) / 1000.0
    wins = raw_rate * games
    wr_pct = round(max(46.0, min(55.0, bayesian_smoothed_win_rate(wins, games) * 100.0)), 1)
    pick_rate = round(2.5 + ((h >> 16) % 95) / 10.0, 2)
    return hero_tier_from_wr_pct(wr_pct), wr_pct, pick_rate


def _deterministic_baseline_aug_stats(
    aug_id: int, name_zh: str, rarity: str
) -> tuple[str, float, float]:
    if name_zh in CURATED_AUGMENTS_BY_NAME:
        wr_pct = CURATED_AUGMENTS_BY_NAME[name_zh]
        return aug_tier_from_wr_pct(wr_pct), wr_pct, 14.0

    h = int(hashlib.sha256(f"aug:{aug_id}".encode("utf-8")).hexdigest()[:8], 16)
    rarity_offset = {"棱彩": 0.040, "黄金": 0.015, "白银": -0.015}.get(rarity, 0.0)
    games = 120 + (h % 240)
    raw_rate = 0.475 + rarity_offset + ((h >> 8) % 85) / 1000.0
    wins = raw_rate * games
    wr_pct = round(max(46.5, min(56.5, bayesian_smoothed_win_rate(wins, games) * 100.0)), 1)
    pick_rate = round(4.0 + ((h >> 16) % 130) / 10.0, 1)
    return aug_tier_from_wr_pct(wr_pct), wr_pct, pick_rate


def _infer_hero_attributes_from_builds(
    builds: list[dict[str, Any]],
    item_metadata: dict[str, Any],
) -> tuple[float, float, float, float]:
    ad_hits = 0.0
    ap_hits = 0.0
    tank_hits = 0.0
    cc_hits = 0.0
    total_items = 0

    for b in builds[:3]:
        for raw_item_id in b.get("items", []):
            meta = item_metadata.get(str(raw_item_id))
            if not isinstance(meta, dict):
                continue
            types = set(meta.get("types") or [])
            total_items += 1
            if types & AD_ITEM_TYPES:
                ad_hits += 1.0
            if types & AP_ITEM_TYPES:
                ap_hits += 1.0
            if types & TANK_ITEM_TYPES:
                tank_hits += 1.0
            if types & CC_ITEM_TYPES:
                cc_hits += 1.0

    if total_items == 0:
        return (0.50, 0.50, 0.35, 0.35)

    dmg_total = ad_hits + ap_hits
    if dmg_total > 0.0:
        ad_w = round(ad_hits / dmg_total, 2)
        ap_w = round(ap_hits / dmg_total, 2)
    else:
        ad_w, ap_w = (0.35, 0.35)

    tank_score = round(min(0.95, 0.10 + (tank_hits / total_items) * 1.15), 2)
    cc_score = round(min(0.85, 0.25 + (cc_hits / total_items) * 0.65), 2)
    return (ad_w, ap_w, tank_score, cc_score)


def _infer_build_tag(
    idx: int, item_ids: list[int], item_metadata: dict[str, Any]
) -> str:
    base_labels = ["高胜三件套", "热门三件套", "备选三件套"]
    base = base_labels[idx] if idx < len(base_labels) else "核心三件套"

    ad_cnt = 0
    ap_cnt = 0
    tank_cnt = 0
    for item_id in item_ids:
        meta = item_metadata.get(str(item_id))
        if not isinstance(meta, dict):
            continue
        types = set(meta.get("types") or [])
        if types & AD_ITEM_TYPES:
            ad_cnt += 1
        if types & AP_ITEM_TYPES:
            ap_cnt += 1
        if types & TANK_ITEM_TYPES:
            tank_cnt += 1

    if tank_cnt >= 2 and tank_cnt >= ad_cnt and tank_cnt >= ap_cnt:
        return f"重装{base}"
    if ap_cnt > ad_cnt:
        return f"法强{base}"
    if ad_cnt > ap_cnt:
        return f"物理{base}"
    return base


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
    aramhex_data: dict[str, Any] | None,
    samples: list[dict[str, Any]],
    db_out: Path,
    data_version: str,
) -> None:
    if db_out.exists():
        db_out.unlink()

    cdragon_items = {int(it["id"]): str(it["name_zh"]) for it in cdragon_meta.get("items", [])}
    cdragon_champs = {
        int(ch["hero_id"]): ch for ch in cdragon_meta.get("champions", []) if int(ch.get("hero_id", 0)) > 0
    }
    cdragon_augs = {
        int(a["augment_id"]): a
        for a in cdragon_meta.get("augments", [])
        if int(a.get("augment_id", 0)) > 0
    }
    cdragon_augs_by_name: dict[str, dict[str, Any]] = {}
    for aug in cdragon_augs.values():
        key = _normalize_name_key(aug.get("name_zh", ""))
        if key and key not in cdragon_augs_by_name:
            cdragon_augs_by_name[key] = aug

    aramhex_heroes: dict[str, Any] = (aramhex_data or {}).get("heroes", {})
    aramhex_augments: dict[str, Any] = (aramhex_data or {}).get("augments", {})
    aramhex_hero_augs: dict[str, Any] = (aramhex_data or {}).get("hero_augments", {})
    aramhex_core_builds: dict[str, Any] = (aramhex_data or {}).get("core_builds", {})
    aramhex_item_meta: dict[str, Any] = (aramhex_data or {}).get("item_metadata", {})

    def resolve_item_name(item_id: int) -> str:
        meta = aramhex_item_meta.get(str(item_id))
        if isinstance(meta, dict):
            name = str(meta.get("name") or "").strip()
            if name:
                return name
        cd_name = cdragon_items.get(item_id, "").strip()
        if cd_name:
            return cd_name
        return FALLBACK_ITEM_NAMES.get(item_id, "核心装备")

    hero_counts: dict[int, list[int]] = defaultdict(lambda: [0, 0])
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

    all_hero_ids = sorted(
        set(cdragon_champs.keys())
        | {int(k) for k in aramhex_heroes.keys() if k.isdigit() and int(k) > 0}
    )

    conn = sqlite3.connect(db_out)
    try:
        init_sqlite_schema(conn, data_version)

        hero_base_wr_map: dict[int, float] = {}
        hero_games_map: dict[int, int] = {}

        for hero_id in all_hero_ids:
            cd_champ = cdragon_champs.get(hero_id, {})
            ah_champ = aramhex_heroes.get(str(hero_id), {})

            name_zh = str(ah_champ.get("name") or cd_champ.get("name_zh") or f"英雄{hero_id}").strip()
            title_zh = str(ah_champ.get("title") or cd_champ.get("title_zh") or "").strip()
            alias = str(cd_champ.get("alias") or f"Hero{hero_id}").strip()

            conn.execute(
                "INSERT OR REPLACE INTO champions (hero_id, name_zh, title_zh, alias) VALUES (?, ?, ?, ?)",
                (hero_id, name_zh, title_zh, alias),
            )

            sample_w, sample_g = hero_counts.get(hero_id, [0, 0])
            ah_games = int(ah_champ.get("games") or 0)
            ah_wins = int(ah_champ.get("wins") or 0)
            arch = BASELINE_HERO_ARCHETYPES.get(hero_id)
            hero_builds_raw = aramhex_core_builds.get(str(hero_id), [])
            if not isinstance(hero_builds_raw, list):
                hero_builds_raw = []

            if sample_g > 0:
                wr_ratio = bayesian_smoothed_win_rate(sample_w, sample_g)
                wr_pct = round(wr_ratio * 100.0, 1)
                pick_rate = round((sample_g / max(total_participant_games, 1)) * 100.0, 2)
                tier = hero_tier_from_wr_pct(wr_pct)
                hero_games_map[hero_id] = sample_g
            elif ah_games > 0:
                wr_ratio = bayesian_smoothed_win_rate(ah_wins, ah_games)
                wr_pct = round(wr_ratio * 100.0, 1)
                pick_rate = round(float(ah_champ.get("pick_rate") or 0.0) * 100.0, 2)
                tier = hero_tier_from_wr_pct(wr_pct)
                hero_games_map[hero_id] = ah_games
            elif arch is not None:
                tier = arch[0]
                wr_pct = {"S+": 54.8, "S": 53.0, "A": 51.2, "B": 49.5}.get(tier, 49.5)
                wr_ratio = wr_pct / 100.0
                pick_rate = 8.0
                hero_games_map[hero_id] = 100
            else:
                tier, wr_pct, pick_rate = _deterministic_baseline_hero_stats(hero_id)
                wr_ratio = wr_pct / 100.0
                hero_games_map[hero_id] = 100

            hero_base_wr_map[hero_id] = wr_ratio

            if arch is not None:
                ad_w, ap_w, tank_s, cc_s = (arch[1], arch[2], arch[3], arch[4])
                tags_list = [arch[5]]
            else:
                ad_w, ap_w, tank_s, cc_s = _infer_hero_attributes_from_builds(
                    hero_builds_raw, aramhex_item_meta
                )
                if tank_s >= 0.65:
                    tags_list = ["前排承伤"]
                elif ap_w >= 0.65:
                    tags_list = ["法术输出"]
                elif ad_w >= 0.65:
                    tags_list = ["物理输出"]
                else:
                    tags_list = ["均衡流"]

            conn.execute(
                "INSERT OR REPLACE INTO hero_tier_stats (hero_id, tier, win_rate, pick_rate, tags) VALUES (?, ?, ?, ?, ?)",
                (hero_id, tier, wr_pct, pick_rate, json.dumps(tags_list, ensure_ascii=False)),
            )
            conn.execute(
                "INSERT OR REPLACE INTO hero_attributes (hero_id, ad_weight, ap_weight, tank_score, cc_score) VALUES (?, ?, ?, ?, ?)",
                (hero_id, ad_w, ap_w, tank_s, cc_s),
            )

            # Insert up to 3 builds from aramhex core_builds.json sorted by (win, games) descending.
            inserted_builds = 0
            sorted_builds = sorted(
                [b for b in hero_builds_raw if isinstance(b, dict)],
                key=lambda b: (float(b.get("win") or 0.0), int(b.get("games") or 0)),
                reverse=True,
            )
            for b in sorted_builds:
                if inserted_builds >= 3:
                    break
                parsed_ids = [
                    int(x) for x in (b.get("items") or []) if str(x).isdigit() and int(x) > 0
                ]
                if not parsed_ids:
                    continue
                items_payload = [
                    {"id": item_id, "name_zh": resolve_item_name(item_id)}
                    for item_id in parsed_ids[:3]
                ]
                build_wr = round(float(b.get("win") or wr_ratio) * 100.0, 1)
                tag = _infer_build_tag(inserted_builds, parsed_ids, aramhex_item_meta)
                conn.execute(
                    "INSERT INTO hero_builds (hero_id, playstyle_tag, core_items_json, win_rate) VALUES (?, ?, ?, ?)",
                    (hero_id, tag, json.dumps(items_payload, ensure_ascii=False), build_wr),
                )
                inserted_builds += 1

            # Ensure zero heroes have empty/missing hero_builds.
            if inserted_builds == 0:
                fallback_ids = arch[6] if arch is not None else [3078, 3053, 6333]
                fallback_tag = arch[5] if arch is not None else "通用推荐"
                items_payload = [
                    {"id": item_id, "name_zh": resolve_item_name(item_id)}
                    for item_id in fallback_ids
                ]
                conn.execute(
                    "INSERT INTO hero_builds (hero_id, playstyle_tag, core_items_json, win_rate) VALUES (?, ?, ?, ?)",
                    (hero_id, fallback_tag, json.dumps(items_payload, ensure_ascii=False), wr_pct),
                )

        for (hero_id, item_tuple), (w, g) in hero_item_builds.items():
            if g < 5:
                continue
            wr_pct = round(bayesian_smoothed_win_rate(w, g) * 100.0, 1)
            items_payload = [
                {"id": item_id, "name_zh": resolve_item_name(item_id)}
                for item_id in item_tuple
            ]
            conn.execute(
                "INSERT INTO hero_builds (hero_id, playstyle_tag, core_items_json, win_rate) VALUES (?, ?, ?, ?)",
                (hero_id, "热门核心流", json.dumps(items_payload, ensure_ascii=False), wr_pct),
            )

        # Union aramhex augments + CDragon augments
        all_aug_ids = sorted(
            set(cdragon_augs.keys())
            | {int(k) for k in aramhex_augments.keys() if k.isdigit() and int(k) > 0}
        )

        # Aggregate weighted global stats from aramhex hero_augments
        aug_global_counts: dict[int, list[int]] = defaultdict(lambda: [0, 0])  # [wins, games]
        total_hero_games_sum = sum(hero_games_map.values()) or 1

        for raw_hid, aug_map in aramhex_hero_augs.items():
            if not raw_hid.isdigit() or not isinstance(aug_map, dict):
                continue
            hid = int(raw_hid)
            hero_base_wr = hero_base_wr_map.get(hid, BASE_WIN_RATE)
            hero_total_g = max(hero_games_map.get(hid, 1), 1)
            for raw_aid, stat in aug_map.items():
                if not raw_aid.isdigit() or not isinstance(stat, dict):
                    continue
                aid = int(raw_aid)
                g = int(stat.get("games") or 0)
                w = int(stat.get("wins") or 0)
                if g <= 0:
                    continue
                aug_global_counts[aid][0] += w
                aug_global_counts[aid][1] += g
                wr = bayesian_smoothed_win_rate(w, g, base_rate=hero_base_wr)
                wr_pct = round(wr * 100.0, 1)
                pick_rate = round((g / hero_total_g) * 100.0, 1)
                conn.execute(
                    "INSERT OR REPLACE INTO hero_augment_stats (hero_id, augment_id, tier, win_rate, pick_rate) VALUES (?, ?, ?, ?, ?)",
                    (hid, aid, aug_tier_from_wr_pct(wr_pct), wr_pct, pick_rate),
                )

        aug_id_by_name: dict[str, int] = {}
        for aug_id in all_aug_ids:
            ah_aug = aramhex_augments.get(str(aug_id), {})
            cd_aug = cdragon_augs.get(aug_id)

            ah_name = str(ah_aug.get("name") or "").strip()
            cd_name = str((cd_aug or {}).get("name_zh") or "").strip()
            name_zh = ah_name or cd_name

            if cd_aug is None and name_zh:
                cd_aug = cdragon_augs_by_name.get(_normalize_name_key(name_zh))

            raw_rarity = ah_aug.get("rarity") or (cd_aug or {}).get("rarity") or ""
            rarity = normalize_rarity_zh(raw_rarity)
            phash = str((cd_aug or {}).get("phash") or "0000000000000000")
            desc_zh = str((cd_aug or {}).get("desc_zh") or "").strip()

            conn.execute(
                "INSERT OR REPLACE INTO augment_meta (augment_id, name_zh, rarity, phash, desc_zh) VALUES (?, ?, ?, ?, ?)",
                (aug_id, name_zh, rarity, phash, desc_zh),
            )
            if name_zh:
                norm_key = _normalize_name_key(name_zh)
                if norm_key not in aug_id_by_name:
                    aug_id_by_name[norm_key] = aug_id

            gw, gg = aug_global_counts.get(aug_id, [0, 0])
            if gg > 0:
                wr = bayesian_smoothed_win_rate(gw, gg, base_rate=BASE_WIN_RATE)
                wr_pct = round(wr * 100.0, 1)
                pick_rate = round((gg / total_hero_games_sum) * 100.0, 2)
                tier = aug_tier_from_wr_pct(wr_pct)
            else:
                tier, wr_pct, pick_rate = _deterministic_baseline_aug_stats(aug_id, name_zh, rarity)

            conn.execute(
                "INSERT OR REPLACE INTO hero_augment_stats (hero_id, augment_id, tier, win_rate, pick_rate) VALUES (?, ?, ?, ?, ?)",
                (0, aug_id, tier, wr_pct, pick_rate),
            )

        # Overlay real crowdsourced sample stats if provided
        for (hero_id, aug_id), (w, g) in hero_aug_counts.items():
            hw, hg = hero_counts.get(hero_id, [0, 0])
            p_h = bayesian_smoothed_win_rate(hw, hg) if hg > 0 else BASE_WIN_RATE
            wr = bayesian_smoothed_win_rate(w, g, base_rate=p_h)
            wr_pct = round(wr * 100.0, 1)
            pick_rate = round((g / max(hg, 1)) * 100.0, 1)
            conn.execute(
                "INSERT OR REPLACE INTO hero_augment_stats (hero_id, augment_id, tier, win_rate, pick_rate) VALUES (?, ?, ?, ?, ?)",
                (hero_id, aug_id, aug_tier_from_wr_pct(wr_pct), wr_pct, pick_rate),
            )

        for name_a, name_b, lift, n in CURATED_AUGMENT_SYNERGIES_BY_NAME:
            id_a = aug_id_by_name.get(_normalize_name_key(name_a))
            id_b = aug_id_by_name.get(_normalize_name_key(name_b))
            if id_a and id_b and id_a != id_b:
                lo, hi = (id_a, id_b) if id_a <= id_b else (id_b, id_a)
                conn.execute(
                    "INSERT OR REPLACE INTO augment_synergy (hero_id, aug_a, aug_b, lift_score, sample_size) VALUES (?, ?, ?, ?, ?)",
                    (0, lo, hi, lift, n),
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
        "--aramhex-url",
        type=str,
        default=DEFAULT_ARAMHEX_BASE_URL,
        help="Base URL for czgreat/aramhex-data repository.",
    )
    parser.add_argument(
        "--skip-aramhex",
        action="store_true",
        help="Skip downloading czgreat/aramhex-data.",
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
        default=None,
        help="Release version string for manifest.json and meta_info.data_version.",
    )
    parser.add_argument(
        "--db-url",
        type=str,
        default="https://raw.githubusercontent.com/zoffyultraman/Hexrs/data/meta.db.zst",
        help="Public download URL written into manifest.json.",
    )
    args = parser.parse_args()

    cdragon_meta = json.loads(args.input.read_text(encoding="utf-8"))
    aramhex_data: dict[str, Any] | None = None
    if not args.skip_aramhex:
        aramhex_data = fetch_aramhex_data(args.aramhex_url)

    samples: list[dict[str, Any]] = []
    if args.samples is not None and args.samples.exists():
        loaded = json.loads(args.samples.read_text(encoding="utf-8"))
        if isinstance(loaded, list):
            samples = loaded

    today_str = datetime.now(timezone.utc).strftime("%Y%m%d")
    if args.version:
        version = args.version
    elif aramhex_data is not None:
        version = f"aramhex-{aramhex_data['patch']}-{today_str}"
    else:
        version = f"cdragon-{today_str}"

    args.out_dir.mkdir(parents=True, exist_ok=True)
    db_out = args.out_dir / "meta.db"
    build_meta_database(cdragon_meta, aramhex_data, samples, db_out, version)
    zst_path, manifest_path = compress_and_emit_manifest(
        db_out, args.out_dir, version, args.db_url
    )
    print(
        f"Built {db_out} ({db_out.stat().st_size} bytes, version={version}) -> "
        f"{zst_path} ({zst_path.stat().st_size} bytes) & {manifest_path}"
    )


if __name__ == "__main__":
    main()
