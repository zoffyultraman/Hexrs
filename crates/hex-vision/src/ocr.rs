//! Windows native WinRT OCR (`Windows.Media.Ocr`) and augment title text matching.

use hex_core::AugmentOfferLayout;
use hex_data::AugmentNameEntry;

const BAND_X_MIN: f32 = 0.02;
const BAND_X_MAX: f32 = 0.98;
const BAND_Y_MIN: f32 = 0.22;
const BAND_Y_MAX: f32 = 0.72;
const ROW_CLUSTER_Y_TOL: f32 = 0.08;
const CARD_SEPARATION_X_MIN: f32 = 0.12;

/// Single recognized text line and its full-frame normalized center coordinates (`0.0..=1.0`).
#[derive(Debug, Clone, PartialEq)]
pub struct OcrLineHit {
    pub text: String,
    pub center_x_norm: f32,
    pub center_y_norm: f32,
}

/// Matched 3-slot augment offer IDs together with detected screen-space layout anchors.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrOfferMatch {
    pub candidates: Vec<i32>,
    pub layout: AugmentOfferLayout,
}

/// Diagnostic breakdown explaining why an OCR frame did not match a full augment offer.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrMatchDiagnostic {
    pub total_lines: usize,
    pub matched_hits: Vec<(i32, String, f32, f32)>,
    pub row_unmatched_texts: Vec<(String, f32, f32)>,
    pub reject_reason: &'static str,
}

/// Target identity metadata for a single loading-screen player card.
/// Transient in-memory only; never log `summoner_name` or target text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadingSlotTarget {
    pub name_zh: String,
    pub title_zh: String,
    pub alias: String,
    pub summoner_name: String,
}

/// Target rosters for top (`team_one`) and bottom (`team_two`) loading-screen rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadingAlignmentTargets {
    pub game_id: u64,
    pub team_one: Vec<LoadingSlotTarget>,
    pub team_two: Vec<LoadingSlotTarget>,
}

#[derive(Debug, Clone)]
struct NormalizedAugmentEntry {
    augment_id: i32,
    name_zh: String,
    norm_name: String,
    char_count: usize,
}

#[derive(Debug, Clone, Copy)]
struct MatchedHit {
    aug_id: i32,
    score: i32,
    cx: f32,
    cy: f32,
}

/// Maps WinRT OCR text hits across the 3 augment card slots to `[left, mid, right]` augment IDs.
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
                if char_count == 0 || (char_count == 1 && norm_name.is_ascii()) {
                    return None;
                }
                Some(NormalizedAugmentEntry {
                    augment_id: entry.augment_id,
                    name_zh: entry.name_zh,
                    norm_name,
                    char_count,
                })
            })
            .collect();
    }

    /// Matches recognized OCR lines aligned on a horizontal card title row into `[left, mid, right]`.
    pub fn match_ocr_lines_to_offer(&self, lines: &[OcrLineHit]) -> Option<Vec<i32>> {
        self.match_ocr_lines_to_offer_with_layout(lines)
            .map(|matched| matched.candidates)
    }

    /// Matches recognized OCR lines aligned on a horizontal card title row into `[left, mid, right]`
    /// and computes normalized screen-space card anchor coordinates (`AugmentOfferLayout`).
    ///
    /// Works across both centered 16:9 and left-shifted 21:9/ultrawide layouts without hardcoded
    /// column boundaries. Returns `Some(...)` with `0` placeholders only when at least 2 distinct
    /// horizontally separated cards align on the same title row.
    pub fn match_ocr_lines_to_offer_with_layout(
        &self,
        lines: &[OcrLineHit],
    ) -> Option<OcrOfferMatch> {
        if self.entries.is_empty() || lines.is_empty() {
            return None;
        }

        let in_band: Vec<&OcrLineHit> = lines
            .iter()
            .filter(|hit| {
                (BAND_X_MIN..=BAND_X_MAX).contains(&hit.center_x_norm)
                    && (BAND_Y_MIN..=BAND_Y_MAX).contains(&hit.center_y_norm)
            })
            .collect();

        let mut matched = Vec::new();
        for &hit in &in_band {
            let hit_norm = normalize_ocr_text(&hit.text);
            let hit_len = hit_norm.chars().count();
            if hit_len == 0 || (hit_len == 1 && hit_norm.is_ascii()) {
                continue;
            }

            if let Some((aug_id, score)) = self.score_line(&hit_norm, hit_len) {
                matched.push(MatchedHit {
                    aug_id,
                    score,
                    cx: hit.center_x_norm,
                    cy: hit.center_y_norm,
                });
            }
        }

        // Synthesize 2-line wrapped augment titles (e.g., "升级：" + "米凯尔的祝福").
        for &a in &in_band {
            for &b in &in_band {
                let dy = b.center_y_norm - a.center_y_norm;
                if !(0.005..=0.055).contains(&dy) {
                    continue;
                }
                if (a.center_x_norm - b.center_x_norm).abs() > 0.06 {
                    continue;
                }

                let merged_raw = format!("{}{}", a.text, b.text);
                let merged_norm = normalize_ocr_text(&merged_raw);
                let merged_len = merged_norm.chars().count();
                if !(4..=16).contains(&merged_len) {
                    continue;
                }

                if let Some((aug_id, score)) = self.score_line(&merged_norm, merged_len) {
                    matched.push(MatchedHit {
                        aug_id,
                        score,
                        cx: (a.center_x_norm + b.center_x_norm) * 0.5,
                        cy: (a.center_y_norm + b.center_y_norm) * 0.5,
                    });
                }
            }
        }

        if matched.len() < 2 {
            return None;
        }

        let mut best_cluster: Option<(usize, i32, Vec<MatchedHit>)> = None;

        for anchor in &matched {
            let mut row_hits: Vec<MatchedHit> = matched
                .iter()
                .copied()
                .filter(|h| (h.cy - anchor.cy).abs() <= ROW_CLUSTER_Y_TOL)
                .collect();

            row_hits.sort_by(|a, b| b.score.cmp(&a.score));

            let mut selected: Vec<MatchedHit> = Vec::with_capacity(4);
            for candidate in row_hits {
                let too_close = selected
                    .iter()
                    .any(|s| (s.cx - candidate.cx).abs() < CARD_SEPARATION_X_MIN);
                let duplicate_id = selected.iter().any(|s| s.aug_id == candidate.aug_id);
                if !too_close && !duplicate_id {
                    selected.push(candidate);
                }
            }

            if selected.len() < 2 {
                continue;
            }

            // Cap to the top 4 highest-scoring hits before spatial ordering (supports 3-card and 4-card offers).
            selected.truncate(4);
            let total_score: i32 = selected.iter().map(|s| s.score).sum();
            let count = selected.len();

            let is_better = best_cluster
                .as_ref()
                .map(|(best_cnt, best_score, _)| {
                    count > *best_cnt || (count == *best_cnt && total_score > *best_score)
                })
                .unwrap_or(true);

            if is_better {
                best_cluster = Some((count, total_score, selected));
            }
        }

        let (_, _, mut winners) = best_cluster?;
        winners.sort_by(|a, b| a.cx.total_cmp(&b.cx));

        if winners.len() >= 4 {
            let (w0, w1, w2, w3) = (winners[0], winners[1], winners[2], winners[3]);
            Some(OcrOfferMatch {
                candidates: vec![w0.aug_id, w1.aug_id, w2.aug_id, w3.aug_id],
                layout: AugmentOfferLayout {
                    slot_centers_x: vec![w0.cx, w1.cx, w2.cx, w3.cx],
                    title_y: (w0.cy + w1.cy + w2.cy + w3.cy) / 4.0,
                },
            })
        } else if winners.len() == 3 {
            let (w0, w1, w2) = (winners[0], winners[1], winners[2]);
            let gap01 = w1.cx - w0.cx;
            let gap12 = w2.cx - w1.cx;
            let title_y = (w0.cy + w1.cy + w2.cy) / 3.0;
            if gap01 > gap12 * 1.55 && gap12 >= CARD_SEPARATION_X_MIN {
                Some(OcrOfferMatch {
                    candidates: vec![w0.aug_id, 0, w1.aug_id, w2.aug_id],
                    layout: AugmentOfferLayout {
                        slot_centers_x: vec![w0.cx, (w0.cx + w1.cx) * 0.5, w1.cx, w2.cx],
                        title_y,
                    },
                })
            } else if gap12 > gap01 * 1.55 && gap01 >= CARD_SEPARATION_X_MIN {
                Some(OcrOfferMatch {
                    candidates: vec![w0.aug_id, w1.aug_id, 0, w2.aug_id],
                    layout: AugmentOfferLayout {
                        slot_centers_x: vec![w0.cx, w1.cx, (w1.cx + w2.cx) * 0.5, w2.cx],
                        title_y,
                    },
                })
            } else {
                Some(OcrOfferMatch {
                    candidates: vec![w0.aug_id, w1.aug_id, w2.aug_id],
                    layout: AugmentOfferLayout {
                        slot_centers_x: vec![w0.cx, w1.cx, w2.cx],
                        title_y,
                    },
                })
            }
        } else {
            let h0 = winners[0];
            let h1 = winners[1];
            let gap = h1.cx - h0.cx;
            let title_y = (h0.cy + h1.cy) * 0.5;
            if gap > 0.36 {
                Some(OcrOfferMatch {
                    candidates: vec![h0.aug_id, 0, h1.aug_id],
                    layout: AugmentOfferLayout {
                        slot_centers_x: vec![h0.cx, (h0.cx + h1.cx) * 0.5, h1.cx],
                        title_y,
                    },
                })
            } else if (h0.cx + h1.cx) * 0.5 < 0.48 {
                Some(OcrOfferMatch {
                    candidates: vec![h0.aug_id, h1.aug_id, 0],
                    layout: AugmentOfferLayout {
                        slot_centers_x: vec![h0.cx, h1.cx, (h1.cx + gap).min(0.95)],
                        title_y,
                    },
                })
            } else {
                Some(OcrOfferMatch {
                    candidates: vec![0, h0.aug_id, h1.aug_id],
                    layout: AugmentOfferLayout {
                        slot_centers_x: vec![(h0.cx - gap).max(0.05), h0.cx, h1.cx],
                        title_y,
                    },
                })
            }
        }
    }

    /// Produces a privacy-safe diagnostic summary when `match_ocr_lines_to_offer_with_layout` returns `None`.
    pub fn diagnose_ocr_lines(&self, lines: &[OcrLineHit]) -> OcrMatchDiagnostic {
        let total_lines = lines.len();
        if lines.is_empty() {
            return OcrMatchDiagnostic {
                total_lines,
                matched_hits: Vec::new(),
                row_unmatched_texts: Vec::new(),
                reject_reason: "no_ocr_lines",
            };
        }

        let in_band: Vec<&OcrLineHit> = lines
            .iter()
            .filter(|hit| {
                (BAND_X_MIN..=BAND_X_MAX).contains(&hit.center_x_norm)
                    && (BAND_Y_MIN..=BAND_Y_MAX).contains(&hit.center_y_norm)
            })
            .collect();

        let mut matched_indices = vec![false; in_band.len()];
        let mut matched_hits: Vec<(i32, String, f32, f32)> = Vec::new();

        for (idx, &hit) in in_band.iter().enumerate() {
            let hit_norm = normalize_ocr_text(&hit.text);
            let hit_len = hit_norm.chars().count();
            if hit_len == 0 || (hit_len == 1 && hit_norm.is_ascii()) {
                continue;
            }
            if let Some((aug_id, _score)) = self.score_line(&hit_norm, hit_len) {
                matched_indices[idx] = true;
                let name = self.lookup_name(aug_id);
                matched_hits.push((aug_id, name, hit.center_x_norm, hit.center_y_norm));
            }
        }

        for (i, &a) in in_band.iter().enumerate() {
            for (j, &b) in in_band.iter().enumerate() {
                if matched_indices[i] && matched_indices[j] {
                    continue;
                }
                let dy = b.center_y_norm - a.center_y_norm;
                if !(0.005..=0.055).contains(&dy) {
                    continue;
                }
                if (a.center_x_norm - b.center_x_norm).abs() > 0.06 {
                    continue;
                }
                let merged_raw = format!("{}{}", a.text, b.text);
                let merged_norm = normalize_ocr_text(&merged_raw);
                let merged_len = merged_norm.chars().count();
                if !(4..=16).contains(&merged_len) {
                    continue;
                }
                if let Some((aug_id, _score)) = self.score_line(&merged_norm, merged_len) {
                    matched_indices[i] = true;
                    matched_indices[j] = true;
                    let name = self.lookup_name(aug_id);
                    matched_hits.push((
                        aug_id,
                        name,
                        (a.center_x_norm + b.center_x_norm) * 0.5,
                        (a.center_y_norm + b.center_y_norm) * 0.5,
                    ));
                }
            }
        }

        let reject_reason = if matched_hits.is_empty() {
            "no_dictionary_match"
        } else if matched_hits.len() == 1 {
            "single_hit_only"
        } else {
            "row_alignment_or_spacing_rejected"
        };

        let mut row_unmatched_texts = Vec::new();
        if let Some(&(_, _, _, ref_y)) = matched_hits.first() {
            for (idx, &hit) in in_band.iter().enumerate() {
                if matched_indices[idx] {
                    continue;
                }
                if (hit.center_y_norm - ref_y).abs() <= 0.06 {
                    let trimmed = hit.text.trim();
                    if !trimmed.is_empty() {
                        let truncated: String = trimmed.chars().take(24).collect();
                        row_unmatched_texts.push((
                            truncated,
                            hit.center_x_norm,
                            hit.center_y_norm,
                        ));
                        if row_unmatched_texts.len() >= 6 {
                            break;
                        }
                    }
                }
            }
        }

        OcrMatchDiagnostic {
            total_lines,
            matched_hits,
            row_unmatched_texts,
            reject_reason,
        }
    }

    fn lookup_name(&self, aug_id: i32) -> String {
        self.entries
            .iter()
            .find(|e| e.augment_id == aug_id)
            .map(|e| e.name_zh.clone())
            .unwrap_or_default()
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
    if entry_len == 1 {
        return if hit_norm == entry_norm { 960 } else { 0 };
    }

    if hit_norm == entry_norm {
        return 1000 + entry_len as i32 * 5;
    }

    // Strict guard for 2-char augment names ("拯救", "灵巧") with 1 stray ASCII glyph tolerance.
    if entry_len == 2 {
        if hit_len == 3 && hit_norm.contains(entry_norm) {
            let leftover = hit_norm.replacen(entry_norm, "", 1);
            let mut chars = leftover.chars();
            if let (Some(ch), None) = (chars.next(), chars.next()) {
                if ch.is_ascii_alphanumeric() {
                    return 760;
                }
            }
        }
        return 0;
    }

    if hit_norm.contains(entry_norm) {
        let extra = hit_len.saturating_sub(entry_len);
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

struct NormalizedLoadingTarget {
    name_zh: String,
    name_zh_chars: Vec<char>,
    title_zh: String,
    title_zh_chars: Vec<char>,
    alias: String,
    summoner: String,
    summoner_chars: Vec<char>,
}

impl NormalizedLoadingTarget {
    fn from_target(t: &LoadingSlotTarget) -> Self {
        let name_zh = normalize_ocr_text(&t.name_zh);
        let name_zh_chars = name_zh.chars().collect();
        let title_zh = normalize_ocr_text(&t.title_zh);
        let title_zh_chars = title_zh.chars().collect();
        let alias = normalize_ocr_text(&t.alias);
        let summoner = normalize_ocr_text(&t.summoner_name);
        let summoner_chars = summoner.chars().collect();
        Self {
            name_zh,
            name_zh_chars,
            title_zh,
            title_zh_chars,
            alias,
            summoner,
            summoner_chars,
        }
    }
}

fn longest_common_substring_chars(a: &[char], b: &[char]) -> usize {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let mut prev = vec![0usize; b.len() + 1];
    let mut curr = vec![0usize; b.len() + 1];
    let mut best = 0usize;
    for &ca in a {
        for (j, &cb) in b.iter().enumerate() {
            if ca == cb {
                let v = prev[j] + 1;
                curr[j + 1] = v;
                if v > best {
                    best = v;
                }
            } else {
                curr[j + 1] = 0;
            }
        }
        std::mem::swap(&mut prev, &mut curr);
        curr.fill(0);
    }
    best
}

fn count_shared_non_ascii_chars(hay: &[char], needle: &[char]) -> usize {
    let mut seen: Vec<char> = Vec::new();
    let mut shared = 0usize;
    for &c in needle {
        if c.is_ascii() || seen.contains(&c) {
            continue;
        }
        seen.push(c);
        if hay.contains(&c) {
            shared += 1;
        }
    }
    shared
}

fn score_loading_summoner(
    col_lines: &[(String, Vec<char>)],
    col_joined: &str,
    needle: &str,
    needle_chars: &[char],
) -> i32 {
    let n_len = needle_chars.len();
    if n_len == 0 {
        return 0;
    }
    if n_len == 1 {
        return if !needle_chars[0].is_ascii()
            && col_lines.iter().any(|(l, _)| l.as_str() == needle)
        {
            900
        } else {
            0
        };
    }

    let mut best = 0i32;
    let has_non_ascii = needle_chars.iter().any(|c| !c.is_ascii());
    for (line, line_chars) in col_lines {
        if line.as_str() == needle {
            best = best.max(1200 + (n_len as i32) * 10);
        } else if line.contains(needle) {
            best = best.max(950 + (n_len as i32) * 10);
        } else if line_chars.len() >= 2
            && line_chars.len() * 2 >= n_len
            && needle.contains(line.as_str())
        {
            best = best.max(520 + (line_chars.len() as i32) * 10);
        } else if n_len >= 3 {
            let lcs = longest_common_substring_chars(line_chars, needle_chars);
            if has_non_ascii {
                let shared = count_shared_non_ascii_chars(line_chars, needle_chars);
                if (lcs >= 2 && lcs * 2 >= n_len) || (shared >= 2 && shared * 2 >= n_len) {
                    best = best.max(180 + (lcs as i32) * 60 + (shared as i32) * 35);
                }
            } else if n_len >= 4 && lcs >= 3 && lcs * 2 >= n_len {
                best = best.max(180 + (lcs as i32) * 60);
            }
        }
    }

    if best < 900 && col_joined.contains(needle) {
        best = best.max(900 + (n_len as i32) * 10);
    }
    best
}

fn score_loading_name_zh(
    col_lines: &[(String, Vec<char>)],
    col_joined: &str,
    needle: &str,
    needle_chars: &[char],
) -> i32 {
    let n_len = needle_chars.len();
    if n_len == 0 {
        return 0;
    }
    if n_len == 1 {
        if !needle_chars[0].is_ascii() {
            if col_lines.iter().any(|(l, _)| l.as_str() == needle) {
                return 720;
            }
            if col_lines.iter().any(|(l, _)| l.ends_with(needle)) {
                return 360;
            }
        }
        return 0;
    }

    let mut best = 0i32;
    for (line, line_chars) in col_lines {
        if line.as_str() == needle {
            best = best.max(850 + (n_len as i32) * 10);
        } else if line.contains(needle) {
            let suffix_bonus = if line.ends_with(needle) { 80 } else { 0 };
            best = best.max(700 + (n_len as i32) * 10 + suffix_bonus);
        } else if n_len >= 3 {
            let lcs = longest_common_substring_chars(line_chars, needle_chars);
            let shared = count_shared_non_ascii_chars(line_chars, needle_chars);
            if shared >= 2 && shared * 2 >= n_len {
                best = best.max(220 + (lcs as i32) * 80 + (shared as i32) * 40);
            }
        }
    }

    if best < 680 && col_joined.contains(needle) {
        best = best.max(680 + (n_len as i32) * 10);
    }
    best
}

fn score_loading_title_zh(
    col_lines: &[(String, Vec<char>)],
    col_joined: &str,
    needle: &str,
    needle_chars: &[char],
) -> i32 {
    let n_len = needle_chars.len();
    if n_len < 2 {
        return 0;
    }

    let mut best = 0i32;
    let min_shared = if n_len >= 4 { 3 } else { 2 };
    for (line, line_chars) in col_lines {
        if line.as_str() == needle {
            best = best.max(800 + (n_len as i32) * 10);
        } else if line.contains(needle) {
            best = best.max(650 + (n_len as i32) * 10);
        } else if n_len >= 3 {
            let lcs = longest_common_substring_chars(line_chars, needle_chars);
            let shared = count_shared_non_ascii_chars(line_chars, needle_chars);
            if shared >= min_shared && (n_len >= 4 || lcs >= 2) {
                best = best.max(200 + (lcs as i32) * 70 + (shared as i32) * 35);
            }
        }
    }

    if best < 630 && col_joined.contains(needle) {
        best = best.max(630 + (n_len as i32) * 10);
    }
    best
}

fn score_loading_alias(col_lines: &[(String, Vec<char>)], col_joined: &str, needle: &str) -> i32 {
    let n_len = needle.chars().count();
    if n_len < 2 {
        return 0;
    }

    let mut best = 0i32;
    for (line, _) in col_lines {
        if line.as_str() == needle {
            best = best.max(700 + (n_len as i32) * 5);
        } else if n_len >= 4 && line.contains(needle) {
            best = best.max(500 + (n_len as i32) * 5);
        }
    }

    if best < 480 && n_len >= 4 && col_joined.contains(needle) {
        best = best.max(480 + (n_len as i32) * 5);
    }
    best
}

fn score_loading_column_against_target(
    col_lines: &[(String, Vec<char>)],
    col_joined: &str,
    target: &NormalizedLoadingTarget,
) -> i32 {
    if col_lines.is_empty() {
        return 0;
    }
    let summoner_score =
        score_loading_summoner(col_lines, col_joined, &target.summoner, &target.summoner_chars);
    let name_score =
        score_loading_name_zh(col_lines, col_joined, &target.name_zh, &target.name_zh_chars);
    let title_score =
        score_loading_title_zh(col_lines, col_joined, &target.title_zh, &target.title_zh_chars);
    let alias_score = score_loading_alias(col_lines, col_joined, &target.alias);
    let champ_score = name_score.max(title_score).max(alias_score);
    summoner_score + champ_score
}

fn generate_permutations(n: usize) -> Vec<Vec<usize>> {
    fn backtrack(start: usize, current: &mut [usize], out: &mut Vec<Vec<usize>>) {
        if start == current.len() {
            out.push(current.to_vec());
            return;
        }
        for i in start..current.len() {
            current.swap(start, i);
            backtrack(start + 1, current, out);
            current.swap(start, i);
        }
    }

    let mut out = Vec::new();
    let mut current: Vec<usize> = (0..n).collect();
    backtrack(0, &mut current, &mut out);
    out
}

/// Aligns a single 1..=5 player loading-screen row from WinRT OCR line hits using calibrated
/// card X-geometry and optimal 1-to-1 permutation matching (`n! <= 120`).
///
/// Returns `Some((order, matched_cols, total_score))` where `order[visual_col]` is the index
/// into `targets` occupying visual column `visual_col`, or `None` on blank/low-confidence frames.
pub fn align_loading_row_from_ocr(
    lines: &[OcrLineHit],
    targets: &[LoadingSlotTarget],
    frame_w: u32,
    frame_h: u32,
) -> Option<(Vec<usize>, usize, i32)> {
    let n = targets.len();
    if n == 0 || n > 5 || lines.is_empty() {
        return None;
    }

    let w = frame_w.max(320) as f32;
    let h = frame_h.max(200) as f32;
    let raw_card_w = h * 0.2328;
    let raw_gap = h * 0.0411;
    let raw_total = raw_card_w * 5.0 + raw_gap * 4.0;
    let scale = if raw_total > w * 0.90 {
        (w * 0.90) / raw_total
    } else {
        1.0
    };
    let card_w = raw_card_w * scale;
    let card_gap = raw_gap * scale;
    let row_w = card_w * 5.0 + card_gap * 4.0;
    let row_left = ((w - row_w) * 0.5 - w * 0.0015).max(0.0);
    let half_pitch_norm = (card_w + card_gap) * 0.5 / w;

    let mut col_lines: Vec<Vec<(String, Vec<char>)>> = vec![Vec::new(); n];
    for hit in lines {
        let norm = normalize_ocr_text(&hit.text);
        if norm.is_empty() {
            continue;
        }
        let mut best_col: Option<(usize, f32)> = None;
        for col in 0..n {
            let col_left = (row_left + col as f32 * (card_w + card_gap)) / w;
            let col_center = col_left + (card_w * 0.5) / w;
            let dist = (hit.center_x_norm - col_center).abs();
            if dist <= half_pitch_norm && best_col.map(|(_, d)| dist < d).unwrap_or(true) {
                best_col = Some((col, dist));
            }
        }
        if let Some((col, _)) = best_col {
            let chars: Vec<char> = norm.chars().collect();
            col_lines[col].push((norm, chars));
        }
    }

    let col_joined: Vec<String> = col_lines
        .iter()
        .map(|items| items.iter().map(|(s, _)| s.as_str()).collect::<String>())
        .collect();
    let norm_targets: Vec<NormalizedLoadingTarget> = targets
        .iter()
        .map(NormalizedLoadingTarget::from_target)
        .collect();

    let mut scores = vec![vec![0i32; n]; n];
    for col in 0..n {
        for (target_idx, target) in norm_targets.iter().enumerate() {
            scores[col][target_idx] =
                score_loading_column_against_target(&col_lines[col], &col_joined[col], target);
        }
    }

    let mut best: Option<((i32, usize, usize, usize), Vec<usize>)> = None;
    for perm in generate_permutations(n) {
        let total_score: i32 = (0..n).map(|col| scores[col][perm[col]]).sum();
        let matched_cols: usize = (0..n).filter(|&col| scores[col][perm[col]] > 0).count();
        let identity_hits: usize = (0..n).filter(|&col| perm[col] == col).count();
        let mut in_order_pairs = 0usize;
        for i in 0..n {
            for j in (i + 1)..n {
                if perm[i] < perm[j] {
                    in_order_pairs += 1;
                }
            }
        }
        let key = (total_score, matched_cols, identity_hits, in_order_pairs);
        if best.as_ref().map(|(bk, _)| key > *bk).unwrap_or(true) {
            best = Some((key, perm));
        }
    }

    let ((best_score, best_matched, _, _), best_perm) = best?;
    let min_matched = n.min(2);
    let min_score = (min_matched as i32) * 250;
    if best_matched < min_matched || best_score < min_score {
        return None;
    }

    Some((best_perm, best_matched, best_score))
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

    const CROP_X_START: f32 = 0.0;
    const CROP_X_END: f32 = 1.0;
    const CROP_Y_START: f32 = 0.25;
    const CROP_Y_END: f32 = 0.68;

    /// Native Windows Runtime OCR recognizer (`Windows.Media.Ocr.OcrEngine`) scoped to the
    /// current thread (must be initialized on the WGC capture callback thread).
    pub struct WinOcrRecognizer {
        engine: OcrEngine,
        language_tag: String,
        available_languages: Vec<String>,
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

            let language_tag = engine
                .RecognizerLanguage()
                .and_then(|l| l.LanguageTag())
                .map(|s| s.to_string())
                .unwrap_or_else(|_| "unknown".to_owned());

            let available_languages = OcrEngine::AvailableRecognizerLanguages()
                .map(|langs| {
                    langs
                        .into_iter()
                        .filter_map(|l| l.LanguageTag().ok().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            Some(Self {
                engine,
                language_tag,
                available_languages,
            })
        }

        /// Returns the BCP-47 language tag actually active on the underlying `OcrEngine`.
        pub fn language_tag(&self) -> &str {
            &self.language_tag
        }

        /// Returns the BCP-47 language tags installed for Windows OCR on this machine.
        pub fn available_languages(&self) -> &[String] {
            &self.available_languages
        }

        /// Returns `true` when the active OCR engine language is Chinese (`zh-*`).
        pub fn is_chinese(&self) -> bool {
            self.language_tag.to_ascii_lowercase().starts_with("zh")
        }

        /// Crops a horizontal band (`x: 0.0..1.0, y: y_start..y_end`), runs WinRT OCR, and
        /// maps recognized line bounding centers back to full-frame normalized `[0..1]` coordinates.
        pub fn recognize_band(
            &self,
            frame_rgba: &[u8],
            width: u32,
            height: u32,
            y_start: f32,
            y_end: f32,
            bgra_scratch: &mut Vec<u8>,
        ) -> Option<Vec<OcrLineHit>> {
            if width < 32 || height < 32 {
                return None;
            }
            let y_start = y_start.clamp(0.0, 1.0);
            let y_end = y_end.clamp(0.0, 1.0);
            if y_end <= y_start {
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
            let y0 = ((height as f32 * y_start).floor() as u32).min(height.saturating_sub(1));
            let y1 = ((height as f32 * y_end).ceil() as u32).clamp(y0 + 1, height);

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

            let ocr_result = self.engine.RecognizeAsync(&bitmap).ok()?.get().ok()?;
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

        /// Crops the augment card band (`x: 0.0..1.0, y: 0.25..0.68`), runs WinRT OCR, and
        /// maps recognized line bounding centers back to full-frame normalized coordinates.
        pub fn recognize_augment_region(
            &self,
            frame_rgba: &[u8],
            width: u32,
            height: u32,
            bgra_scratch: &mut Vec<u8>,
        ) -> Option<Vec<OcrLineHit>> {
            self.recognize_band(
                frame_rgba,
                width,
                height,
                CROP_Y_START,
                CROP_Y_END,
                bgra_scratch,
            )
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
                name_zh: "灵巧".to_owned(),
                rarity: "白银".to_owned(),
            },
        ]
    }

    #[test]
    fn matches_left_shifted_ultrawide_layout_and_centered_16_9_layout() {
        let matcher = AugmentTextMatcher::new(sample_entries());

        // Left-shifted 21:9 screenshot coordinates (x ≈ 0.11, 0.36, 0.60).
        let ultrawide_hits = vec![
            OcrLineHit {
                text: "急 速 之 追 求".to_owned(),
                center_x_norm: 0.11,
                center_y_norm: 0.42,
            },
            OcrLineHit {
                text: "杀戮时间到了！".to_owned(),
                center_x_norm: 0.36,
                center_y_norm: 0.43,
            },
            OcrLineHit {
                text: "拯救".to_owned(),
                center_x_norm: 0.60,
                center_y_norm: 0.42,
            },
        ];
        assert_eq!(
            matcher.match_ocr_lines_to_offer(&ultrawide_hits),
            Some(vec![101, 102, 103])
        );

        // Centered 16:9 coordinates (x ≈ 0.32, 0.50, 0.68).
        let centered_hits = vec![
            OcrLineHit {
                text: "急速之追求".to_owned(),
                center_x_norm: 0.32,
                center_y_norm: 0.40,
            },
            OcrLineHit {
                text: "杀戮时间到了".to_owned(),
                center_x_norm: 0.50,
                center_y_norm: 0.40,
            },
            OcrLineHit {
                text: "拯救".to_owned(),
                center_x_norm: 0.68,
                center_y_norm: 0.40,
            },
        ];
        assert_eq!(
            matcher.match_ocr_lines_to_offer(&centered_hits),
            Some(vec![101, 102, 103])
        );
    }

    #[test]
    fn handles_partial_noisy_text_and_two_hit_middle_gap() {
        let matcher = AugmentTextMatcher::new(sample_entries());

        // 2-hit middle gap: left & right separated by > 0.36 -> [101, 0, 103].
        let middle_gap_hits = vec![
            OcrLineHit {
                text: "·急速之追".to_owned(),
                center_x_norm: 0.11,
                center_y_norm: 0.44,
            },
            OcrLineHit {
                text: "拯救".to_owned(),
                center_x_norm: 0.60,
                center_y_norm: 0.44,
            },
        ];
        assert_eq!(
            matcher.match_ocr_lines_to_offer(&middle_gap_hits),
            Some(vec![101, 0, 103])
        );
    }

    #[test]
    fn rejects_two_char_substring_noise_and_random_hud_text() {
        let matcher = AugmentTextMatcher::new(sample_entries());
        let hits = vec![
            OcrLineHit {
                text: "敌方英雄已被击杀".to_owned(),
                center_x_norm: 0.28,
                center_y_norm: 0.35,
            },
            OcrLineHit {
                text: "拯救一下".to_owned(),
                center_x_norm: 0.35,
                center_y_norm: 0.45,
            },
            OcrLineHit {
                text: "灵巧走位".to_owned(),
                center_x_norm: 0.55,
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

    #[test]
    fn computes_layout_anchors_for_three_hit_and_partial_two_hit_offers() {
        let matcher = AugmentTextMatcher::new(sample_entries());

        let three_hits = vec![
            OcrLineHit {
                text: "急速之追求".to_owned(),
                center_x_norm: 0.11,
                center_y_norm: 0.41,
            },
            OcrLineHit {
                text: "杀戮时间到了".to_owned(),
                center_x_norm: 0.36,
                center_y_norm: 0.43,
            },
            OcrLineHit {
                text: "拯救".to_owned(),
                center_x_norm: 0.60,
                center_y_norm: 0.42,
            },
        ];
        let full_match = matcher
            .match_ocr_lines_to_offer_with_layout(&three_hits)
            .expect("3-card match should succeed");
        assert_eq!(full_match.candidates, vec![101, 102, 103]);
        assert!((full_match.layout.slot_centers_x[0] - 0.11).abs() < 1e-4);
        assert!((full_match.layout.slot_centers_x[1] - 0.36).abs() < 1e-4);
        assert!((full_match.layout.slot_centers_x[2] - 0.60).abs() < 1e-4);
        assert!((full_match.layout.title_y - 0.42).abs() < 1e-4);

        let two_hits_left = vec![
            OcrLineHit {
                text: "急速之追求".to_owned(),
                center_x_norm: 0.12,
                center_y_norm: 0.44,
            },
            OcrLineHit {
                text: "杀戮时间到了".to_owned(),
                center_x_norm: 0.36,
                center_y_norm: 0.44,
            },
        ];
        let left_pair = matcher
            .match_ocr_lines_to_offer_with_layout(&two_hits_left)
            .expect("left-pair match should extrapolate right center");
        assert_eq!(left_pair.candidates, vec![101, 102, 0]);
        assert!((left_pair.layout.slot_centers_x[2] - 0.60).abs() < 1e-4);
        assert!((left_pair.layout.title_y - 0.44).abs() < 1e-4);
    }

    #[test]
    fn matches_four_card_jhin_band_and_missing_middle_slots() {
        let matcher = AugmentTextMatcher::new(sample_entries());

        let four_card_hits = vec![
            OcrLineHit {
                text: "急速之追求".to_owned(),
                center_x_norm: 0.11,
                center_y_norm: 0.42,
            },
            OcrLineHit {
                text: "杀戮时间到了".to_owned(),
                center_x_norm: 0.365,
                center_y_norm: 0.42,
            },
            OcrLineHit {
                text: "拯救".to_owned(),
                center_x_norm: 0.62,
                center_y_norm: 0.42,
            },
            OcrLineHit {
                text: "灵巧".to_owned(),
                center_x_norm: 0.875,
                center_y_norm: 0.42,
            },
        ];
        let full_four = matcher
            .match_ocr_lines_to_offer_with_layout(&four_card_hits)
            .expect("4-card match should succeed");
        assert_eq!(full_four.candidates, vec![101, 102, 103, 104]);
        assert_eq!(full_four.layout.slot_centers_x.len(), 4);
        assert!((full_four.layout.slot_centers_x[0] - 0.11).abs() < 1e-4);
        assert!((full_four.layout.slot_centers_x[3] - 0.875).abs() < 1e-4);

        // Missing slot 1 in a 4-card offer -> [101, 0, 103, 104] with interpolated cx[1].
        let missing_slot1 = vec![
            OcrLineHit {
                text: "急速之追求".to_owned(),
                center_x_norm: 0.11,
                center_y_norm: 0.42,
            },
            OcrLineHit {
                text: "拯救".to_owned(),
                center_x_norm: 0.62,
                center_y_norm: 0.42,
            },
            OcrLineHit {
                text: "灵巧".to_owned(),
                center_x_norm: 0.875,
                center_y_norm: 0.42,
            },
        ];
        let m1 = matcher
            .match_ocr_lines_to_offer_with_layout(&missing_slot1)
            .expect("4-card missing slot 1 should interpolate");
        assert_eq!(m1.candidates, vec![101, 0, 103, 104]);
        assert!((m1.layout.slot_centers_x[1] - 0.365).abs() < 1e-4);

        // Missing slot 2 in a 4-card offer -> [101, 102, 0, 104] with interpolated cx[2].
        let missing_slot2 = vec![
            OcrLineHit {
                text: "急速之追求".to_owned(),
                center_x_norm: 0.11,
                center_y_norm: 0.42,
            },
            OcrLineHit {
                text: "杀戮时间到了".to_owned(),
                center_x_norm: 0.365,
                center_y_norm: 0.42,
            },
            OcrLineHit {
                text: "灵巧".to_owned(),
                center_x_norm: 0.875,
                center_y_norm: 0.42,
            },
        ];
        let m2 = matcher
            .match_ocr_lines_to_offer_with_layout(&missing_slot2)
            .expect("4-card missing slot 2 should interpolate");
        assert_eq!(m2.candidates, vec![101, 102, 0, 104]);
        assert!((m2.layout.slot_centers_x[2] - 0.62).abs() < 1e-4);
    }

    #[test]
    fn matches_single_char_bang_noisy_two_char_and_two_line_wrapped_titles() {
        let mut entries = sample_entries();
        entries.push(AugmentNameEntry {
            augment_id: 2111,
            name_zh: "邦！".to_owned(),
            rarity: "黄金".to_owned(),
        });
        entries.push(AugmentNameEntry {
            augment_id: 2112,
            name_zh: "升级：米凯尔的祝福".to_owned(),
            rarity: "黄金".to_owned(),
        });
        let matcher = AugmentTextMatcher::new(entries);

        // (a) 1-char "邦！" matches in a 3-card row, and does not false-match standalone or noisy text.
        let bang_row = vec![
            OcrLineHit {
                text: "邦！".to_owned(),
                center_x_norm: 0.32,
                center_y_norm: 0.41,
            },
            OcrLineHit {
                text: "急速之追求".to_owned(),
                center_x_norm: 0.50,
                center_y_norm: 0.41,
            },
            OcrLineHit {
                text: "杀戮时间到了".to_owned(),
                center_x_norm: 0.68,
                center_y_norm: 0.41,
            },
        ];
        assert_eq!(
            matcher.match_ocr_lines_to_offer(&bang_row),
            Some(vec![2111, 101, 102])
        );
        let bang_standalone_or_noise = vec![
            OcrLineHit {
                text: "邦！".to_owned(),
                center_x_norm: 0.32,
                center_y_norm: 0.41,
            },
            OcrLineHit {
                text: "邦1".to_owned(),
                center_x_norm: 0.50,
                center_y_norm: 0.41,
            },
            OcrLineHit {
                text: "邦一下".to_owned(),
                center_x_norm: 0.68,
                center_y_norm: 0.41,
            },
        ];
        assert_eq!(
            matcher.match_ocr_lines_to_offer(&bang_standalone_or_noise),
            None
        );

        // (b) Noisy 2-char "l灵巧" / "灵巧1" match "灵巧", while CJK substrings remain rejected.
        let noisy_two_char_hits = vec![
            OcrLineHit {
                text: "l灵巧".to_owned(),
                center_x_norm: 0.32,
                center_y_norm: 0.41,
            },
            OcrLineHit {
                text: "1拯救".to_owned(),
                center_x_norm: 0.50,
                center_y_norm: 0.41,
            },
            OcrLineHit {
                text: "拯救一下".to_owned(),
                center_x_norm: 0.68,
                center_y_norm: 0.41,
            },
        ];
        assert_eq!(
            matcher.match_ocr_lines_to_offer(&noisy_two_char_hits),
            Some(vec![104, 103, 0])
        );
        let noisy_two_char_tail = vec![
            OcrLineHit {
                text: "灵巧1".to_owned(),
                center_x_norm: 0.32,
                center_y_norm: 0.41,
            },
            OcrLineHit {
                text: "急速之追求".to_owned(),
                center_x_norm: 0.50,
                center_y_norm: 0.41,
            },
            OcrLineHit {
                text: "灵巧走位".to_owned(),
                center_x_norm: 0.68,
                center_y_norm: 0.41,
            },
        ];
        assert_eq!(
            matcher.match_ocr_lines_to_offer(&noisy_two_char_tail),
            Some(vec![104, 101, 0])
        );

        // (c) 2-line wrapped "升级：" + "米凯尔的祝福" merges cleanly.
        let wrapped_hits = vec![
            OcrLineHit {
                text: "急速之追求".to_owned(),
                center_x_norm: 0.32,
                center_y_norm: 0.41,
            },
            OcrLineHit {
                text: "升级：".to_owned(),
                center_x_norm: 0.50,
                center_y_norm: 0.395,
            },
            OcrLineHit {
                text: "米凯尔的祝福".to_owned(),
                center_x_norm: 0.505,
                center_y_norm: 0.425,
            },
            OcrLineHit {
                text: "拯救".to_owned(),
                center_x_norm: 0.68,
                center_y_norm: 0.41,
            },
        ];
        assert_eq!(
            matcher.match_ocr_lines_to_offer(&wrapped_hits),
            Some(vec![101, 2112, 103])
        );
    }

    #[test]
    fn diagnoses_empty_unmatched_single_hit_and_row_filtering() {
        let matcher = AugmentTextMatcher::new(sample_entries());

        let empty_diag = matcher.diagnose_ocr_lines(&[]);
        assert_eq!(empty_diag.reject_reason, "no_ocr_lines");
        assert_eq!(empty_diag.total_lines, 0);

        let no_dict = matcher.diagnose_ocr_lines(&[OcrLineHit {
            text: "完全无关的屏幕文字".to_owned(),
            center_x_norm: 0.50,
            center_y_norm: 0.42,
        }]);
        assert_eq!(no_dict.reject_reason, "no_dictionary_match");
        assert!(no_dict.matched_hits.is_empty());
        assert!(no_dict.row_unmatched_texts.is_empty());

        let single_hit = matcher.diagnose_ocr_lines(&[
            OcrLineHit {
                text: "急速之追求".to_owned(),
                center_x_norm: 0.32,
                center_y_norm: 0.42,
            },
            OcrLineHit {
                text: "这是一段超过二十四个字符的同行未匹配卡片标题测试文本内容".to_owned(),
                center_x_norm: 0.50,
                center_y_norm: 0.44,
            },
            OcrLineHit {
                text: "底部聊天无关消息".to_owned(),
                center_x_norm: 0.20,
                center_y_norm: 0.68,
            },
        ]);
        assert_eq!(single_hit.reject_reason, "single_hit_only");
        assert_eq!(single_hit.matched_hits.len(), 1);
        assert_eq!(single_hit.matched_hits[0].0, 101);
        assert_eq!(single_hit.matched_hits[0].1, "急速之追求");
        assert_eq!(single_hit.row_unmatched_texts.len(), 1);
        assert_eq!(single_hit.row_unmatched_texts[0].0.chars().count(), 24);

        let misaligned = matcher.diagnose_ocr_lines(&[
            OcrLineHit {
                text: "急速之追求".to_owned(),
                center_x_norm: 0.32,
                center_y_norm: 0.30,
            },
            OcrLineHit {
                text: "拯救".to_owned(),
                center_x_norm: 0.35,
                center_y_norm: 0.55,
            },
        ]);
        assert_eq!(misaligned.reject_reason, "row_alignment_or_spacing_rejected");
        assert_eq!(misaligned.matched_hits.len(), 2);
    }

    fn loading_col_center_x(col: usize, frame_w: u32, frame_h: u32) -> f32 {
        let w = frame_w.max(320) as f32;
        let h = frame_h.max(200) as f32;
        let raw_card_w = h * 0.2328;
        let raw_gap = h * 0.0411;
        let raw_total = raw_card_w * 5.0 + raw_gap * 4.0;
        let scale = if raw_total > w * 0.90 {
            (w * 0.90) / raw_total
        } else {
            1.0
        };
        let card_w = raw_card_w * scale;
        let card_gap = raw_gap * scale;
        let row_w = card_w * 5.0 + card_gap * 4.0;
        let row_left = ((w - row_w) * 0.5 - w * 0.0015).max(0.0);
        (row_left + col as f32 * (card_w + card_gap) + card_w * 0.5) / w
    }

    #[test]
    fn aligns_top_loading_row_real_capture_sample() {
        let targets = vec![
            LoadingSlotTarget {
                name_zh: "莫甘娜".to_owned(),
                title_zh: "堕落天使".to_owned(),
                alias: "Morgana".to_owned(),
                summoner_name: "忘忧君c".to_owned(),
            },
            LoadingSlotTarget {
                name_zh: "厄斐琉斯".to_owned(),
                title_zh: "残月之肃".to_owned(),
                alias: "Aphelios".to_owned(),
                summoner_name: "薇古丝的黑影".to_owned(),
            },
            LoadingSlotTarget {
                name_zh: "卡莎".to_owned(),
                title_zh: "虚空之女".to_owned(),
                alias: "Kaisa".to_owned(),
                summoner_name: "汤坑ikun、中分头".to_owned(),
            },
            LoadingSlotTarget {
                name_zh: "格雷福斯".to_owned(),
                title_zh: "法外狂徒".to_owned(),
                alias: "Graves".to_owned(),
                summoner_name: "让我看看谁是笨比".to_owned(),
            },
            LoadingSlotTarget {
                name_zh: "萨勒芬妮".to_owned(),
                title_zh: "星籁歌姬".to_owned(),
                alias: "Seraphine".to_owned(),
                summoner_name: "黑影的薇古丝".to_owned(),
            },
        ];

        let (fw, fh) = (1920, 1080);
        let c = |col: usize| loading_col_center_x(col, fw, fh);
        let ocr_lines = vec![
            OcrLineHit {
                text: "黑帮教父 格雷福斯".to_owned(),
                center_x_norm: c(0),
                center_y_norm: 0.41,
            },
            OcrLineHit {
                text: "让我看看谁是笨比".to_owned(),
                center_x_norm: c(0),
                center_y_norm: 0.45,
            },
            OcrLineHit {
                text: "至臻 K/DA ALL OUT 卡莎".to_owned(),
                center_x_norm: c(1),
                center_y_norm: 0.41,
            },
            OcrLineHit {
                text: "汤坑ikun、中分头".to_owned(),
                center_x_norm: c(1),
                center_y_norm: 0.45,
            },
            OcrLineHit {
                text: "花仙子 萨勒芬妮".to_owned(),
                center_x_norm: c(2),
                center_y_norm: 0.41,
            },
            OcrLineHit {
                text: "黑影的薇古丝".to_owned(),
                center_x_norm: c(2),
                center_y_norm: 0.45,
            },
            OcrLineHit {
                text: "鬼魂新娘 莫甘娜".to_owned(),
                center_x_norm: c(3),
                center_y_norm: 0.41,
            },
            OcrLineHit {
                text: "忘忧君c".to_owned(),
                center_x_norm: c(3),
                center_y_norm: 0.45,
            },
            OcrLineHit {
                text: "灵魂莲华 厄斐琉斯".to_owned(),
                center_x_norm: c(4),
                center_y_norm: 0.41,
            },
            OcrLineHit {
                text: "薇古丝的黑影".to_owned(),
                center_x_norm: c(4),
                center_y_norm: 0.45,
            },
        ];

        let (order, matched_cols, total_score) =
            align_loading_row_from_ocr(&ocr_lines, &targets, fw, fh)
                .expect("top loading row should align");
        assert_eq!(order, vec![3, 2, 4, 0, 1]);
        assert_eq!(matched_cols, 5);
        assert!(total_score >= 5000);
    }

    #[test]
    fn aligns_bottom_loading_row_with_partial_ocr_and_title_fallback() {
        let full_targets = vec![
            LoadingSlotTarget {
                name_zh: "赛恩".to_owned(),
                title_zh: "亡灵战神".to_owned(),
                alias: "Sion".to_owned(),
                summoner_name: "蒋心来".to_owned(),
            },
            LoadingSlotTarget {
                name_zh: "辛德拉".to_owned(),
                title_zh: "暗黑元首".to_owned(),
                alias: "Syndra".to_owned(),
                summoner_name: "输也很厉害了".to_owned(),
            },
            LoadingSlotTarget {
                name_zh: "克格莫".to_owned(),
                title_zh: "深渊巨口".to_owned(),
                alias: "KogMaw".to_owned(),
                summoner_name: "彼岸花、忆".to_owned(),
            },
            LoadingSlotTarget {
                name_zh: "墨菲特".to_owned(),
                title_zh: "熔岩巨兽".to_owned(),
                alias: "Malphite".to_owned(),
                summoner_name: "跳跳糖真好吃".to_owned(),
            },
            LoadingSlotTarget {
                name_zh: "凯尔".to_owned(),
                title_zh: "正义天使".to_owned(),
                alias: "Kayle".to_owned(),
                summoner_name: "啪怕Z".to_owned(),
            },
        ];

        let (fw, fh) = (2560, 1440);
        let c = |col: usize| loading_col_center_x(col, fw, fh);

        // Full OCR lines (skin/hero + summoner names).
        let full_lines = vec![
            OcrLineHit {
                text: "至臻 星之守护者 辛德拉".to_owned(),
                center_x_norm: c(0),
                center_y_norm: 0.89,
            },
            OcrLineHit {
                text: "输也很厉害了".to_owned(),
                center_x_norm: c(0),
                center_y_norm: 0.93,
            },
            OcrLineHit {
                text: "霸天巨兽 墨菲特".to_owned(),
                center_x_norm: c(1),
                center_y_norm: 0.89,
            },
            OcrLineHit {
                text: "跳跳糖真好吃".to_owned(),
                center_x_norm: c(1),
                center_y_norm: 0.93,
            },
            OcrLineHit {
                text: "电格莫".to_owned(),
                center_x_norm: c(2),
                center_y_norm: 0.89,
            },
            OcrLineHit {
                text: "彼岸花、忆".to_owned(),
                center_x_norm: c(2),
                center_y_norm: 0.93,
            },
            OcrLineHit {
                text: "钢铁之翼 凯尔".to_owned(),
                center_x_norm: c(3),
                center_y_norm: 0.89,
            },
            OcrLineHit {
                text: "啪怕Z".to_owned(),
                center_x_norm: c(3),
                center_y_norm: 0.93,
            },
            OcrLineHit {
                text: "亡灵战神".to_owned(),
                center_x_norm: c(4),
                center_y_norm: 0.89,
            },
            OcrLineHit {
                text: "蒋心来".to_owned(),
                center_x_norm: c(4),
                center_y_norm: 0.93,
            },
        ];
        let (order, matched_cols, _) =
            align_loading_row_from_ocr(&full_lines, &full_targets, fw, fh)
                .expect("bottom row with full OCR should align");
        assert_eq!(order, vec![1, 3, 2, 4, 0]);
        assert_eq!(matched_cols, 5);

        // Hero/skin lines only (no summoner names at all), including partial "电格莫" -> "克格莫" and "亡灵战神" -> "赛恩".
        let hero_only_lines = vec![
            OcrLineHit {
                text: "至臻 星之守护者 辛德拉".to_owned(),
                center_x_norm: c(0),
                center_y_norm: 0.89,
            },
            OcrLineHit {
                text: "霸天巨兽 墨菲特".to_owned(),
                center_x_norm: c(1),
                center_y_norm: 0.89,
            },
            OcrLineHit {
                text: "电格莫".to_owned(),
                center_x_norm: c(2),
                center_y_norm: 0.89,
            },
            OcrLineHit {
                text: "钢铁之翼 凯尔".to_owned(),
                center_x_norm: c(3),
                center_y_norm: 0.89,
            },
            OcrLineHit {
                text: "亡灵战神".to_owned(),
                center_x_norm: c(4),
                center_y_norm: 0.89,
            },
        ];
        let targets_no_summoner: Vec<LoadingSlotTarget> = full_targets
            .iter()
            .map(|t| LoadingSlotTarget {
                summoner_name: String::new(),
                ..t.clone()
            })
            .collect();
        let (order_hero_only, matched_hero_only, _) =
            align_loading_row_from_ocr(&hero_only_lines, &targets_no_summoner, fw, fh)
                .expect("hero/skin-only bottom row should still align accurately");
        assert_eq!(order_hero_only, vec![1, 3, 2, 4, 0]);
        assert_eq!(matched_hero_only, 5);
    }

    #[test]
    fn rejects_empty_and_noise_loading_ocr_frames() {
        let targets = vec![
            LoadingSlotTarget {
                name_zh: "赛恩".to_owned(),
                title_zh: "亡灵战神".to_owned(),
                alias: "Sion".to_owned(),
                summoner_name: "蒋心来".to_owned(),
            },
            LoadingSlotTarget {
                name_zh: "辛德拉".to_owned(),
                title_zh: "暗黑元首".to_owned(),
                alias: "Syndra".to_owned(),
                summoner_name: "输也很厉害了".to_owned(),
            },
            LoadingSlotTarget {
                name_zh: "克格莫".to_owned(),
                title_zh: "深渊巨口".to_owned(),
                alias: "KogMaw".to_owned(),
                summoner_name: "彼岸花、忆".to_owned(),
            },
        ];

        assert_eq!(align_loading_row_from_ocr(&[], &targets, 1920, 1080), None);
        assert_eq!(align_loading_row_from_ocr(&[], &[], 1920, 1080), None);

        let noise = vec![
            OcrLineHit {
                text: "100%".to_owned(),
                center_x_norm: loading_col_center_x(0, 1920, 1080),
                center_y_norm: 0.42,
            },
            OcrLineHit {
                text: "正在加载游戏提示".to_owned(),
                center_x_norm: loading_col_center_x(1, 1920, 1080),
                center_y_norm: 0.42,
            },
            OcrLineHit {
                text: "42ms".to_owned(),
                center_x_norm: loading_col_center_x(2, 1920, 1080),
                center_y_norm: 0.42,
            },
        ];
        assert_eq!(align_loading_row_from_ocr(&noise, &targets, 1920, 1080), None);
    }
}
