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
    #[serde(default)]
    pub puuid: String,
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
    #[serde(default)]
    pub allow_subset_champion_picks: bool,
    #[serde(default)]
    pub subset_champions: Vec<i32>,
    pub rerolls_remaining: u32,
}

/// One participant entry extracted from the Live Client Data API (`allPlayers`).
/// `display_name` is transient in-memory only (used for Tab OCR alignment); never log,
/// persist to disk, or emit to telemetry.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LivePlayerEntry {
    pub team: LoadingTeamSide,
    pub champion_name: String,
    pub raw_champion_name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub position: String,
}

/// Snapshot of the local player's in-game state from the Live Client Data API (`127.0.0.1:2999`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LiveGameState {
    pub game_mode: String,
    pub game_time_secs: f64,
    pub active_champion_name: String,
    #[serde(default)]
    pub active_champion_raw_name: String,
    pub active_champion_id: Option<i32>,
    pub level: u32,
    pub current_gold: f64,
    pub owned_item_ids: Vec<i32>,
    #[serde(default)]
    pub is_dead: bool,
    #[serde(default)]
    pub respawn_timer: f64,
    #[serde(default)]
    pub game_started_event: bool,
    #[serde(default)]
    pub players: Vec<LivePlayerEntry>,
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

/// Fetch/analysis readiness state for a teammate's ARAM Mayhem rating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SummonerRatingStatus {
    Loading,
    Ready,
    Private,
    InsufficientSamples,
}

/// One score-to-title threshold rule in `RatingTitlesConfig`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RatingTierRule {
    pub min_score: u8,
    pub label: String,
}

/// Customizable title mapping for teammate ratings (`0..=100` composite score).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RatingTitlesConfig {
    pub tiers: Vec<RatingTierRule>,
    pub insufficient_samples: String,
    pub private_profile: String,
}

impl Default for RatingTitlesConfig {
    fn default() -> Self {
        Self {
            tiers: vec![
                RatingTierRule {
                    min_score: 82,
                    label: "夯".to_owned(),
                },
                RatingTierRule {
                    min_score: 70,
                    label: "顶级".to_owned(),
                },
                RatingTierRule {
                    min_score: 58,
                    label: "人上人".to_owned(),
                },
                RatingTierRule {
                    min_score: 45,
                    label: "NPC".to_owned(),
                },
                RatingTierRule {
                    min_score: 0,
                    label: "拉完了".to_owned(),
                },
            ],
            insufficient_samples: "样本少".to_owned(),
            private_profile: "隐藏".to_owned(),
        }
    }
}

impl RatingTitlesConfig {
    /// Resolves `(tier_index: 0..=4, label)` for a status and composite score.
    pub fn resolve(&self, status: SummonerRatingStatus, score: u8) -> (u8, String) {
        match status {
            SummonerRatingStatus::Loading => (2, "查询中".to_owned()),
            SummonerRatingStatus::Private => (4, self.private_profile.clone()),
            SummonerRatingStatus::InsufficientSamples => (3, self.insufficient_samples.clone()),
            SummonerRatingStatus::Ready => {
                if self.tiers.is_empty() {
                    return Self::default().resolve(status, score);
                }
                let mut sorted: Vec<&RatingTierRule> = self.tiers.iter().collect();
                sorted.sort_by(|a, b| b.min_score.cmp(&a.min_score));
                for (idx, rule) in sorted.iter().enumerate() {
                    if score >= rule.min_score {
                        return (idx.min(4) as u8, rule.label.clone());
                    }
                }
                let last_idx = sorted.len().saturating_sub(1).min(4) as u8;
                let last_label = sorted
                    .last()
                    .map(|r| r.label.clone())
                    .unwrap_or_else(|| "拉完了".to_owned());
                (last_idx, last_label)
            }
        }
    }

    /// Loads config from `path`, or creates pretty-printed defaults if missing (falling back on errors).
    pub fn load_or_init(path: &std::path::Path) -> Self {
        if !path.exists() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let defaults = Self::default();
            if let Ok(json) = serde_json::to_string_pretty(&defaults) {
                let _ = std::fs::write(path, json);
            }
            return defaults;
        }

        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(&raw) {
            Ok(cfg) if !cfg.tiers.is_empty() => cfg,
            _ => Self::default(),
        }
    }
}

/// Per-teammate ARAM Mayhem (`KIWI` / `queueId 2400`) rating and premade tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummonerRatingSummary {
    pub cell_id: i32,
    pub status: SummonerRatingStatus,
    pub score: u8,
    pub tier_index: u8,
    pub title: String,
    pub games_count: u8,
    pub win_rate: f64,
    pub avg_kda: f64,
    pub streak: i8,
    pub premade_group_id: Option<u8>,
    pub is_premade_last_mvp: bool,
}

/// Detected premade party group across current allied teammates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PremadeGroupSummary {
    pub group_id: u8,
    pub slot_indices: Vec<usize>,
    pub shared_games: usize,
    pub last_mvp_slot_index: Option<usize>,
}

/// Aggregated ratings and premade/last-game-MVP groups for all allied champ select slots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TeamRatingsSnapshot {
    pub slots: Vec<SummonerRatingSummary>,
    pub premade_groups: Vec<PremadeGroupSummary>,
}

/// Which team side the local player belongs to on the match loading screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LoadingTeamSide {
    #[default]
    Blue,
    Red,
}

/// One participant slot on the 10-player loading screen (`teamOne` or `teamTwo`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LoadingPlayerSlot {
    pub slot_index: usize,
    pub champion_id: i32,
    pub puuid: String,
    /// Transient in-memory Riot/summoner display name for OCR card-order alignment only.
    /// Never log, persist to disk, or emit to telemetry.
    #[serde(default)]
    pub display_name: String,
}

/// Snapshot of both 5-player teams and their ARAM Mayhem ratings during match loading.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LoadingScreenSnapshot {
    pub game_id: u64,
    pub game_mode: String,
    pub queue_id: Option<i32>,
    pub local_side: LoadingTeamSide,
    #[serde(default)]
    pub local_champion_id: Option<i32>,
    pub team_one: Vec<LoadingPlayerSlot>,
    pub team_two: Vec<LoadingPlayerSlot>,
    pub team_one_ratings: TeamRatingsSnapshot,
    pub team_two_ratings: TeamRatingsSnapshot,
}

/// Core events streamed from `hex-core` (and `hex-vision`) to the UI layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AppEvent {
    ModeChanged(AppMode),
    ChampSelectUpdated(ChampSelectState),
    TeamRatingsUpdated(TeamRatingsSnapshot),
    LoadingScreenUpdated(LoadingScreenSnapshot),
    /// Refreshes the active match's 10-player roster/ratings in-game (e.g. Tab panel) without showing the loading overlay.
    MatchRosterUpdated(LoadingScreenSnapshot),
    LoadingCardOrderDetected {
        game_id: u64,
        team_one_order: Vec<usize>,
        team_two_order: Vec<usize>,
    },
    TabRowOrderDetected {
        game_id: u64,
        left_order: Vec<usize>,
        right_order: Vec<usize>,
    },
    LoadingScreenCleared,
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
