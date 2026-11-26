//! Core state machine, process monitoring, LCU WebSocket, and Live Client Data API.

pub mod lcu;
pub mod live;
pub mod models;
pub mod process;
pub mod state_machine;

pub use lcu::{LcuClient, LcuWsEvent};
pub use live::LiveClient;
pub use models::{
    AppEvent, AppMode, AspectRoiPreset, AugmentOfferLayout, ChampSelectState,
    EndOfGameParticipant, EndOfGameStats, HeroTier, LiveGameState, RoiManifest, RoiRect, TeamSlot,
};
pub use process::{
    is_current_process_elevated, parse_raw_cmdline_credentials, LcuCredentials, ProcessMonitor,
    ProcessSnapshot,
};
pub use state_machine::{spawn_core_service, CoreDiagnostics, CoreHandle, CoreRuntime};

/// Crate identifier helper verifying workspace linkage.
pub fn crate_name() -> &'static str {
    "hex-core"
}
