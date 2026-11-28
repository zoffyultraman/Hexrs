//! Centralized Hextech esports Design Tokens (hexrs_hud.md Section 2).

use gpui::{rgb, rgba, Rgba};
use hex_core::HeroTier;

/// Monospace font family for tabular numeric alignment across all HUD views.
pub const MONO_FONT: &str = "Consolas";

/// Fully transparent fill for idle or invisible overlay surfaces.
pub fn transparent() -> Rgba {
    rgba(0x00000000)
}

/// Primary HUD translucent deep obsidian surface (`rgba(9, 20, 40, 0.85)`).
pub fn bg_surface() -> Rgba {
    rgba(0x091428D9)
}

/// Dense translucent dark slate surface (`rgba(11, 18, 32, 0.92)`).
pub fn bg_surface_dense() -> Rgba {
    rgba(0x0B1220EB)
}

/// Highlighted / recommended / expanded card fill (`rgba(15, 32, 58, 0.94)`).
pub fn bg_highlight() -> Rgba {
    rgba(0x0F203AF0)
}

/// Subtle crimson danger surface for low-sample warnings (`rgba(28, 16, 20, 0.88)`).
pub fn bg_danger_subtle() -> Rgba {
    rgba(0x1C1014E0)
}

/// Deep Hextech obsidian sidebar canvas (`#091428`).
pub fn canvas_bg() -> Rgba {
    rgb(0x091428)
}

/// Primary sidebar card surface (`#0B1220`).
pub fn card_bg() -> Rgba {
    rgb(0x0B1220)
}

/// Elevated / hover sidebar card surface (`#0F203A`).
pub fn card_elevated_bg() -> Rgba {
    rgb(0x0F203A)
}

/// Floating corner overlay card surface (`bg_surface_dense`).
pub fn overlay_card_bg() -> Rgba {
    bg_surface_dense()
}

/// Subtle translucent gold border (`rgba(200, 170, 110, 0.20)`).
pub fn border_subtle() -> Rgba {
    rgba(0xC8AA6E33)
}

/// Cool slate border for normal cards and inactive slots (`rgba(71, 85, 105, 0.45)`).
pub fn border_cool() -> Rgba {
    rgba(0x47556973)
}

/// Active Hextech gold border (`rgba(232, 193, 112, 0.50)`).
pub fn border_gold() -> Rgba {
    rgba(0xE8C17080)
}

/// Solid Hextech gold highlight border (`#E8C170`).
pub fn border_gold_strong() -> Rgba {
    rgb(0xE8C170)
}

/// Danger warning card border (`rgba(248, 113, 113, 0.35)`).
pub fn danger_border() -> Rgba {
    rgba(0xF8717159)
}

/// Primary crisp HUD foreground text (`#F8FAFC`).
pub fn text_primary() -> Rgba {
    rgb(0xF8FAFC)
}

/// Secondary light slate foreground text (`#CBD5E1`).
pub fn text_secondary() -> Rgba {
    rgb(0xCBD5E1)
}

/// Muted metadata foreground text (`#94A3B8`).
pub fn text_muted() -> Rgba {
    rgb(0x94A3B8)
}

/// Primary Hextech gold accent (`#E8C170`).
pub fn gold() -> Rgba {
    rgb(0xE8C170)
}

/// Translucent Hextech gold pill fill.
pub fn gold_dim() -> Rgba {
    rgba(0xE8C17029)
}

/// Primary Hextech sky-cyan accent (`#38BDF8`).
pub fn cyan() -> Rgba {
    rgb(0x38BDF8)
}

/// Translucent sky-cyan border.
pub fn border_cyan() -> Rgba {
    rgba(0x38BDF873)
}

/// Translucent sky-cyan pill fill.
pub fn cyan_dim() -> Rgba {
    rgba(0x38BDF829)
}

/// Warning pill background.
pub fn warning_bg() -> Rgba {
    rgba(0xF871712E)
}

/// Warning pill border.
pub fn warning_border() -> Rgba {
    rgba(0xF8717180)
}

/// Warning pill & alert text (`#F87171`).
pub fn warning_text() -> Rgba {
    rgb(0xF87171)
}

/// Glowing gold background for synergy alerts.
pub fn synergy_gold_bg() -> Rgba {
    rgba(0xE8C1702E)
}

/// Glowing gold border for synergy alerts.
pub fn synergy_gold_border() -> Rgba {
    rgba(0xE8C17099)
}

/// Bright gold text for synergy alerts.
pub fn synergy_gold_text() -> Rgba {
    rgb(0xE8C170)
}

/// Highlight background for already-owned build items.
pub fn owned_item_bg() -> Rgba {
    rgba(0x38BDF829)
}

/// Highlight border for already-owned build items.
pub fn owned_item_border() -> Rgba {
    rgba(0x38BDF880)
}

/// Physical damage ratio bar fill.
pub fn ad_bar() -> Rgba {
    rgb(0xE07A5F)
}

/// Magic damage ratio bar fill.
pub fn ap_bar() -> Rgba {
    rgb(0x38BDF8)
}

/// Left transparent edge of the allied team strip horizontal gradient.
pub fn team_strip_gradient_start() -> Rgba {
    rgba(0x09142800)
}

/// Right opaque edge of the allied team strip horizontal gradient.
pub fn team_strip_gradient_end() -> Rgba {
    rgba(0x091428E6)
}

/// Top gradient stop for the straddling `★ 推荐` badge.
pub fn recommended_gold_start() -> Rgba {
    rgb(0xE8C170)
}

/// Bottom gradient stop for the straddling `★ 推荐` badge.
pub fn recommended_gold_end() -> Rgba {
    rgb(0xD97706)
}

/// Dark high-contrast text on the `★ 推荐` gold badge.
pub fn recommended_text() -> Rgba {
    rgb(0x0F172A)
}

/// Frame border color for bench hero nameplates (highlights `S+` and `S` for quick scanning).
pub fn bench_frame_border(tier: HeroTier) -> Rgba {
    match tier {
        HeroTier::SPlus => rgba(0xE8C17066),
        HeroTier::S => rgba(0x38BDF866),
        _ => border_cool(),
    }
}

/// Color trio for rendering strength tier badges (`S+` .. `D`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TierPalette {
    pub bg: Rgba,
    pub border: Rgba,
    pub text: Rgba,
}

/// Returns the Section 2.2 semantic badge palette for a `HeroTier`.
pub fn tier_palette(tier: HeroTier) -> TierPalette {
    match tier {
        HeroTier::SPlus => TierPalette {
            bg: rgba(0xE8C17029),
            border: rgba(0xE8C17080),
            text: rgb(0xE8C170),
        },
        HeroTier::S => TierPalette {
            bg: rgba(0x38BDF829),
            border: rgba(0x38BDF873),
            text: rgb(0x38BDF8),
        },
        HeroTier::A | HeroTier::B => TierPalette {
            bg: rgba(0x34D39924),
            border: rgba(0x34D39966),
            text: rgb(0x34D399),
        },
        HeroTier::C => TierPalette {
            bg: rgba(0x94A3B824),
            border: rgba(0x94A3B859),
            text: rgb(0x94A3B8),
        },
        HeroTier::D => TierPalette {
            bg: rgba(0xF871711F),
            border: rgba(0xF8717159),
            text: rgb(0xF87171),
        },
    }
}

/// Returns the 18% opacity fill used by large (`TierBadgeSize::Lg`) tier badges.
pub fn tier_lg_bg(tier: HeroTier) -> Rgba {
    match tier {
        HeroTier::SPlus => rgba(0xE8C1702E),
        HeroTier::S => rgba(0x38BDF82E),
        HeroTier::A | HeroTier::B => rgba(0x34D3992E),
        HeroTier::C => rgba(0x94A3B82E),
        HeroTier::D => rgba(0xF871712E),
    }
}

/// Returns the Section 2.3 win-rate text color for numeric strength tiers.
pub fn win_rate_color(win_rate: f64) -> Rgba {
    if win_rate >= 52.0 {
        rgb(0x38BDF8)
    } else if win_rate >= 50.0 {
        rgb(0xF8FAFC)
    } else if win_rate >= 48.0 {
        rgb(0x94A3B8)
    } else {
        rgb(0xF87171)
    }
}

/// Returns the 75%-alpha suffix color matching `win_rate_color`.
pub fn win_rate_suffix_color(win_rate: f64) -> Rgba {
    if win_rate >= 52.0 {
        rgba(0x38BDF8BF)
    } else if win_rate >= 50.0 {
        rgba(0xF8FAFCBF)
    } else if win_rate >= 48.0 {
        rgba(0x94A3B8BF)
    } else {
        rgba(0xF87171BF)
    }
}

/// Returns `(score_color, title_color)` for V2 player rating `tier_index` (Section 2.3).
pub fn player_score_colors(tier_index: u8) -> (Rgba, Rgba) {
    match tier_index {
        0 => (rgb(0xFDE047), rgb(0xFEF08A)),
        1 => (rgb(0xE8C170), rgb(0xE2E8F0)),
        2 => (rgb(0x34D399), rgb(0xCBD5E1)),
        3 => (rgb(0xCBD5E1), rgb(0x94A3B8)),
        _ => (rgb(0xFDA4AF), rgb(0xF87171)),
    }
}

/// Color trio for translucent Ghost Pill tags (Section 2.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PillPalette {
    pub bg: Rgba,
    pub border: Rgba,
    pub text: Rgba,
}

/// Returns the palette for a detected premade party group ID.
pub fn premade_pill_palette(group_id: u8) -> PillPalette {
    match group_id {
        1 => PillPalette {
            bg: rgba(0x22D3EE2E),
            border: rgba(0x22D3EE73),
            text: rgb(0x22D3EE),
        },
        2 => PillPalette {
            bg: rgba(0xC084FC2E),
            border: rgba(0xC084FC73),
            text: rgb(0xC084FC),
        },
        _ => PillPalette {
            bg: rgba(0x38BDF82E),
            border: rgba(0x38BDF873),
            text: rgb(0x38BDF8),
        },
    }
}

/// Returns the gold palette for the premade last-game MVP pill.
pub fn mvp_pill_palette() -> PillPalette {
    PillPalette {
        bg: rgba(0xE8C1702E),
        border: rgba(0xE8C17080),
        text: rgb(0xE8C170),
    }
}

/// Returns the warm-orange palette for win-streak pills (`>= 3`).
pub fn win_streak_pill_palette() -> PillPalette {
    PillPalette {
        bg: rgba(0xF973162E),
        border: rgba(0xFB923C73),
        text: rgb(0xFB923C),
    }
}

/// Returns the coral-red palette for loss-streak pills (`<= -3`).
pub fn loss_streak_pill_palette() -> PillPalette {
    PillPalette {
        bg: rgba(0xF871712E),
        border: rgba(0xF8717180),
        text: rgb(0xF87171),
    }
}

#[cfg(test)]
mod tests {
    use super::{player_score_colors, win_rate_color};
    use gpui::rgb;

    #[test]
    fn win_rate_color_respects_section_2_3_thresholds() {
        assert_eq!(win_rate_color(52.0), rgb(0x38BDF8));
        assert_eq!(win_rate_color(51.99), rgb(0xF8FAFC));
        assert_eq!(win_rate_color(50.0), rgb(0xF8FAFC));
        assert_eq!(win_rate_color(49.99), rgb(0x94A3B8));
        assert_eq!(win_rate_color(48.0), rgb(0x94A3B8));
        assert_eq!(win_rate_color(47.99), rgb(0xF87171));
    }

    #[test]
    fn player_score_colors_maps_all_v2_tiers() {
        assert_eq!(player_score_colors(0), (rgb(0xFDE047), rgb(0xFEF08A)));
        assert_eq!(player_score_colors(1), (rgb(0xE8C170), rgb(0xE2E8F0)));
        assert_eq!(player_score_colors(2), (rgb(0x34D399), rgb(0xCBD5E1)));
        assert_eq!(player_score_colors(3), (rgb(0xCBD5E1), rgb(0x94A3B8)));
        assert_eq!(player_score_colors(4), (rgb(0xFDA4AF), rgb(0xF87171)));
        assert_eq!(player_score_colors(9), (rgb(0xFDA4AF), rgb(0xF87171)));
    }
}
