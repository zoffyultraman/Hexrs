#!/usr/bin/env python3
"""Fetch CommunityDragon champions, items, and all Arena/Cherry/Hex augments + compute exact DCT-II pHashes.

The 64-bit pHash implementation matches `crates/hex-vision/src/phash.rs::compute_region_phash`
bit-for-bit:
1. Box area-averaged downsampling of RGBA8 image to 32x32 luminance:
   `L = 0.299 * R + 0.587 * G + 0.114 * B` (float32).
2. Separable 2D DCT-II with `C(0) = sqrt(1/32)`, `C(k > 0) = sqrt(2/32)`,
   keeping the top-left 8x8 low-frequency block.
3. Exclude DC `(0, 0)` -> 63 AC coefficients -> `median = sorted(ac)[31]`.
4. Pack into uint64: bit 0 is 0; for `(v, u) != (0, 0)`, bit `v * 8 + u` is 1 iff `dct[v, u] > median`.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import math
import threading
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from typing import Any

import numpy as np
import requests
from PIL import Image
from requests.adapters import HTTPAdapter

CDRAGON_BASE = "https://raw.communitydragon.org/latest"
CHAMPIONS_URL = f"{CDRAGON_BASE}/plugins/rcp-be-lol-game-data/global/zh_cn/v1/champion-summary.json"
ITEMS_URL = f"{CDRAGON_BASE}/plugins/rcp-be-lol-game-data/global/zh_cn/v1/items.json"
CHERRY_AUGMENTS_URL = (
    f"{CDRAGON_BASE}/plugins/rcp-be-lol-game-data/global/zh_cn/v1/cherry-augments.json"
)
ARENA_DATA_URL = f"{CDRAGON_BASE}/cdragon/arena/zh_cn.json"

DCT_SIZE = 32
LOW_FREQ_SIZE = 8
MAX_ICON_WORKERS = 16

_thread_local = threading.local()


def _get_session() -> requests.Session:
    session = getattr(_thread_local, "session", None)
    if session is None:
        session = requests.Session()
        adapter = HTTPAdapter(pool_connections=MAX_ICON_WORKERS, pool_maxsize=MAX_ICON_WORKERS)
        session.mount("https://", adapter)
        session.mount("http://", adapter)
        session.headers.update({"User-Agent": "Hexrs-MetaUpdater/1.0"})
        _thread_local.session = session
    return session


def _build_dct_lut() -> np.ndarray:
    lut = np.zeros((LOW_FREQ_SIZE, DCT_SIZE), dtype=np.float32)
    c0 = math.sqrt(1.0 / DCT_SIZE)
    ck = math.sqrt(2.0 / DCT_SIZE)
    for u in range(LOW_FREQ_SIZE):
        scale = np.float32(c0 if u == 0 else ck)
        for x in range(DCT_SIZE):
            angle = ((2 * x + 1) * u * math.pi) / (2.0 * DCT_SIZE)
            lut[u, x] = scale * np.float32(math.cos(angle))
    return lut


DCT_LUT = _build_dct_lut()


def compute_rgba_phash(img: Image.Image) -> str:
    """Computes the 16-char hex 64-bit DCT-II pHash matching Rust `compute_region_phash`."""
    rgba_img = img.convert("RGBA")
    arr = np.asarray(rgba_img, dtype=np.float32)
    rh, rw = arr.shape[0], arr.shape[1]
    if rw == 0 or rh == 0:
        return "0000000000000000"

    # Composite semi-transparent PNG edges over dark hextech card backdrop (#0B101B)
    alpha = arr[:, :, 3:4] / np.float32(255.0)
    backdrop = np.array([11.0, 16.0, 27.0], dtype=np.float32)
    rgb = arr[:, :, :3] * alpha + backdrop * (np.float32(1.0) - alpha)

    lum = (
        np.float32(0.299) * rgb[:, :, 0]
        + np.float32(0.587) * rgb[:, :, 1]
        + np.float32(0.114) * rgb[:, :, 2]
    )

    gray = np.zeros((DCT_SIZE, DCT_SIZE), dtype=np.float32)
    for oy in range(DCT_SIZE):
        y0 = (oy * rh) // DCT_SIZE
        y1 = min(max(((oy + 1) * rh) // DCT_SIZE, y0 + 1), rh)
        for ox in range(DCT_SIZE):
            x0 = (ox * rw) // DCT_SIZE
            x1 = min(max(((ox + 1) * rw) // DCT_SIZE, x0 + 1), rw)
            gray[oy, ox] = np.float32(lum[y0:y1, x0:x1].mean())

    row_dct = gray @ DCT_LUT.T
    dct = DCT_LUT @ row_dct

    ac_coeffs: list[float] = []
    for v in range(LOW_FREQ_SIZE):
        for u in range(LOW_FREQ_SIZE):
            if v == 0 and u == 0:
                continue
            ac_coeffs.append(float(dct[v, u]))

    ac_coeffs.sort()
    median = ac_coeffs[len(ac_coeffs) // 2]

    hash_val = 0
    for v in range(LOW_FREQ_SIZE):
        for u in range(LOW_FREQ_SIZE):
            if v == 0 and u == 0:
                continue
            if float(dct[v, u]) > median:
                bit_idx = v * LOW_FREQ_SIZE + u
                hash_val |= 1 << bit_idx

    return f"{hash_val:016x}"


def normalize_cdragon_asset_url(raw_path: str) -> str:
    """Converts CDragon `/lol-game-data/assets/...` or relative asset path to full HTTPS URL."""
    cleaned = raw_path.strip()
    if not cleaned:
        return ""
    if cleaned.startswith("http://") or cleaned.startswith("https://"):
        return cleaned
    lower = cleaned.lower()
    prefix = "/lol-game-data/assets/"
    if lower.startswith(prefix):
        rel = lower[len(prefix) :]
        return f"{CDRAGON_BASE}/plugins/rcp-be-lol-game-data/global/default/{rel}"
    if lower.startswith("assets/"):
        return f"{CDRAGON_BASE}/game/{lower}"
    return f"{CDRAGON_BASE}/game/{lower.lstrip('/')}"


def fetch_json(session: requests.Session, url: str, timeout: int = 30) -> Any:
    resp = session.get(url, timeout=timeout)
    resp.raise_for_status()
    return resp.json()


def rarity_zh(raw_rarity: Any) -> str:
    text = str(raw_rarity or "").lower()
    if "prismatic" in text or "kprismatic" in text or text == "4" or "棱彩" in text:
        return "棱彩"
    if "gold" in text or "kgold" in text or text == "1" or "黄金" in text:
        return "黄金"
    return "白银"


def _download_and_hash_icon(icon_url: str, cache_dir: Path | None) -> str:
    if not icon_url:
        return "0000000000000000"

    cache_file: Path | None = None
    if cache_dir is not None:
        digest = hashlib.sha256(icon_url.encode("utf-8")).hexdigest()[:24]
        cache_file = cache_dir / f"{digest}.png"
        if cache_file.exists() and cache_file.stat().st_size > 8:
            try:
                with Image.open(io.BytesIO(cache_file.read_bytes())) as img:
                    return compute_rgba_phash(img)
            except Exception:
                pass

    session = _get_session()
    try:
        resp = session.get(icon_url, timeout=15)
        resp.raise_for_status()
        img_bytes = resp.content
        with Image.open(io.BytesIO(img_bytes)) as img:
            phash = compute_rgba_phash(img)
        if cache_file is not None:
            cache_file.write_bytes(img_bytes)
        return phash
    except Exception:
        return "0000000000000000"


def fetch_cdragon_metadata(
    cache_dir: Path | None = None,
    skip_icons: bool = False,
    limit_icons: int | None = None,
) -> dict[str, Any]:
    session = _get_session()

    champions_raw = fetch_json(session, CHAMPIONS_URL)
    champions = []
    for item in champions_raw:
        hero_id = int(item.get("id", -1))
        if hero_id <= 0:
            continue
        champions.append(
            {
                "hero_id": hero_id,
                "name_zh": str(item.get("name") or "").strip(),
                "title_zh": str(item.get("title") or "").strip(),
                "alias": str(item.get("alias") or "").strip(),
            }
        )

    items_raw = fetch_json(session, ITEMS_URL)
    items = []
    for item in items_raw:
        item_id = int(item.get("id", -1))
        if item_id <= 0:
            continue
        items.append(
            {
                "id": item_id,
                "name_zh": str(item.get("name") or "").strip(),
                "price_total": int(item.get("priceTotal") or 0),
            }
        )

    cherry_raw: list[dict[str, Any]] = []
    try:
        loaded = fetch_json(session, CHERRY_AUGMENTS_URL, timeout=30)
        if isinstance(loaded, list):
            cherry_raw = loaded
    except Exception:
        cherry_raw = []

    arena_augments_by_id: dict[int, dict[str, Any]] = {}
    try:
        arena_bundle = fetch_json(session, ARENA_DATA_URL, timeout=45)
        if isinstance(arena_bundle, dict) and isinstance(arena_bundle.get("augments"), list):
            for entry in arena_bundle["augments"]:
                aid = int(entry.get("id", -1))
                if aid > 0:
                    arena_augments_by_id[aid] = entry
    except Exception:
        pass

    merged_by_id: dict[int, dict[str, Any]] = {}
    for aug in cherry_raw:
        aug_id = int(aug.get("id", -1))
        if aug_id <= 0:
            continue
        arena_extra = arena_augments_by_id.get(aug_id, {})
        name_zh = str(
            aug.get("nameTRA")
            or aug.get("name")
            or arena_extra.get("name")
            or aug.get("augmentNameId")
            or ""
        ).strip()
        desc_zh = str(
            aug.get("summaryDescription")
            or arena_extra.get("desc")
            or arena_extra.get("tooltip")
            or aug.get("desc")
            or ""
        ).strip()
        icon_raw = str(
            aug.get("augmentLargeIconPath")
            or aug.get("augmentSmallIconPath")
            or arena_extra.get("iconLarge")
            or arena_extra.get("iconSmall")
            or ""
        )
        merged_by_id[aug_id] = {
            "augment_id": aug_id,
            "name_zh": name_zh,
            "rarity": rarity_zh(aug.get("rarity") or arena_extra.get("rarity")),
            "desc_zh": desc_zh,
            "icon_url": normalize_cdragon_asset_url(icon_raw),
        }

    for aug_id, arena_aug in arena_augments_by_id.items():
        if aug_id in merged_by_id:
            continue
        name_zh = str(arena_aug.get("name") or arena_aug.get("nameTRA") or "").strip()
        desc_zh = str(arena_aug.get("desc") or arena_aug.get("tooltip") or "").strip()
        icon_raw = str(arena_aug.get("iconLarge") or arena_aug.get("iconSmall") or "")
        merged_by_id[aug_id] = {
            "augment_id": aug_id,
            "name_zh": name_zh,
            "rarity": rarity_zh(arena_aug.get("rarity")),
            "desc_zh": desc_zh,
            "icon_url": normalize_cdragon_asset_url(icon_raw),
        }

    if cache_dir is not None:
        cache_dir.mkdir(parents=True, exist_ok=True)

    augments_list = [merged_by_id[k] for k in sorted(merged_by_id)]
    url_to_phash: dict[str, str] = {}

    if not skip_icons:
        unique_urls: list[str] = []
        seen_urls: set[str] = set()
        for aug in augments_list:
            url = aug["icon_url"]
            if url and url not in seen_urls:
                seen_urls.add(url)
                unique_urls.append(url)
        if limit_icons is not None:
            unique_urls = unique_urls[:limit_icons]

        with ThreadPoolExecutor(max_workers=MAX_ICON_WORKERS) as pool:
            future_map = {
                pool.submit(_download_and_hash_icon, url, cache_dir): url for url in unique_urls
            }
            for future in as_completed(future_map):
                url = future_map[future]
                try:
                    url_to_phash[url] = future.result()
                except Exception:
                    url_to_phash[url] = "0000000000000000"

    augments = []
    for aug in augments_list:
        phash = url_to_phash.get(aug["icon_url"], "0000000000000000")
        augments.append(
            {
                "augment_id": aug["augment_id"],
                "name_zh": aug["name_zh"],
                "rarity": aug["rarity"],
                "phash": phash,
                "desc_zh": aug["desc_zh"],
                "icon_url": aug["icon_url"],
            }
        )

    return {
        "champions": champions,
        "items": items,
        "augments": augments,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Fetch CDragon metadata and compute pHashes.")
    parser.add_argument(
        "--output",
        type=Path,
        default=Path("dist/cdragon_meta.json"),
        help="Output JSON file path.",
    )
    parser.add_argument(
        "--cache-dir",
        type=Path,
        default=Path(".cache/cdragon_icons"),
        help="Optional PNG icon cache directory.",
    )
    parser.add_argument(
        "--skip-icons",
        action="store_true",
        help="Skip downloading augment PNG icons.",
    )
    parser.add_argument(
        "--limit-icons",
        type=int,
        default=None,
        help="Optional limit on number of unique augment icons to hash.",
    )
    args = parser.parse_args()

    payload = fetch_cdragon_metadata(
        cache_dir=args.cache_dir,
        skip_icons=args.skip_icons,
        limit_icons=args.limit_icons,
    )

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
    hashed_ok = sum(1 for a in payload["augments"] if a["phash"] != "0000000000000000")
    print(
        f"Saved {len(payload['champions'])} champions, "
        f"{len(payload['items'])} items, {len(payload['augments'])} augments "
        f"({hashed_ok} pHashed) -> {args.output}"
    )


if __name__ == "__main__":
    main()
