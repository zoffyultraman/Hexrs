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
pub use ocr::{
    align_loading_row_from_ocr, align_tab_column_from_ocr, AugmentTextMatcher,
    LoadingAlignmentTargets, LoadingSlotTarget, OcrLineHit, OcrMatchDiagnostic, OcrOfferMatch,
    TabAlignmentTargets,
};
pub use phash::{
    compute_phash_from_rgba32, compute_region_phash, hamming_distance, AugmentMatcher,
};
pub use roi::{detect_augment_screen, select_preset, SharedRoiManifest};
