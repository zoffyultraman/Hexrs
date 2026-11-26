//! Windows native WinRT OCR (`Windows.Media.Ocr`) and augment title text matching.

use hex_data::AugmentNameEntry;

const BAND_X_MIN: f32 = 0.12;
const BAND_X_MAX: f32 = 0.88;
const BAND_Y_MIN: f32 = 0.22;
const BAND_Y_MAX: f32 = 0.72;

/// Single recognized text line and its full-frame normalized center coordinates (`0.0..=1.0`).
#[derive(Debug, Clone, PartialEq)]
pub struct OcrLineHit {
    pub text: String,
    pub center_x_norm: f32,
    pub center_y_norm: f32,
}

#[derive(Debug, Clone)]
struct NormalizedAugmentEntry {
    augment_id: i32,
    norm_name: String,
    char_count: usize,
}

/// Maps WinRT OCR text hits across the 3 augment card columns to `[left, mid, right]` augment IDs.
#[derive(Debug, Clone, Default)]
pub struct AugmentTextMatcher {
    entries: Vec<NormalizedAugmentEntry>,
}

impl AugmentTextMatcher {
    /// Builds a matcher from `meta.db` augment display names.
    pub fn new(entries: Vec<AugmentNameEntry>) -> Self {
        let mut matcher = Self::default();
        matcher.update_entries(entries);
        matcher
    }

    /// Replaces the augment name dictionary (e.g., on `meta.db` hot-update).
    pub fn update_entries(&mut self, entries: Vec<AugmentNameEntry>) {
        self.entries = entries
            .into_iter()
            .filter_map(|entry| {
                if entry.augment_id <= 0 {
                    return None;
                }
                let norm_name = normalize_ocr_text(&entry.name_zh);
                let char_count = norm_name.chars().count();
                if char_count < 2 {
                    return None;
                }
                Some(NormalizedAugmentEntry {
                    augment_id: entry.augment_id,
                    norm_name,
                    char_count,
                })
            })
            .collect();
    }

    /// Matches recognized OCR lines within the augment card band into a 3-slot offer `[left, mid, right]`.
    ///
    /// Returns `Some([left, mid, right])` with `0` placeholders for an unmatched slot only when at
    /// least 2 columns resolve to distinct positive `augment_id`s.
    pub fn match_ocr_lines_to_offer(&self, lines: &[OcrLineHit]) -> Option<Vec<i32>> {
        if self.entries.is_empty() || lines.is_empty() {
            return None;
        }

        let mut col_candidates: [Vec<(i32, i32)>; 3] = [Vec::new(), Vec::new(), Vec::new()];

        for hit in lines {
            if !(BAND_X_MIN..=BAND_X_MAX).contains(&hit.center_x_norm)
                || !(BAND_Y_MIN..=BAND_Y_MAX).contains(&hit.center_y_norm)
            {
                continue;
            }

            let col = if hit.center_x_norm < 0.40 {
                0
            } else if hit.center_x_norm < 0.60 {
                1
            } else {
                2
            };

            let hit_norm = normalize_ocr_text(&hit.text);
            let hit_len = hit_norm.chars().count();
            if hit_len < 2 {
                continue;
            }

            if let Some((aug_id, score)) = self.score_line(&hit_norm, hit_len) {
                col_candidates[col].push((aug_id, score));
            }
        }

        for list in &mut col_candidates {
            list.sort_by(|a, b| b.1.cmp(&a.1));
        }

        let mut slots = vec![0_i32; 3];
        let mut slot_scores = [0_i32; 3];
        for (col, list) in col_candidates.iter().enumerate() {
            if let Some(&(aug_id, score)) = list.first() {
                slots[col] = aug_id;
                slot_scores[col] = score;
            }
        }

        // Enforce distinct augment IDs across columns, preserving the highest-confidence column.
        for i in 0..3 {
            if slots[i] <= 0 {
                continue;
            }
            for j in (i + 1)..3 {
                if slots[j] == slots[i] {
                    if slot_scores[i] >= slot_scores[j] {
                        slots[j] = col_candidates[j]
                            .iter()
                            .find(|&&(id, _)| id != slots[i] && !slots.contains(&id))
                            .map(|&(id, _)| id)
                            .unwrap_or(0);
                    } else {
                        let keep_id = slots[j];
                        slots[i] = col_candidates[i]
                            .iter()
                            .find(|&&(id, _)| id != keep_id && !slots.contains(&id))
                            .map(|&(id, _)| id)
                            .unwrap_or(0);
                    }
                }
            }
        }

        let matched_count = slots.iter().filter(|&&id| id > 0).count();
        if matched_count >= 2 {
            Some(slots)
        } else {
            None
        }
    }

    fn score_line(&self, hit_norm: &str, hit_len: usize) -> Option<(i32, i32)> {
        let mut best: Option<(i32, i32)> = None;

        for entry in &self.entries {
            let score = score_pair(hit_norm, hit_len, &entry.norm_name, entry.char_count);
            if score > 0 && best.map(|(_, b)| score > b).unwrap_or(true) {
                best = Some((entry.augment_id, score));
            }
        }

        best
    }
}

fn score_pair(hit_norm: &str, hit_len: usize, entry_norm: &str, entry_len: usize) -> i32 {
    if hit_norm == entry_norm {
        return 1000 + entry_len as i32 * 5;
    }

    if hit_norm.contains(entry_norm) {
        let extra = hit_len.saturating_sub(entry_len);
        // Reject long chat/description sentences that merely contain a short augment name.
        if hit_len <= 14 && entry_len * 2 >= hit_len && extra <= 4 {
            return 850 - (extra as i32 * 25) + entry_len as i32 * 3;
        }
        return 0;
    }

    if entry_norm.contains(hit_norm) && hit_len >= 2 && hit_len * 2 >= entry_len {
        let missing = entry_len.saturating_sub(hit_len);
        if missing <= 3 {
            return 720 - (missing as i32 * 30) + hit_len as i32 * 2;
        }
        return 0;
    }

    let max_len = hit_len.max(entry_len);
    let min_len = hit_len.min(entry_len);
    if min_len >= 3 && max_len.saturating_sub(min_len) <= 2 {
        let dist = char_levenshtein(hit_norm, entry_norm);
        let max_allowed = if max_len >= 6 { 2 } else { 1 };
        if dist <= max_allowed {
            return 680 - (dist as i32 * 90);
        }
    }

    0
}

fn normalize_ocr_text(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn char_levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let n = b_chars.len();
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0_usize; n + 1];

    for (i, &ca) in a_chars.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b_chars.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1)
                .min(curr[j] + 1)
                .min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[n]
}

#[cfg(target_os = "windows")]
mod win_ocr {
    use windows::core::h;
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

    use super::OcrLineHit;

    const CROP_X_START: f32 = 0.14;
    const CROP_X_END: f32 = 0.86;
    const CROP_Y_START: f32 = 0.25;
    const CROP_Y_END: f32 = 0.68;

    /// Native Windows Runtime OCR recognizer (`Windows.Media.Ocr.OcrEngine`) scoped to the
    /// current thread (must be initialized on the WGC capture callback thread).
    pub struct WinOcrRecognizer {
        engine: OcrEngine,
    }

    impl WinOcrRecognizer {
        /// Initializes WinRT MTA and creates a Chinese/user-profile `OcrEngine`.
        pub fn new() -> Option<Self> {
            // SAFETY: Safe to initialize WinRT apartment on the calling capture thread.
            let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };

            let engine = Language::CreateLanguage(h!("zh-Hans"))
                .and_then(|lang| OcrEngine::TryCreateFromLanguage(&lang))
                .or_else(|_| {
                    Language::CreateLanguage(h!("zh-CN"))
                        .and_then(|lang| OcrEngine::TryCreateFromLanguage(&lang))
                })
                .or_else(|_| OcrEngine::TryCreateFromUserProfileLanguages())
                .ok()?;

            Some(Self { engine })
        }

        /// Crops the augment card band (`x: 0.14..0.86, y: 0.25..0.68`), runs WinRT OCR, and
        /// maps recognized line bounding centers back to full-frame normalized coordinates.
        pub fn recognize_augment_region(
            &self,
            frame_rgba: &[u8],
            width: u32,
            height: u32,
            bgra_scratch: &mut Vec<u8>,
        ) -> Option<Vec<OcrLineHit>> {
            if width < 32 || height < 32 {
                return None;
            }
            let expected_len = (width as usize)
                .checked_mul(height as usize)?
                .checked_mul(4)?;
            if frame_rgba.len() < expected_len {
                return None;
            }

            let x0 = ((width as f32 * CROP_X_START).floor() as u32).min(width.saturating_sub(1));
            let x1 = ((width as f32 * CROP_X_END).ceil() as u32).clamp(x0 + 1, width);
            let y0 = ((height as f32 * CROP_Y_START).floor() as u32).min(height.saturating_sub(1));
            let y1 = ((height as f32 * CROP_Y_END).ceil() as u32).clamp(y0 + 1, height);

            let cw = x1 - x0;
            let ch = y1 - y0;
            if cw < 16 || ch < 16 {
                return None;
            }

            let crop_bytes = (cw as usize).checked_mul(ch as usize)?.checked_mul(4)?;
            bgra_scratch.clear();
            bgra_scratch.reserve(crop_bytes);

            let src_stride = width as usize * 4;
            for py in y0..y1 {
                let row_start = py as usize * src_stride + x0 as usize * 4;
                let row_end = row_start + cw as usize * 4;
                for px_bytes in frame_rgba[row_start..row_end].chunks_exact(4) {
                    bgra_scratch.push(px_bytes[2]);
                    bgra_scratch.push(px_bytes[1]);
                    bgra_scratch.push(px_bytes[0]);
                    bgra_scratch.push(255);
                }
            }

            let writer = DataWriter::new().ok()?;
            writer.WriteBytes(bgra_scratch).ok()?;
            let buffer = writer.DetachBuffer().ok()?;
            let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
                &buffer,
                BitmapPixelFormat::Bgra8,
                cw as i32,
                ch as i32,
                BitmapAlphaMode::Premultiplied,
            )
            .ok()?;

            let ocr_result = self.engine.RecognizeAsync(&bitmap).ok()?.join().ok()?;
            let lines = ocr_result.Lines().ok()?;

            let mut hits = Vec::new();
            for line in lines {
                let text = line.Text().ok()?.to_string();
                if text.trim().is_empty() {
                    continue;
                }
                let words = line.Words().ok()?;
                let mut min_x = f32::MAX;
                let mut min_y = f32::MAX;
                let mut max_x = f32::MIN;
                let mut max_y = f32::MIN;
                let mut any_word = false;
                for word in words {
                    if let Ok(rect) = word.BoundingRect() {
                        min_x = min_x.min(rect.X);
                        min_y = min_y.min(rect.Y);
                        max_x = max_x.max(rect.X + rect.Width);
                        max_y = max_y.max(rect.Y + rect.Height);
                        any_word = true;
                    }
                }
                if !any_word {
                    continue;
                }

                let rect_cx = (min_x + max_x) * 0.5;
                let rect_cy = (min_y + max_y) * 0.5;
                hits.push(OcrLineHit {
                    text,
                    center_x_norm: (x0 as f32 + rect_cx) / width as f32,
                    center_y_norm: (y0 as f32 + rect_cy) / height as f32,
                });
            }

            Some(hits)
        }
    }
}

#[cfg(target_os = "windows")]
pub use win_ocr::WinOcrRecognizer;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entries() -> Vec<AugmentNameEntry> {
        vec![
            AugmentNameEntry {
                augment_id: 101,
                name_zh: "急速之追求".to_owned(),
                rarity: "白银".to_owned(),
            },
            AugmentNameEntry {
                augment_id: 102,
                name_zh: "杀戮时间到了".to_owned(),
                rarity: "黄金".to_owned(),
            },
            AugmentNameEntry {
                augment_id: 103,
                name_zh: "拯救".to_owned(),
                rarity: "棱彩".to_owned(),
            },
            AugmentNameEntry {
                augment_id: 104,
                name_zh: "歌利亚巨人".to_owned(),
                rarity: "棱彩".to_owned(),
            },
        ]
    }

    #[test]
    fn matches_three_chinese_augment_titles_across_columns() {
        let matcher = AugmentTextMatcher::new(sample_entries());
        let hits = vec![
            OcrLineHit {
                text: "急 速 之 追 求".to_owned(),
                center_x_norm: 0.27,
                center_y_norm: 0.42,
            },
            OcrLineHit {
                text: "杀戮时间到了！".to_owned(),
                center_x_norm: 0.50,
                center_y_norm: 0.42,
            },
            OcrLineHit {
                text: "拯救".to_owned(),
                center_x_norm: 0.73,
                center_y_norm: 0.42,
            },
        ];

        assert_eq!(
            matcher.match_ocr_lines_to_offer(&hits),
            Some(vec![101, 102, 103])
        );
    }

    #[test]
    fn handles_partial_or_noisy_ocr_text_with_placeholder() {
        let matcher = AugmentTextMatcher::new(sample_entries());
        let hits = vec![
            OcrLineHit {
                text: "·急速之追".to_owned(),
                center_x_norm: 0.26,
                center_y_norm: 0.44,
            },
            OcrLineHit {
                text: "杀戮时间这了".to_owned(),
                center_x_norm: 0.51,
                center_y_norm: 0.44,
            },
        ];

        assert_eq!(
            matcher.match_ocr_lines_to_offer(&hits),
            Some(vec![101, 102, 0])
        );
    }

    #[test]
    fn rejects_random_game_hud_chat_and_killfeed_text() {
        let matcher = AugmentTextMatcher::new(sample_entries());
        let hits = vec![
            OcrLineHit {
                text: "敌方英雄已被击杀".to_owned(),
                center_x_norm: 0.28,
                center_y_norm: 0.35,
            },
            OcrLineHit {
                text: "快集合团战拯救一下残血队友".to_owned(),
                center_x_norm: 0.50,
                center_y_norm: 0.45,
            },
            OcrLineHit {
                text: "拯救".to_owned(),
                center_x_norm: 0.18,
                center_y_norm: 0.85,
            },
        ];

        assert_eq!(matcher.match_ocr_lines_to_offer(&hits), None);
    }
}
