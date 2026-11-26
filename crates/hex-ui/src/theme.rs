//! Dark hextech esports color tokens and shared badge styling helpers.

use gpui::{div, prelude::*, px, rgb, rgba, Div, FontWeight, Rgba};
use hex_core::HeroTier;

/// Deep hextech obsidian canvas (`#0B101B`).
pub fn canvas_bg() -> Rgba {
    rgb(0x0B101B)
}

/// Primary card surface (`#131B2E`).
pub fn card_bg() -> Rgba {
    rgb(0x131B2E)
}

/// Hover / elevated card surface (`#1B253B`).
pub fn card_elevated_bg() -> Rgba {
    rgb(0x1B253B)
}

/// Primary hextech gold accent (`#C8AA6E`).
pub fn gold() -> Rgba {
    rgb(0xC8AA6E)
}

/// Subtle translucent gold border.
pub fn border_subtle() -> Rgba {
    rgba(0xC8AA6E38)
}

/// Active gold border highlight.
pub fn border_gold() -> Rgba {
    rgba(0xC8AA6E99)
}

/// Translucent gold pill fill.
pub fn gold_dim() -> Rgba {
    rgba(0xC8AA6E24)
}

/// Primary hextech cyan accent (`#0AC8B9`).
pub fn cyan() -> Rgba {
    rgb(0x0AC8B9)
}

/// Translucent cyan border.
pub fn border_cyan() -> Rgba {
    rgba(0x0AC8B988)
}

/// Translucent cyan pill fill.
pub fn cyan_dim() -> Rgba {
    rgba(0x0AC8B924)
}

/// Primary warm parchment text (`#F0E6D2`).
pub fn text_primary() -> Rgba {
    rgb(0xF0E6D2)
}

/// Secondary muted text (`#A09B8C`).
pub fn text_muted() -> Rgba {
    rgb(0xA09B8C)
}

/// Warning pill background.
pub fn warning_bg() -> Rgba {
    rgba(0xE8405724)
}

/// Warning pill border.
pub fn warning_border() -> Rgba {
    rgba(0xE8405788)
}

/// Warning pill text.
pub fn warning_text() -> Rgba {
    rgb(0xFF7B8E)
}

/// Physical damage ratio bar fill.
pub fn ad_bar() -> Rgba {
    rgb(0xE07A5F)
}

/// Magic damage ratio bar fill.
pub fn ap_bar() -> Rgba {
    rgb(0x4EA8DE)
}

/// Translucent dark obsidian fill for floating in-game overlay cards.
pub fn overlay_card_bg() -> Rgba {
    rgba(0x0B101BE8)
}

/// Glowing gold background for `⚡ 完美协同` alerts.
pub fn synergy_gold_bg() -> Rgba {
    rgba(0xC8AA6E33)
}

/// Glowing gold border for `⚡ 完美协同` alerts.
pub fn synergy_gold_border() -> Rgba {
    rgba(0xFFD97DCC)
}

/// Bright gold text for `⚡ 完美协同` alerts.
pub fn synergy_gold_text() -> Rgba {
    rgb(0xFFD97D)
}

/// Highlight background for already-owned build items.
pub fn owned_item_bg() -> Rgba {
    rgba(0x0AC8B92E)
}

/// Highlight border for already-owned build items.
pub fn owned_item_border() -> Rgba {
    rgba(0x0AC8B9AA)
}

/// Color trio for rendering strength tier badges (`S+` .. `D`).
pub struct TierPalette {
    pub bg: Rgba,
    pub border: Rgba,
    pub text: Rgba,
}

/// Returns the esports badge palette for a `HeroTier`.
pub fn tier_palette(tier: HeroTier) -> TierPalette {
    match tier {
        HeroTier::SPlus => TierPalette {
            bg: rgba(0xC8AA6E38),
            border: rgba(0xF0C75EBF),
            text: rgb(0xFFD97D),
        },
        HeroTier::S => TierPalette {
            bg: rgba(0x0AC8B933),
            border: rgba(0x0AC8B9AA),
            text: rgb(0x4BE8DB),
        },
        HeroTier::A => TierPalette {
            bg: rgba(0x3B82F62E),
            border: rgba(0x60A5FA99),
            text: rgb(0x93C5FD),
        },
        HeroTier::B => TierPalette {
            bg: rgba(0x10B98126),
            border: rgba(0x34D39988),
            text: rgb(0x6EE7B7),
        },
        HeroTier::C => TierPalette {
            bg: rgba(0x94A3B824),
            border: rgba(0x94A3B866),
            text: rgb(0xCBD5E1),
        },
        HeroTier::D => TierPalette {
            bg: rgba(0xE840571F),
            border: rgba(0xE8405755),
            text: rgb(0xFDA4AF),
        },
    }
}

/// Returns `(background, border, text)` colors matching `tier_badge(tier)`.
pub fn tier_colors(tier: HeroTier) -> (Rgba, Rgba, Rgba) {
    let pal = tier_palette(tier);
    (pal.bg, pal.border, pal.text)
}

/// Renders a compact `S+`..`D` tier badge chip.
pub fn tier_badge(tier: HeroTier) -> Div {
    let pal = tier_palette(tier);
    div()
        .px(px(6.0))
        .py(px(2.0))
        .rounded_md()
        .border_1()
        .border_color(pal.border)
        .bg(pal.bg)
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(pal.text)
        .child(tier.label())
}
