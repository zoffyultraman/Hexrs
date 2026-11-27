//! Local SQLite read-only meta queries, Zstd hot-update loader, icon cache, and anonymized EOG reporting.

pub mod db;
pub mod icons;
pub mod reporter;
pub mod schema;
pub mod seed;
pub mod updater;

pub use db::{
    bayesian_smoothed_win_rate, data_dir, db_path, AugmentEvaluation, AugmentHashEntry,
    AugmentNameEntry, BenchHighlight, BuildItemInfo, DynamicBuildPlan, HeroPreview, HeroStats,
    MetaEngine, SynergyAlert, SynergyRecommendation, TeamDiagnosis, TeamRecommendations,
    AUGMENT_MIN_HERO_SAMPLES,
};
pub use icons::IconCache;
pub use reporter::{EogReporter, ReporterConfig};
pub use schema::{init_schema, SCHEMA_VERSION};
pub use seed::{lookup_item_name_zh, populate_seed_data};
pub use updater::{
    check_and_apply_update, check_and_apply_update_async, spawn_background_update,
    ManifestPayload, UpdateOutcome, UpdaterConfig, DEFAULT_MANIFEST_URL,
};

/// Crate identifier helper verifying workspace linkage.
pub fn crate_name() -> &'static str {
    "hex-data"
}
