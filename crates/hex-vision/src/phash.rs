//! Deterministic 64-bit DCT perceptual hashing (`< 0.3ms` per ROI) and Hamming matching.
//!
//! # Algorithm Specification (must match `scripts/fetch_cdragon.py` bit-for-bit)
//! 1. Downsample an RGBA8 region `(x, y, w, h)` using box area averaging to `32x32` grayscale
//!    luminance: `L = 0.299 * R + 0.587 * G + 0.114 * B` (`f32`).
//! 2. Compute the separable 2D DCT-II (`32x32`), keeping only the top-left `8x8` low-frequency
//!    block (`v in 0..8, u in 0..8`), with orthonormal scaling:
//!    `C(0) = sqrt(1/32)`, `C(k > 0) = sqrt(2/32)`.
//! 3. Exclude the DC term `(0, 0)` to obtain 63 AC coefficients. Compute their median
//!    (`sorted[31]` of 63 ascending `f32` values).
//! 4. Pack into a `u64`: bit `0` (`(0, 0)`) is always `0`; for `(v, u) != (0, 0)`, bit
//!    `bit_idx = v * 8 + u` (`1..=63`) is `1` iff `dct[v][u] > median`.

use std::f32::consts::PI;
use std::sync::LazyLock;
use std::time::Instant;

use hex_core::RoiRect;
use hex_data::AugmentHashEntry;

const DCT_SIZE: usize = 32;
const LOW_FREQ_SIZE: usize = 8;
const DEFAULT_MAX_HAMMING_DISTANCE: u32 = 14;

static DCT_LUT: LazyLock<[[f32; DCT_SIZE]; LOW_FREQ_SIZE]> = LazyLock::new(|| {
    let mut lut = [[0.0_f32; DCT_SIZE]; LOW_FREQ_SIZE];
    let c0 = (1.0_f32 / DCT_SIZE as f32).sqrt();
    let ck = (2.0_f32 / DCT_SIZE as f32).sqrt();

    for u in 0..LOW_FREQ_SIZE {
        let scale = if u == 0 { c0 } else { ck };
        for x in 0..DCT_SIZE {
            let angle = ((2 * x + 1) as f32) * (u as f32) * PI / (2.0 * DCT_SIZE as f32);
            lut[u][x] = scale * angle.cos();
        }
    }
    lut
});

/// Computes the Hamming distance (`0..=64`) between two 64-bit perceptual hashes.
#[inline]
pub fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Computes the 64-bit DCT pHash for a full tightly-packed RGBA8 image buffer.
pub fn compute_phash_from_rgba32(rgba: &[u8], width: u32, height: u32) -> Option<u64> {
    compute_region_phash(rgba, width, height, (0, 0, width, height))
}

/// Computes the 64-bit DCT pHash for pixel sub-rectangle `(rx, ry, rw, rh)` in an RGBA8 frame.
pub fn compute_region_phash(
    frame_rgba: &[u8],
    frame_width: u32,
    frame_height: u32,
    rect: (u32, u32, u32, u32),
) -> Option<u64> {
    if frame_width == 0 || frame_height == 0 {
        return None;
    }
    let expected_len = (frame_width as usize)
        .checked_mul(frame_height as usize)?
        .checked_mul(4)?;
    if frame_rgba.len() < expected_len {
        return None;
    }

    let rx = rect.0.min(frame_width.saturating_sub(1));
    let ry = rect.1.min(frame_height.saturating_sub(1));
    let rw = rect.2.min(frame_width.saturating_sub(rx));
    let rh = rect.3.min(frame_height.saturating_sub(ry));
    if rw == 0 || rh == 0 {
        return None;
    }

    let mut gray = [0.0_f32; DCT_SIZE * DCT_SIZE];
    let stride = frame_width as usize * 4;

    for oy in 0..DCT_SIZE {
        let y0 = ry + ((oy as u32 * rh) / DCT_SIZE as u32);
        let y1 = (ry + (((oy as u32 + 1) * rh) / DCT_SIZE as u32))
            .max(y0 + 1)
            .min(ry + rh);

        for ox in 0..DCT_SIZE {
            let x0 = rx + ((ox as u32 * rw) / DCT_SIZE as u32);
            let x1 = (rx + (((ox as u32 + 1) * rw) / DCT_SIZE as u32))
                .max(x0 + 1)
                .min(rx + rw);

            let mut sum = 0.0_f32;
            let mut count = 0_u32;

            for py in y0..y1 {
                let row_offset = py as usize * stride;
                for px in x0..x1 {
                    let idx = row_offset + px as usize * 4;
                    let r = frame_rgba[idx] as f32;
                    let g = frame_rgba[idx + 1] as f32;
                    let b = frame_rgba[idx + 2] as f32;
                    sum += 0.299 * r + 0.587 * g + 0.114 * b;
                    count += 1;
                }
            }

            gray[oy * DCT_SIZE + ox] = if count > 0 { sum / count as f32 } else { 0.0 };
        }
    }

    let lut = &*DCT_LUT;

    // Separable 2D DCT-II: horizontal pass (32 rows x 8 low-frequency columns).
    let mut row_dct = [[0.0_f32; LOW_FREQ_SIZE]; DCT_SIZE];
    for y in 0..DCT_SIZE {
        let row = &gray[y * DCT_SIZE..(y + 1) * DCT_SIZE];
        for u in 0..LOW_FREQ_SIZE {
            let basis = &lut[u];
            let mut acc = 0.0_f32;
            for x in 0..DCT_SIZE {
                acc += row[x] * basis[x];
            }
            row_dct[y][u] = acc;
        }
    }

    // Vertical pass (8x8 low-frequency block).
    let mut dct = [[0.0_f32; LOW_FREQ_SIZE]; LOW_FREQ_SIZE];
    let mut ac_coeffs = [0.0_f32; LOW_FREQ_SIZE * LOW_FREQ_SIZE - 1];
    let mut ac_idx = 0_usize;

    for v in 0..LOW_FREQ_SIZE {
        let basis = &lut[v];
        for u in 0..LOW_FREQ_SIZE {
            let mut acc = 0.0_f32;
            for y in 0..DCT_SIZE {
                acc += row_dct[y][u] * basis[y];
            }
            dct[v][u] = acc;
            if v != 0 || u != 0 {
                ac_coeffs[ac_idx] = acc;
                ac_idx += 1;
            }
        }
    }

    ac_coeffs.sort_by(f32::total_cmp);
    let median = ac_coeffs[ac_coeffs.len() / 2];

    let mut hash = 0_u64;
    for v in 0..LOW_FREQ_SIZE {
        for u in 0..LOW_FREQ_SIZE {
            if v == 0 && u == 0 {
                continue;
            }
            if dct[v][u] > median {
                let bit_idx = v * LOW_FREQ_SIZE + u;
                hash |= 1_u64 << bit_idx;
            }
        }
    }

    Some(hash)
}

/// Fast in-memory augment icon pHash matcher (`< 2ms` across 3 card ROIs).
#[derive(Debug, Clone)]
pub struct AugmentMatcher {
    entries: Vec<AugmentHashEntry>,
    max_distance: u32,
}

impl Default for AugmentMatcher {
    fn default() -> Self {
        Self::new(Vec::new(), DEFAULT_MAX_HAMMING_DISTANCE)
    }
}

impl AugmentMatcher {
    /// Creates a new matcher from preloaded `meta.db` augment hashes and a Hamming threshold.
    pub fn new(entries: Vec<AugmentHashEntry>, max_distance: u32) -> Self {
        Self {
            entries,
            max_distance,
        }
    }

    /// Replaces the hash dictionary (e.g., after a `meta.db` hot-update).
    pub fn update_entries(&mut self, entries: Vec<AugmentHashEntry>) {
        self.entries = entries;
    }

    /// Updates the maximum accepted Hamming distance (`0..=64`).
    pub fn set_max_distance(&mut self, max_distance: u32) {
        self.max_distance = max_distance;
    }

    /// Returns `(augment_id, distance)` for the closest hash within `max_distance`.
    pub fn match_hash(&self, hash: u64) -> Option<(i32, u32)> {
        let mut best: Option<(i32, u32)> = None;
        for entry in &self.entries {
            let dist = hamming_distance(hash, entry.phash);
            if dist <= self.max_distance
                && best.map(|(_, best_dist)| dist < best_dist).unwrap_or(true)
            {
                best = Some((entry.augment_id, dist));
            }
        }
        best
    }

    /// Crops the 3 augment card icon ROIs, computes pHashes, and matches all 3 cards.
    ///
    /// Returns `Some((candidate_augment_ids, elapsed_ms))` when all 3 ROIs match known augments.
    pub fn match_offer(
        &self,
        frame_rgba: &[u8],
        width: u32,
        height: u32,
        rois: &[RoiRect; 3],
    ) -> Option<(Vec<i32>, u64)> {
        if self.entries.is_empty() {
            return None;
        }

        let start = Instant::now();
        let mut candidates = Vec::with_capacity(3);

        for roi in rois {
            let pixel_rect = roi.to_pixel_rect(width, height);
            let hash = compute_region_phash(frame_rgba, width, height, pixel_rect)?;
            let (aug_id, _dist) = self.match_hash(hash)?;
            candidates.push(aug_id);
        }

        let elapsed_ms = start.elapsed().as_millis() as u64;
        Some((candidates, elapsed_ms))
    }

    /// Adaptive multi-offset, multi-scale pHash matcher across the 3 augment card slots.
    ///
    /// Probes small `(dx, dy, scale)` offsets on an anchor slot to compensate for resolution,
    /// aspect ratio, and UI patch drift, then aligns all 3 slots (`0` placeholder if one fails)
    /// and succeeds whenever at least 2 distinct positive augment IDs match within `max_distance`.
    pub fn match_offer_adaptive(
        &self,
        frame_rgba: &[u8],
        width: u32,
        height: u32,
        rois: &[RoiRect; 3],
    ) -> Option<(Vec<i32>, u64)> {
        if self.entries.is_empty() {
            return None;
        }

        const DY_OFFSETS: &[f32] = &[0.0, -0.025, 0.025, -0.05, 0.05];
        const DX_OFFSETS: &[f32] = &[0.0, -0.012, 0.012];
        const SCALES: &[f32] = &[1.0, 0.90, 1.12];
        const FINE_DX: &[f32] = &[0.0, -0.008, 0.008];
        const ANCHOR_ORDER: [usize; 3] = [1, 0, 2];

        let start = Instant::now();
        let mut locked: Option<(f32, f32, f32, u32)> = None;

        for &slot in &ANCHOR_ORDER {
            let base_roi = rois[slot];
            for &dy in DY_OFFSETS {
                for &dx in DX_OFFSETS {
                    for &scale in SCALES {
                        let candidate_roi = adjust_roi(base_roi, dx, dy, scale);
                        if let Some((_aug_id, dist)) =
                            self.match_roi(frame_rgba, width, height, candidate_roi)
                        {
                            let replace = locked
                                .map(|(_, _, _, best_dist)| dist < best_dist)
                                .unwrap_or(true);
                            if replace {
                                locked = Some((dx, dy, scale, dist));
                            }
                            if dist <= 4 {
                                break;
                            }
                        }
                    }
                }
            }
            if locked.is_some() {
                break;
            }
        }

        let (base_dx, base_dy, base_scale, _) = locked?;

        let mut candidates = [0_i32; 3];
        let mut used_ids = Vec::with_capacity(3);
        let mut matched_count = 0_usize;

        for (slot, base_roi) in rois.iter().enumerate() {
            let mut best_slot: Option<(i32, u32)> = None;
            for &fine_dx in FINE_DX {
                let shifted = adjust_roi(*base_roi, base_dx + fine_dx, base_dy, base_scale);
                if let Some((aug_id, dist)) = self.match_roi(frame_rgba, width, height, shifted) {
                    if aug_id > 0
                        && !used_ids.contains(&aug_id)
                        && best_slot.map(|(_, b)| dist < b).unwrap_or(true)
                    {
                        best_slot = Some((aug_id, dist));
                    }
                }
            }

            if let Some((aug_id, _)) = best_slot {
                candidates[slot] = aug_id;
                used_ids.push(aug_id);
                matched_count += 1;
            }
        }

        if matched_count >= 2 {
            let elapsed_ms = start.elapsed().as_millis() as u64;
            Some((candidates.to_vec(), elapsed_ms))
        } else {
            None
        }
    }

    fn match_roi(
        &self,
        frame_rgba: &[u8],
        width: u32,
        height: u32,
        roi: RoiRect,
    ) -> Option<(i32, u32)> {
        let pixel_rect = roi.to_pixel_rect(width, height);
        let hash = compute_region_phash(frame_rgba, width, height, pixel_rect)?;
        self.match_hash(hash)
    }
}

fn adjust_roi(roi: RoiRect, dx: f32, dy: f32, scale: f32) -> RoiRect {
    let cx = roi.x + roi.w * 0.5 + dx;
    let cy = roi.y + roi.h * 0.5 + dy;
    let new_w = (roi.w * scale).clamp(0.01, 1.0);
    let new_h = (roi.h * scale).clamp(0.01, 1.0);
    let new_x = (cx - new_w * 0.5).clamp(0.0, (1.0 - new_w).max(0.0));
    let new_y = (cy - new_h * 0.5).clamp(0.0, (1.0 - new_h).max(0.0));
    RoiRect::new(new_x, new_y, new_w, new_h)
}
