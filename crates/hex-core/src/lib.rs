//! Core state machine, process monitoring, LCU WebSocket, and Live Client Data API.

pub mod lcu;
pub mod live;
pub mod models;
pub mod player_rating;
pub mod process;
pub mod sgp;
pub mod state_machine;

pub use lcu::{GameflowSessionSnapshot, LcuClient, LcuWsEvent};
pub use live::LiveClient;
pub use models::{
    AppEvent, AppMode, AspectRoiPreset, AugmentOfferLayout, ChampSelectState,
    EndOfGameParticipant, EndOfGameStats, HeroTier, LiveGameState, LoadingPlayerSlot,
    LoadingScreenSnapshot, LoadingTeamSide, PremadeGroupSummary, RatingTierRule,
    RatingTitlesConfig, RoiManifest, RoiRect, SummonerRatingStatus, SummonerRatingSummary,
    TeamRatingsSnapshot, TeamSlot,
};
pub use player_rating::{
    analyze_team_ratings, is_valid_kiwi_match, score_single_match_performance, MatchFetchResult,
    NormalizedMatch, NormalizedParticipant,
};
pub use process::{
    is_current_process_elevated, parse_raw_cmdline_credentials,
    parse_raw_cmdline_credentials_full, LcuCredentials, ProcessMonitor, ProcessSnapshot,
};
pub use sgp::{get_sgp_match_history_url, get_sgp_server_id, SgpClient};
pub use state_machine::{spawn_core_service, CoreDiagnostics, CoreHandle, CoreRuntime};

/// Crate identifier helper verifying workspace linkage.
pub fn crate_name() -> &'static str {
    "hex-core"
}
