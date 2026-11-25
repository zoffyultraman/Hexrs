#!/usr/bin/env python3
"""Fetch CommunityDragon champions, items, and Arena/Hex augments + compute exact DCT-II pHashes.

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
import io
import json
import math
from pathlib import Path
from typing import Any

import numpy as np
import requests
from PIL import Image

CDRAGON_BASE = "https://raw.communitydragon.org/latest"
CHAMPIONS_URL = f"{CDRAGON_BASE}/plugins/rcp-be-lol-game-data/global/zh_cn/v1/champion-summary.json"
ITEMS_URL = f"{CDRAGON_BASE}/plugins/rcp-be-lol-game-data/global/zh_cn/v1/items.json"
ARENA_DATA_URL = f"{CDRAGON_BASE}/cdragon/arena/zh_cn.json"
CHERRY_AUGMENTS_URL = (
    f"{CDRAGON_BASE}/plugins/rcp-be-lol-game-data/global/zh_cn/v1/cherry-augments.json"
)

DCT_SIZE = 32
LOW_FREQ_SIZE = 8


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


def fetch_cdragon_metadata(
    cache_dir: Path | None = None,
    skip_icons: bool = False,
    limit_icons: int | None = None,
) -> dict[str, Any]:
    session = requests.Session()
    session.headers.update({"User-Agent": "Hexrs-MetaUpdater/1.0"})

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

    augments_raw: list[dict[str, Any]] = []
    try:
        arena_bundle = fetch_json(session, ARENA_DATA_URL, timeout=45)
        if isinstance(arena_bundle, dict) and isinstance(arena_bundle.get("augments"), list):
            augments_raw = arena_bundle["augments"]
    except Exception:
        augments_raw = []

    if not augments_raw:
        try:
            cherry_raw = fetch_json(session, CHERRY_AUGMENTS_URL, timeout=30)
            if isinstance(cherry_raw, list):
                augments_raw = cherry_raw
        except Exception:
            augments_raw = []

    if cache_dir is not None:
        cache_dir.mkdir(parents=True, exist_ok=True)

    augments = []
    hashed_count = 0
    for aug in augments_raw:
        aug_id = int(aug.get("id", -1))
        if aug_id <= 0:
            continue
        name_zh = str(aug.get("name") or aug.get("nameTRA") or "").strip()
        desc_zh = str(aug.get("desc") or aug.get("tooltip") or "").strip()
        icon_raw = str(
            aug.get("iconLarge") or aug.get("iconSmall") or aug.get("augmentSmallIconPath") or ""
        )
        icon_url = normalize_cdragon_asset_url(icon_raw)

        phash = "0000000000000000"
        if not skip_icons and icon_url and (limit_icons is None or hashed_count < limit_icons):
            cache_file = cache_dir / f"{aug_id}.png" if cache_dir is not None else None
            try:
                if cache_file is not None and cache_file.exists():
                    img_bytes = cache_file.read_bytes()
                else:
                    icon_resp = session.get(icon_url, timeout=15)
                    icon_resp.raise_for_status()
                    img_bytes = icon_resp.content
                    if cache_file is not None:
                        cache_file.write_bytes(img_bytes)
                with Image.open(io.BytesIO(img_bytes)) as img:
                    phash = compute_rgba_phash(img)
                hashed_count += 1
            except Exception:
                pass

        augments.append(
            {
                "augment_id": aug_id,
                "name_zh": name_zh,
                "rarity": rarity_zh(aug.get("rarity")),
                "phash": phash,
                "desc_zh": desc_zh,
                "icon_url": icon_url,
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
        help="Optional limit on number of augment icons to hash.",
    )
    args = parser.parse_args()

    payload = fetch_cdragon_metadata(
        cache_dir=args.cache_dir,
        skip_icons=args.skip_icons,
        limit_icons=args.limit_icons,
    )

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
    print(
        f"Saved {len(payload['champions'])} champions, "
        f"{len(payload['items'])} items, {len(payload['augments'])} augments -> {args.output}"
    )


if __name__ == "__main__":
    main()
