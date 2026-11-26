//! Shared domain models, ROI manifests, and event types for Hexrs.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Active overlay operating mode managed by the core state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AppMode {
    /// Neither champ select nor an active match is detected; overlay stays hidden.
    #[default]
    Dormant,
    /// Client is in champ select; sidebar docks cleanly next to `LeagueClientUx.exe`.
    LobbyDocked,
    /// Game client `League of Legends.exe` is running; overlay renders click-through in-game.
    InGameOverlay,
}

/// Strength tier rating for champions and hextech augments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum HeroTier {
    /// Highest meta tier (`S+`).
    SPlus,
    /// Strong meta tier (`S`).
    S,
    /// Above-average tier (`A`).
    A,
    /// Balanced tier (`B`).
    B,
    /// Situational tier (`C`).
    C,
    /// Weak tier (`D`).
    D,
}

impl HeroTier {
    /// Returns the human-readable badge string for UI rendering.
    pub const fn label(self) -> &'static str {
        match self {
            Self::SPlus => "S+",
            Self::S => "S",
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
        }
    }

    /// Returns a numeric sort key where smaller values represent stronger tiers.
    pub const fn sort_rank(self) -> u8 {
        match self {
            Self::SPlus => 0,
            Self::S => 1,
            Self::A => 2,
            Self::B => 3,
            Self::C => 4,
            Self::D => 5,
        }
    }

    /// Parses a tier label (case-insensitive) from metadata or SQLite storage.
    pub fn from_label(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "S+" | "SPLUS" | "S_PLUS" => Some(Self::SPlus),
            "S" => Some(Self::S),
            "A" => Some(Self::A),
            "B" => Some(Self::B),
            "C" => Some(Self::C),
            "D" => Some(Self::D),
            _ => None,
        }
    }
}

impl fmt::Display for HeroTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// One allied slot inside champ select.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TeamSlot {
    pub cell_id: i32,
    pub champion_id: i32,
    pub pick_intent: i32,
    pub assigned_position: String,
}

/// Snapshot of the current LCU champ select session.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChampSelectState {
    pub phase: String,
    pub game_mode: String,
    pub queue_id: Option<i32>,
    pub local_player_cell_id: i32,
    pub local_champion_id: i32,
    pub local_intent_id: i32,
    pub my_team: Vec<TeamSlot>,
    pub bench_champions: Vec<i32>,
    pub rerolls_remaining: u32,
}

/// Snapshot of the local player's in-game state from the Live Client Data API (`127.0.0.1:2999`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LiveGameState {
    pub game_mode: String,
    pub game_time_secs: f64,
    pub active_champion_name: String,
    pub active_champion_id: Option<i32>,
    pub level: u32,
    pub current_gold: f64,
    pub owned_item_ids: Vec<i32>,
    #[serde(default)]
    pub is_dead: bool,
    #[serde(default)]
    pub respawn_timer: f64,
}

/// Strictly anonymized end-of-game participant record (zero PII: no names, PUUIDs, or Riot IDs).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EndOfGameParticipant {
    pub champion_id: i32,
    pub placement: u8,
    pub win: bool,
    pub items: Vec<i32>,
    pub augments: Vec<i32>,
}

/// Strictly anonymized end-of-game match summary ready for crowdsourced telemetry.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EndOfGameStats {
    pub game_mode: String,
    pub queue_id: Option<i32>,
    pub participants: Vec<EndOfGameParticipant>,
}

/// Normalized screen-space rectangle (`0.0..=1.0` relative to capture width/height).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct RoiRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RoiRect {
    /// Creates a new normalized ROI rectangle.
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Converts normalized coordinates into clamped pixel `(x, y, width, height)`.
    pub fn to_pixel_rect(self, frame_width: u32, frame_height: u32) -> (u32, u32, u32, u32) {
        let fw = frame_width as f32;
        let fh = frame_height as f32;

        let px = (self.x.clamp(0.0, 1.0) * fw).round() as u32;
        let py = (self.y.clamp(0.0, 1.0) * fh).round() as u32;
        let max_w = frame_width.saturating_sub(px);
        let max_h = frame_height.saturating_sub(py);
        let pw = ((self.w.clamp(0.0, 1.0) * fw).round() as u32).min(max_w);
        let ph = ((self.h.clamp(0.0, 1.0) * fh).round() as u32).min(max_h);

        (px, py, pw, ph)
    }
}

/// Aspect-ratio-specific ROI layout for the 3-card augment selection modal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AspectRoiPreset {
    pub card_icons: [RoiRect; 3],
    pub header_indicators: Vec<RoiRect>,
}

/// Cloud-updatable ROI manifest supporting multiple monitor aspect ratios.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoiManifest {
    pub version: String,
    pub sha256: String,
    #[serde(rename = "16_9")]
    pub preset_16_9: AspectRoiPreset,
    #[serde(rename = "16_10")]
    pub preset_16_10: AspectRoiPreset,
    #[serde(rename = "21_9")]
    pub preset_21_9: AspectRoiPreset,
}

impl RoiManifest {
    /// Selects the closest aspect-ratio preset for a given capture resolution.
    pub fn preset_for_resolution(&self, width: u32, height: u32) -> &AspectRoiPreset {
        if height == 0 {
            return &self.preset_16_9;
        }
        let ratio = width as f32 / height as f32;
        if ratio >= 2.05 {
            &self.preset_21_9
        } else if ratio <= 1.66 {
            &self.preset_16_10
        } else {
            &self.preset_16_9
        }
    }
}

impl Default for RoiManifest {
    fn default() -> Self {
        Self {
            version: "1.0.0-default".to_owned(),
            sha256: String::new(),
            preset_16_9: AspectRoiPreset {
                card_icons: [
                    RoiRect::new(0.285, 0.315, 0.065, 0.115),
                    RoiRect::new(0.467, 0.315, 0.065, 0.115),
                    RoiRect::new(0.650, 0.315, 0.065, 0.115),
                ],
                header_indicators: vec![RoiRect::new(0.410, 0.165, 0.180, 0.045)],
            },
            preset_16_10: AspectRoiPreset {
                card_icons: [
                    RoiRect::new(0.285, 0.325, 0.065, 0.105),
                    RoiRect::new(0.467, 0.325, 0.065, 0.105),
                    RoiRect::new(0.650, 0.325, 0.065, 0.105),
                ],
                header_indicators: vec![RoiRect::new(0.410, 0.180, 0.180, 0.042)],
            },
            preset_21_9: AspectRoiPreset {
                card_icons: [
                    RoiRect::new(0.338, 0.315, 0.049, 0.115),
                    RoiRect::new(0.475, 0.315, 0.049, 0.115),
                    RoiRect::new(0.612, 0.315, 0.049, 0.115),
                ],
                header_indicators: vec![RoiRect::new(0.432, 0.165, 0.136, 0.045)],
            },
        }
    }
}

/// Normalized screen-space anchor geometry of the detected augment cards (`0.0..=1.0`, typically 3 or 4 cards).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AugmentOfferLayout {
    /// Left-to-right normalized card center X coordinates (`0.0..=1.0`, typically 3 or 4 cards).
    pub slot_centers_x: Vec<f32>,
    /// Normalized Y coordinate of the card title row (`0.0..=1.0`).
    pub title_y: f32,
}

/// Core events streamed from `hex-core` (and `hex-vision`) to the UI layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AppEvent {
    ModeChanged(AppMode),
    ChampSelectUpdated(ChampSelectState),
    LiveGameUpdated(LiveGameState),
    EndOfGameCaptured(EndOfGameStats),
    AugmentOfferDetected {
        candidates: Vec<i32>,
        capture_ms: u64,
        #[serde(default)]
        layout: Option<AugmentOfferLayout>,
    },
    AugmentOfferCleared,
    LcuConnectionChanged(bool),
}
