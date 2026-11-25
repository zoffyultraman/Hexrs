//! Multi-aspect-ratio ROI selection and lightweight augment-screen gate heuristic.

use std::sync::{Arc, RwLock};

use hex_core::{AspectRoiPreset, RoiManifest, RoiRect};

/// Thread-safe shared ROI manifest updateable via cloud `manifest.json` hot-updates.
pub type SharedRoiManifest = Arc<RwLock<RoiManifest>>;

/// Selects the aspect-ratio ROI preset (`16:9`, `16:10`, or `21:9`) matching `width x height`.
pub fn select_preset(manifest: &RoiManifest, width: u32, height: u32) -> &AspectRoiPreset {
    manifest.preset_for_resolution(width, height)
}

/// Lightweight heuristic checking whether the current frame displays the hextech augment modal.
///
/// Inspects `header_indicators` for hextech gold/teal trim over a dark translucent backdrop,
/// plus non-flat color variance inside the 3 card icon ROIs, so normal teamfights do not
/// trigger unnecessary pHash work or false matches.
pub fn detect_augment_screen(
    frame_rgba: &[u8],
    width: u32,
    height: u32,
    preset: &AspectRoiPreset,
) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    let Some(expected_len) = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
    else {
        return false;
    };
    if frame_rgba.len() < expected_len || preset.header_indicators.is_empty() {
        return false;
    }

    let mut total_samples = 0_u32;
    let mut hex_accent_samples = 0_u32;
    let mut dark_backdrop_samples = 0_u32;

    for indicator in &preset.header_indicators {
        let (rx, ry, rw, rh) = indicator.to_pixel_rect(width, height);
        if rw < 4 || rh < 4 {
            continue;
        }

        let step_x = (rw / 24).max(1);
        let step_y = (rh / 8).max(1);
        let stride = width as usize * 4;

        let mut py = ry;
        while py < ry + rh {
            let row_off = py as usize * stride;
            let mut px = rx;
            while px < rx + rw {
                let idx = row_off + px as usize * 4;
                let r = frame_rgba[idx];
                let g = frame_rgba[idx + 1];
                let b = frame_rgba[idx + 2];

                total_samples += 1;

                let is_gold = r >= 145
                    && g >= 110
                    && b <= 135
                    && i32::from(r) - i32::from(b) >= 45;
                let is_cyan = g >= 130
                    && b >= 130
                    && r <= 115
                    && i32::from(b) - i32::from(r) >= 40;

                if is_gold || is_cyan {
                    hex_accent_samples += 1;
                }

                let luma = (u32::from(r) * 77 + u32::from(g) * 150 + u32::from(b) * 29) >> 8;
                if luma <= 58 {
                    dark_backdrop_samples += 1;
                }

                px += step_x;
            }
            py += step_y;
        }
    }

    if total_samples == 0 {
        return false;
    }

    let accent_ratio = hex_accent_samples as f32 / total_samples as f32;
    let dark_ratio = dark_backdrop_samples as f32 / total_samples as f32;
    if accent_ratio < 0.035 || dark_ratio < 0.22 {
        return false;
    }

    preset
        .card_icons
        .iter()
        .all(|roi| has_icon_content(frame_rgba, width, height, *roi))
}

fn has_icon_content(frame_rgba: &[u8], width: u32, height: u32, roi: RoiRect) -> bool {
    let (rx, ry, rw, rh) = roi.to_pixel_rect(width, height);
    if rw < 8 || rh < 8 {
        return false;
    }

    let step_x = (rw / 8).max(1);
    let step_y = (rh / 8).max(1);
    let stride = width as usize * 4;

    let mut min_luma = 255_u32;
    let mut max_luma = 0_u32;

    let mut py = ry;
    while py < ry + rh {
        let row_off = py as usize * stride;
        let mut px = rx;
        while px < rx + rw {
            let idx = row_off + px as usize * 4;
            let luma = (u32::from(frame_rgba[idx]) * 77
                + u32::from(frame_rgba[idx + 1]) * 150
                + u32::from(frame_rgba[idx + 2]) * 29)
                >> 8;
            min_luma = min_luma.min(luma);
            max_luma = max_luma.max(luma);
            px += step_x;
        }
        py += step_y;
    }

    max_luma.saturating_sub(min_luma) >= 24
}
