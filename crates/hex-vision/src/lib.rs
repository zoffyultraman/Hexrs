//! Windows Graphics Capture (WGC), aspect-ratio ROI cropping, and pHash augment matching.

pub mod capture;
pub mod phash;
pub mod roi;

pub use capture::{mode_supports_augments, VisionConfig, VisionController};
pub use phash::{
    compute_phash_from_rgba32, compute_region_phash, hamming_distance, AugmentMatcher,
};
pub use roi::{detect_augment_screen, select_preset, SharedRoiManifest};
