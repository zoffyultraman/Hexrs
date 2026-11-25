//! Local SQLite read-only meta queries, Zstd hot-update loader, and anonymized EOG reporting.

pub mod db;
pub mod schema;
pub mod seed;

pub use db::{
    bayesian_smoothed_win_rate, data_dir, db_path, AugmentEvaluation, AugmentHashEntry,
    BenchHighlight, BuildItemInfo, DynamicBuildPlan, HeroPreview, HeroStats, MetaEngine,
    SynergyAlert, SynergyRecommendation, TeamDiagnosis, TeamRecommendations,
};
pub use schema::{init_schema, SCHEMA_VERSION};
pub use seed::{lookup_item_name_zh, populate_seed_data};

/// Crate identifier helper verifying workspace linkage.
pub fn crate_name() -> &'static str {
    "hex-data"
}
