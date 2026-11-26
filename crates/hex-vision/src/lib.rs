//! Windows Graphics Capture (WGC), WinRT OCR text matching, aspect-ratio ROI cropping, and pHash fallback.

pub mod capture;
pub mod ocr;
pub mod phash;
pub mod roi;

pub use capture::{
    mode_supports_augments, AramAugmentGate, VisionConfig, VisionController, VisionGameContext,
};
#[cfg(target_os = "windows")]
pub use ocr::WinOcrRecognizer;
pub use ocr::{AugmentTextMatcher, OcrLineHit, OcrOfferMatch};
pub use phash::{
    compute_phash_from_rgba32, compute_region_phash, hamming_distance, AugmentMatcher,
};
pub use roi::{detect_augment_screen, select_preset, SharedRoiManifest};
