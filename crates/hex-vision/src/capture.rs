//! On-demand Windows Graphics Capture (WGC) session controller (`windows-capture 2.0.1`).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};

use crossbeam_channel::Sender;
use hex_core::AppEvent;
use hex_data::{AugmentHashEntry, AugmentNameEntry};
use tracing::{debug, info, warn};

use crate::ocr::{AugmentTextMatcher, LoadingAlignmentTargets, TabAlignmentTargets};
use crate::phash::AugmentMatcher;
use crate::roi::SharedRoiManifest;

const DEFAULT_MAX_HAMMING_DISTANCE: u32 = 14;
const ARAM_MILESTONES: [u32; 5] = [3, 7, 11, 15, 21];
const FOUNTAIN_WINDOW_SECS: f64 = 45.0;
const MILESTONE_WINDOW_SECS: f64 = 15.0;
const OPENING_WINDOW_SECS: f64 = 120.0;

/// Configuration snapshot controlling when and how WGC augment recognition runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionConfig {
    pub enabled: bool,
    pub game_mode: String,
    pub max_hamming_distance: u32,
}

impl Default for VisionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            game_mode: String::new(),
            max_hamming_distance: DEFAULT_MAX_HAMMING_DISTANCE,
        }
    }
}

/// Live game snapshot fed into `AramAugmentGate` to throttle WGC frame processing outside augment windows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VisionGameContext {
    pub game_mode: String,
    pub level: u32,
    pub game_time_secs: f64,
    pub is_dead: bool,
    pub respawn_timer: f64,
}

/// Gating heuristic opening WGC OCR recognition only when ARAM/Mayhem augment cards can appear.
#[derive(Debug, Clone, Default)]
pub struct AramAugmentGate {
    max_level_seen: u32,
    was_dead: bool,
    last_game_time_secs: f64,
    fountain_window_until: f64,
    milestone_window_until: f64,
    triggered_milestones: [bool; 5],
}

impl AramAugmentGate {
    /// Resets gate state for a new match.
    pub fn reset(&mut self) {
        self.max_level_seen = 0;
        self.was_dead = false;
        self.last_game_time_secs = 0.0;
        self.fountain_window_until = 0.0;
        self.milestone_window_until = 0.0;
        self.triggered_milestones = [false; 5];
    }

    /// Updates state from `ctx` and returns `true` if WGC should read and analyze frames now.
    pub fn update(&mut self, ctx: &VisionGameContext, active_offer: bool) -> bool {
        if ctx.game_time_secs + 5.0 < self.last_game_time_secs
            || (self.max_level_seen > 0 && ctx.level > 0 && ctx.level < self.max_level_seen)
        {
            self.reset();
        }
        self.last_game_time_secs = ctx.game_time_secs;

        let prev_max = self.max_level_seen;
        if prev_max == 0 && ctx.game_time_secs > OPENING_WINDOW_SECS && ctx.level > 0 {
            // Reconnected mid-match after opening; mark earlier milestones so they don't misfire.
            for (idx, &milestone) in ARAM_MILESTONES.iter().enumerate() {
                if milestone < ctx.level {
                    self.triggered_milestones[idx] = true;
                }
            }
        }

        if ctx.level > prev_max {
            for (idx, &milestone) in ARAM_MILESTONES.iter().enumerate() {
                let crossed = if prev_max == 0 && ctx.game_time_secs > OPENING_WINDOW_SECS {
                    ctx.level == milestone
                } else {
                    prev_max < milestone && ctx.level >= milestone
                };
                if crossed && !self.triggered_milestones[idx] {
                    self.triggered_milestones[idx] = true;
                    self.milestone_window_until = self
                        .milestone_window_until
                        .max(ctx.game_time_secs + MILESTONE_WINDOW_SECS);
                }
            }
            self.max_level_seen = ctx.level;
        }

        if ctx.is_dead {
            self.was_dead = true;
            self.fountain_window_until = self
                .fountain_window_until
                .max(ctx.game_time_secs + ctx.respawn_timer.max(0.0) + FOUNTAIN_WINDOW_SECS);
        } else if self.was_dead {
            self.was_dead = false;
            self.fountain_window_until = self
                .fountain_window_until
                .max(ctx.game_time_secs + FOUNTAIN_WINDOW_SECS);
        }

        if ctx.game_mode.trim().eq_ignore_ascii_case("CHERRY") {
            return true;
        }

        active_offer
            || ctx.is_dead
            || ctx.game_time_secs <= OPENING_WINDOW_SECS
            || ctx.game_time_secs <= self.fountain_window_until
            || ctx.game_time_secs <= self.milestone_window_until
    }
}

/// Returns `true` when the active game mode supports hextech augment selection modals.
///
/// Empty `game_mode` returns `true` so 0:00 Level-1 augment choices before Live API responds
/// are never missed; classic Summoner's Rift / TFT / tutorial modes return `false`.
pub fn mode_supports_augments(game_mode: &str) -> bool {
    let upper = game_mode.trim().to_ascii_uppercase();
    if upper.is_empty() {
        return true;
    }
    if matches!(
        upper.as_str(),
        "CLASSIC" | "TFT" | "PRACTICETOOL" | "TUTORIAL"
    ) {
        return false;
    }
    upper.contains("ARAM")
        || upper.contains("KIWI")
        || upper.contains("CHERRY")
        || upper.contains("MAYHEM")
        || upper.contains("HEX")
        || upper.contains("AUGMENT")
        || upper == "ULTBOOK"
}

struct VisionShared {
    matcher: RwLock<AugmentMatcher>,
    text_matcher: RwLock<AugmentTextMatcher>,
    manifest: SharedRoiManifest,
    event_tx: Sender<AppEvent>,
    active: AtomicBool,
    offer_active: AtomicBool,
    gate_open: AtomicBool,
    loading_targets: RwLock<Option<LoadingAlignmentTargets>>,
    loading_targets_gen: AtomicU64,
    loading_scan_pending: AtomicBool,
    tab_targets: RwLock<Option<TabAlignmentTargets>>,
    tab_targets_gen: AtomicU64,
    tab_scan_pending: AtomicBool,
    shutdown: AtomicBool,
    max_distance: AtomicU32,
    game_ctx: RwLock<VisionGameContext>,
    gate: Mutex<AramAugmentGate>,
    last_unsupported_mode_warned: Mutex<Option<String>>,
    wake_lock: Mutex<()>,
    wake_cv: Condvar,
}

impl VisionShared {
    #[cfg(any(target_os = "windows", test))]
    fn should_run_capture(&self) -> bool {
        self.active.load(Ordering::Acquire)
            || self.loading_scan_pending.load(Ordering::Acquire)
            || self.tab_scan_pending.load(Ordering::Acquire)
    }

    fn notify_supervisor(&self) {
        let _guard = self.wake_lock.lock();
        self.wake_cv.notify_all();
    }

    fn set_offer_active(&self, active_offer: bool) {
        self.offer_active.store(active_offer, Ordering::Release);
        self.refresh_gate();
    }

    fn refresh_gate(&self) {
        let active = self.active.load(Ordering::Acquire);
        if !active {
            self.gate_open.store(false, Ordering::Release);
            return;
        }
        let active_offer = self.offer_active.load(Ordering::Acquire);
        let ctx = self
            .game_ctx
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        let open = if let Ok(mut gate) = self.gate.lock() {
            gate.update(&ctx, active_offer)
        } else {
            true
        };
        self.gate_open.store(open, Ordering::Release);
    }
}

/// Controls the lifecycle of the on-demand WGC capture pipeline.
///
/// Hibernates completely with zero GPU/CPU capture overhead outside augment-capable matches.
pub struct VisionController {
    shared: Arc<VisionShared>,
    #[cfg(target_os = "windows")]
    supervisor: Option<std::thread::JoinHandle<()>>,
}

impl VisionController {
    /// Starts the vision controller in dormant state; call `update_game_context` or `set_active` in-game.
    pub fn start(
        matcher_hashes: Vec<AugmentHashEntry>,
        augment_names: Vec<AugmentNameEntry>,
        manifest: SharedRoiManifest,
        event_tx: Sender<AppEvent>,
    ) -> Self {
        let matcher = AugmentMatcher::new(matcher_hashes, DEFAULT_MAX_HAMMING_DISTANCE);
        let text_matcher = AugmentTextMatcher::new(augment_names);
        let shared = Arc::new(VisionShared {
            matcher: RwLock::new(matcher),
            text_matcher: RwLock::new(text_matcher),
            manifest,
            event_tx,
            active: AtomicBool::new(false),
            offer_active: AtomicBool::new(false),
            gate_open: AtomicBool::new(false),
            loading_targets: RwLock::new(None),
            loading_targets_gen: AtomicU64::new(0),
            loading_scan_pending: AtomicBool::new(false),
            tab_targets: RwLock::new(None),
            tab_targets_gen: AtomicU64::new(0),
            tab_scan_pending: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            max_distance: AtomicU32::new(DEFAULT_MAX_HAMMING_DISTANCE),
            game_ctx: RwLock::new(VisionGameContext::default()),
            gate: Mutex::new(AramAugmentGate::default()),
            last_unsupported_mode_warned: Mutex::new(None),
            wake_lock: Mutex::new(()),
            wake_cv: Condvar::new(),
        });

        #[cfg(target_os = "windows")]
        let supervisor = {
            let shared_clone = Arc::clone(&shared);
            match std::thread::Builder::new()
                .name("hex-vision-supervisor".to_owned())
                .spawn(move || win_capture::run_supervisor(shared_clone))
            {
                Ok(handle) => Some(handle),
                Err(err) => {
                    warn!(error = %err, "failed to spawn hex-vision supervisor thread");
                    None
                }
            }
        };

        Self {
            shared,
            #[cfg(target_os = "windows")]
            supervisor,
        }
    }

    /// Updates match context (`level`, `game_time_secs`, `is_dead`, `respawn_timer`) and toggles WGC + ARAM gate.
    pub fn update_game_context(&self, active: bool, ctx: VisionGameContext) {
        let supported = mode_supports_augments(&ctx.game_mode);
        let should_run = active && supported;
        let game_mode_log = ctx.game_mode.clone();
        let level = ctx.level;
        let game_time_secs = ctx.game_time_secs;
        let is_dead = ctx.is_dead;
        let respawn_timer = ctx.respawn_timer;

        if active && !supported {
            if let Ok(mut warned) = self.shared.last_unsupported_mode_warned.lock() {
                if warned.as_deref() != Some(game_mode_log.as_str()) {
                    warn!(
                        game_mode = %game_mode_log,
                        "hex-vision capture disabled: game_mode does not match augment whitelist"
                    );
                    *warned = Some(game_mode_log.clone());
                }
            }
        } else if let Ok(mut warned) = self.shared.last_unsupported_mode_warned.lock() {
            *warned = None;
        }

        if let Ok(mut guard) = self.shared.game_ctx.write() {
            *guard = ctx;
        }

        let prev = self.shared.active.swap(should_run, Ordering::AcqRel);
        let prev_gate = self.shared.gate_open.load(Ordering::Acquire);
        if !should_run {
            self.shared.offer_active.store(false, Ordering::Release);
            if let Ok(mut gate) = self.shared.gate.lock() {
                gate.reset();
            }
        }
        self.shared.refresh_gate();
        let new_gate = self.shared.gate_open.load(Ordering::Acquire);

        if prev != should_run || prev_gate != new_gate {
            info!(
                active = should_run,
                game_mode = %game_mode_log,
                level,
                game_time_secs,
                is_dead,
                respawn_timer,
                gate_open = new_gate,
                "hex-vision capture/gate state changed"
            );
            if prev != should_run {
                if !should_run {
                    let _ = self.shared.event_tx.send(AppEvent::AugmentOfferCleared);
                }
                self.shared.notify_supervisor();
            }
        } else {
            debug!(
                active = should_run,
                game_mode = %game_mode_log,
                gate_open = new_gate,
                "hex-vision context updated"
            );
        }
    }

    /// Backwards-compatible helper toggling capture active state using only `game_mode`.
    pub fn set_active(&self, active: bool, game_mode: &str) {
        let mut ctx = self
            .shared
            .game_ctx
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        ctx.game_mode = game_mode.to_owned();
        if !active {
            ctx = VisionGameContext::default();
        }
        self.update_game_context(active, ctx);
    }

    /// Updates the in-memory augment pHash dictionary (e.g., after a `meta.db` reload).
    pub fn update_hashes(&self, hashes: Vec<AugmentHashEntry>) {
        let max_dist = self.shared.max_distance.load(Ordering::Acquire);
        if let Ok(mut matcher) = self.shared.matcher.write() {
            matcher.update_entries(hashes);
            matcher.set_max_distance(max_dist);
        }
    }

    /// Updates the in-memory augment OCR name dictionary (e.g., after a `meta.db` reload).
    pub fn update_augment_names(&self, names: Vec<AugmentNameEntry>) {
        if let Ok(mut matcher) = self.shared.text_matcher.write() {
            matcher.update_entries(names);
        }
    }

    /// Updates the Hamming distance threshold (`0..=64`).
    pub fn set_max_hamming_distance(&self, max_distance: u32) {
        self.shared
            .max_distance
            .store(max_distance, Ordering::Release);
        if let Ok(mut matcher) = self.shared.matcher.write() {
            matcher.set_max_distance(max_distance);
        }
    }

    /// Sets or clears loading-screen card alignment targets.
    ///
    /// Deduplicates identical target rosters so repeated rating updates during loading do not
    /// reset alignment attempts or unlock a locked alignment. Never logs player/target text.
    pub fn set_loading_targets(&self, targets: Option<LoadingAlignmentTargets>) {
        let normalized = targets.filter(|t| !t.team_one.is_empty() || !t.team_two.is_empty());
        let changed = if let Ok(mut guard) = self.shared.loading_targets.write() {
            if *guard == normalized {
                false
            } else {
                *guard = normalized.clone();
                true
            }
        } else {
            false
        };

        if !changed {
            return;
        }

        let pending = normalized.is_some();
        self.shared
            .loading_targets_gen
            .fetch_add(1, Ordering::AcqRel);
        self.shared
            .loading_scan_pending
            .store(pending, Ordering::Release);

        if let Some(ref t) = normalized {
            debug!(
                game_id = t.game_id,
                team_one_len = t.team_one.len(),
                team_two_len = t.team_two.len(),
                "updated loading screen OCR alignment targets"
            );
        } else {
            debug!("cleared loading screen OCR alignment targets");
        }

        self.shared.notify_supervisor();
    }

    /// Sets or clears in-game Tab scoreboard row alignment targets.
    ///
    /// Deduplicates identical active `Some` targets so holding Tab retains its single-frame lock;
    /// passing `None` clears the targets and lets WGC hibernate when augment scanning is inactive.
    /// Never logs player/target names.
    pub fn set_tab_targets(&self, targets: Option<TabAlignmentTargets>) {
        let normalized = targets.filter(|t| !t.left_names.is_empty() || !t.right_names.is_empty());
        let changed = if let Ok(mut guard) = self.shared.tab_targets.write() {
            if *guard == normalized {
                false
            } else {
                *guard = normalized.clone();
                true
            }
        } else {
            false
        };

        if !changed {
            return;
        }

        let pending = normalized.is_some();
        self.shared.tab_targets_gen.fetch_add(1, Ordering::AcqRel);
        self.shared
            .tab_scan_pending
            .store(pending, Ordering::Release);

        if let Some(ref t) = normalized {
            debug!(
                game_id = t.game_id,
                left_len = t.left_names.len(),
                right_len = t.right_names.len(),
                "updated tab scoreboard OCR alignment targets"
            );
        } else {
            debug!("cleared tab scoreboard OCR alignment targets");
        }

        self.shared.notify_supervisor();
    }

    /// Stops any active capture session and shuts down the supervisor thread.
    pub fn shutdown(&mut self) {
        self.shared.active.store(false, Ordering::Release);
        self.shared.offer_active.store(false, Ordering::Release);
        self.shared.gate_open.store(false, Ordering::Release);
        self.shared
            .loading_scan_pending
            .store(false, Ordering::Release);
        if let Ok(mut guard) = self.shared.loading_targets.write() {
            *guard = None;
        }
        self.shared
            .loading_targets_gen
            .fetch_add(1, Ordering::AcqRel);
        self.shared
            .tab_scan_pending
            .store(false, Ordering::Release);
        if let Ok(mut guard) = self.shared.tab_targets.write() {
            *guard = None;
        }
        self.shared.tab_targets_gen.fetch_add(1, Ordering::AcqRel);
        self.shared.shutdown.store(true, Ordering::Release);
        self.shared.notify_supervisor();

        #[cfg(target_os = "windows")]
        if let Some(handle) = self.supervisor.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for VisionController {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(target_os = "windows")]
mod win_capture {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use hex_core::{AppEvent, AugmentOfferLayout};
    use tracing::{debug, info, warn};
    use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
    use windows_capture::frame::Frame;
    use windows_capture::graphics_capture_api::InternalCaptureControl;
    use windows_capture::settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    };
    use windows_capture::window::Window as CaptureWindow;

    use super::VisionShared;
    use crate::ocr::{
        align_loading_row_from_ocr, align_tab_column_from_ocr, LoadingAlignmentTargets,
        TabAlignmentTargets, WinOcrRecognizer,
    };
    use crate::roi::{detect_augment_screen, select_preset};

    const FRAME_PROCESS_INTERVAL: Duration = Duration::from_millis(400);
    const LOADING_FRAME_INTERVAL: Duration = Duration::from_millis(500);
    const LOADING_TOP_BAND_Y_START: f32 = 0.368;
    const LOADING_TOP_BAND_Y_END: f32 = 0.485;
    const LOADING_BOTTOM_BAND_Y_START: f32 = 0.855;
    const LOADING_BOTTOM_BAND_Y_END: f32 = 0.965;
    const LOADING_MAX_ATTEMPTS: u8 = 12;
    const TAB_FRAME_INTERVAL: Duration = Duration::from_millis(180);
    const TAB_BAND_Y_START: f32 = 0.282;
    const TAB_BAND_Y_END: f32 = 0.638;
    const TAB_MAX_ATTEMPTS: u8 = 5;
    const OFFER_DEDUP_COOLDOWN: Duration = Duration::from_secs(4);
    const CLEAR_AFTER_MISSES: u8 = 3;
    const WINDOW_RETRY_INTERVAL: Duration = Duration::from_millis(1000);
    const ACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(250);
    const FALLBACK_MAX_DISTANCE: u32 = 8;
    const OCR_ERR_LOG_INTERVAL: Duration = Duration::from_secs(10);
    const PARTIAL_DIAG_LOG_INTERVAL: Duration = Duration::from_secs(5);
    const HEARTBEAT_LOG_INTERVAL: Duration = Duration::from_secs(15);
    const WINDOW_MISSING_WARN_DELAY: Duration = Duration::from_secs(5);
    const WINDOW_MISSING_WARN_INTERVAL: Duration = Duration::from_secs(30);

    type BoxError = Box<dyn std::error::Error + Send + Sync>;

    #[derive(Default)]
    struct LoadingAlignmentState {
        generation: u64,
        targets: Option<LoadingAlignmentTargets>,
        best_team_one: Option<(Vec<usize>, usize, i32)>,
        best_team_two: Option<(Vec<usize>, usize, i32)>,
        attempts: u8,
        locked: bool,
    }

    impl LoadingAlignmentState {
        fn reset_for(&mut self, generation: u64, targets: Option<LoadingAlignmentTargets>) {
            self.generation = generation;
            self.targets = targets;
            self.best_team_one = None;
            self.best_team_two = None;
            self.attempts = 0;
            self.locked = false;
        }
    }

    #[derive(Default)]
    struct TabAlignmentState {
        generation: u64,
        targets: Option<TabAlignmentTargets>,
        best_left: Option<(Vec<usize>, usize, i32)>,
        best_right: Option<(Vec<usize>, usize, i32)>,
        attempts: u8,
        locked: bool,
    }

    impl TabAlignmentState {
        fn reset_for(&mut self, generation: u64, targets: Option<TabAlignmentTargets>) {
            self.generation = generation;
            self.targets = targets;
            self.best_left = None;
            self.best_right = None;
            self.attempts = 0;
            self.locked = false;
        }
    }

    struct HexCaptureHandler {
        shared: Arc<VisionShared>,
        ocr_recognizer: Option<WinOcrRecognizer>,
        scratch_buf: Vec<u8>,
        bgra_scratch: Vec<u8>,
        last_processed: Instant,
        last_loading_processed: Instant,
        loading_state: LoadingAlignmentState,
        last_tab_processed: Instant,
        tab_state: TabAlignmentState,
        last_emitted: Option<(Vec<i32>, Instant)>,
        last_frame_dims: Option<(u32, u32)>,
        last_ocr_err_log: Option<Instant>,
        last_partial_diag_log: Option<Instant>,
        last_heartbeat_log: Option<Instant>,
        active_offer: bool,
        consecutive_misses: u8,
    }

    fn try_strict_phash_match(
        shared: &VisionShared,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> Option<(Vec<i32>, u64, Option<AugmentOfferLayout>)> {
        let manifest = shared.manifest.read().ok()?.clone();
        let preset = select_preset(&manifest, width, height);
        if !detect_augment_screen(rgba, width, height, preset) {
            return None;
        }
        let mut matcher = shared.matcher.read().ok()?.clone();
        let strict_dist = shared
            .max_distance
            .load(Ordering::Acquire)
            .min(FALLBACK_MAX_DISTANCE);
        matcher.set_max_distance(strict_dist);
        matcher
            .match_offer(rgba, width, height, &preset.card_icons)
            .map(|(candidates, ms)| (candidates, ms, None))
    }

    impl GraphicsCaptureApiHandler for HexCaptureHandler {
        type Flags = Arc<VisionShared>;
        type Error = BoxError;

        fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
            let ocr_recognizer = WinOcrRecognizer::new();
            if let Some(ref ocr) = ocr_recognizer {
                info!(
                    ocr_lang = %ocr.language_tag(),
                    available_langs = ?ocr.available_languages(),
                    "WinRT OcrEngine initialized on WGC capture thread"
                );
                if !ocr.is_chinese() {
                    warn!(
                        ocr_lang = %ocr.language_tag(),
                        available_langs = ?ocr.available_languages(),
                        "WinRT OcrEngine is using a non-Chinese language; Chinese augment OCR cannot match, will fall back to strict ROI pHash"
                    );
                }
            } else {
                warn!("WinRT OcrEngine unavailable; falling back to strict ROI pHash");
            }

            Ok(Self {
                shared: ctx.flags,
                ocr_recognizer,
                scratch_buf: Vec::new(),
                bgra_scratch: Vec::new(),
                last_processed: Instant::now() - FRAME_PROCESS_INTERVAL,
                last_loading_processed: Instant::now() - LOADING_FRAME_INTERVAL,
                loading_state: LoadingAlignmentState::default(),
                last_tab_processed: Instant::now() - TAB_FRAME_INTERVAL,
                tab_state: TabAlignmentState::default(),
                last_emitted: None,
                last_frame_dims: None,
                last_ocr_err_log: None,
                last_partial_diag_log: None,
                last_heartbeat_log: None,
                active_offer: false,
                consecutive_misses: 0,
            })
        }

        fn on_frame_arrived(
            &mut self,
            frame: &mut Frame,
            capture_control: InternalCaptureControl,
        ) -> Result<(), Self::Error> {
            if !self.shared.should_run_capture()
                || self.shared.shutdown.load(Ordering::Acquire)
            {
                if self.active_offer {
                    self.active_offer = false;
                    self.consecutive_misses = 0;
                    self.last_emitted = None;
                    self.shared.set_offer_active(false);
                    let _ = self.shared.event_tx.send(AppEvent::AugmentOfferCleared);
                }
                capture_control.stop();
                return Ok(());
            }

            let targets_gen = self.shared.loading_targets_gen.load(Ordering::Acquire);
            if targets_gen != self.loading_state.generation {
                let next_targets = self
                    .shared
                    .loading_targets
                    .read()
                    .ok()
                    .and_then(|g| g.clone());
                self.loading_state.reset_for(targets_gen, next_targets);
                self.last_loading_processed = Instant::now() - LOADING_FRAME_INTERVAL;
            }

            if self.loading_state.targets.is_some() {
                if self.loading_state.locked {
                    return Ok(());
                }
                if self.last_loading_processed.elapsed() < LOADING_FRAME_INTERVAL {
                    return Ok(());
                }
                self.last_loading_processed = Instant::now();

                if self.ocr_recognizer.is_none() {
                    let game_id = self
                        .loading_state
                        .targets
                        .as_ref()
                        .map(|t| t.game_id)
                        .unwrap_or(0);
                    warn!(
                        game_id,
                        "WinRT OcrEngine unavailable; skipping loading screen card alignment"
                    );
                    self.loading_state.locked = true;
                    self.shared
                        .loading_scan_pending
                        .store(false, Ordering::Release);
                    return Ok(());
                }

                let (t1_res, t2_res) = {
                    let (Some(targets), Some(ocr)) = (
                        self.loading_state.targets.as_ref(),
                        self.ocr_recognizer.as_ref(),
                    ) else {
                        return Ok(());
                    };

                    let buffer = frame.buffer()?;
                    let width = buffer.width();
                    let height = buffer.height();
                    let rgba = buffer.as_nopadding_buffer(&mut self.scratch_buf);

                    if self.last_frame_dims != Some((width, height)) {
                        self.last_frame_dims = Some((width, height));
                        info!(
                            width,
                            height,
                            ocr_lang = %ocr.language_tag(),
                            "WGC capture frame stream active"
                        );
                    }

                    let t1 = if !targets.team_one.is_empty() {
                        ocr.recognize_band(
                            rgba,
                            width,
                            height,
                            LOADING_TOP_BAND_Y_START,
                            LOADING_TOP_BAND_Y_END,
                            &mut self.bgra_scratch,
                        )
                        .and_then(|lines| {
                            align_loading_row_from_ocr(&lines, &targets.team_one, width, height)
                        })
                    } else {
                        None
                    };

                    let t2 = if !targets.team_two.is_empty() {
                        ocr.recognize_band(
                            rgba,
                            width,
                            height,
                            LOADING_BOTTOM_BAND_Y_START,
                            LOADING_BOTTOM_BAND_Y_END,
                            &mut self.bgra_scratch,
                        )
                        .and_then(|lines| {
                            align_loading_row_from_ocr(&lines, &targets.team_two, width, height)
                        })
                    } else {
                        None
                    };

                    (t1, t2)
                };

                self.loading_state.attempts = self.loading_state.attempts.saturating_add(1);

                let mut improved = false;
                if let Some((order, matched_cols, score)) = t1_res {
                    let is_better = self
                        .loading_state
                        .best_team_one
                        .as_ref()
                        .map(|(_, prev_cols, prev_score)| {
                            matched_cols > *prev_cols
                                || (matched_cols == *prev_cols && score > *prev_score)
                        })
                        .unwrap_or(true);
                    if is_better {
                        self.loading_state.best_team_one = Some((order, matched_cols, score));
                        improved = true;
                    }
                }
                if let Some((order, matched_cols, score)) = t2_res {
                    let is_better = self
                        .loading_state
                        .best_team_two
                        .as_ref()
                        .map(|(_, prev_cols, prev_score)| {
                            matched_cols > *prev_cols
                                || (matched_cols == *prev_cols && score > *prev_score)
                        })
                        .unwrap_or(true);
                    if is_better {
                        self.loading_state.best_team_two = Some((order, matched_cols, score));
                        improved = true;
                    }
                }

                let (game_id, t1_len, t2_len) = self
                    .loading_state
                    .targets
                    .as_ref()
                    .map(|t| (t.game_id, t.team_one.len(), t.team_two.len()))
                    .unwrap_or((0, 0, 0));
                let t1_matched = self
                    .loading_state
                    .best_team_one
                    .as_ref()
                    .map(|(_, m, _)| *m)
                    .unwrap_or(0);
                let t2_matched = self
                    .loading_state
                    .best_team_two
                    .as_ref()
                    .map(|(_, m, _)| *m)
                    .unwrap_or(0);

                if improved {
                    let team_one_order = self
                        .loading_state
                        .best_team_one
                        .as_ref()
                        .map(|(o, _, _)| o.clone())
                        .unwrap_or_else(|| (0..t1_len).collect());
                    let team_two_order = self
                        .loading_state
                        .best_team_two
                        .as_ref()
                        .map(|(o, _, _)| o.clone())
                        .unwrap_or_else(|| (0..t2_len).collect());
                    info!(
                        game_id,
                        ?team_one_order,
                        ?team_two_order,
                        team_one_matched = t1_matched,
                        team_two_matched = t2_matched,
                        attempts = self.loading_state.attempts,
                        "detected loading screen card order"
                    );
                    let _ = self.shared.event_tx.send(AppEvent::LoadingCardOrderDetected {
                        game_id,
                        team_one_order,
                        team_two_order,
                    });
                }

                let t1_ready = t1_len == 0 || t1_matched >= t1_len.min(3);
                let t2_ready = t2_len == 0 || t2_matched >= t2_len.min(3);
                if (t1_ready && t2_ready) || self.loading_state.attempts >= LOADING_MAX_ATTEMPTS {
                    self.loading_state.locked = true;
                    self.shared
                        .loading_scan_pending
                        .store(false, Ordering::Release);
                    info!(
                        game_id,
                        team_one_matched = t1_matched,
                        team_two_matched = t2_matched,
                        attempts = self.loading_state.attempts,
                        "loading screen card alignment locked"
                    );
                }

                return Ok(());
            }

            let tab_gen = self.shared.tab_targets_gen.load(Ordering::Acquire);
            if tab_gen != self.tab_state.generation {
                let next_targets = self
                    .shared
                    .tab_targets
                    .read()
                    .ok()
                    .and_then(|g| g.clone());
                self.tab_state.reset_for(tab_gen, next_targets);
                self.last_tab_processed = Instant::now() - TAB_FRAME_INTERVAL;
            }

            if self.tab_state.targets.is_some() {
                if self.tab_state.locked {
                    return Ok(());
                }
                if self.last_tab_processed.elapsed() < TAB_FRAME_INTERVAL {
                    return Ok(());
                }
                self.last_tab_processed = Instant::now();

                let has_chinese_ocr = self
                    .ocr_recognizer
                    .as_ref()
                    .map(|o| o.is_chinese())
                    .unwrap_or(false);
                if !has_chinese_ocr {
                    let game_id = self
                        .tab_state
                        .targets
                        .as_ref()
                        .map(|t| t.game_id)
                        .unwrap_or(0);
                    warn!(
                        game_id,
                        "Chinese WinRT OcrEngine unavailable; skipping Tab scoreboard row alignment"
                    );
                    self.tab_state.locked = true;
                    self.shared
                        .tab_scan_pending
                        .store(false, Ordering::Release);
                    return Ok(());
                }

                let (left_res, right_res) = {
                    let (Some(targets), Some(ocr)) = (
                        self.tab_state.targets.as_ref(),
                        self.ocr_recognizer.as_ref(),
                    ) else {
                        return Ok(());
                    };

                    let buffer = frame.buffer()?;
                    let width = buffer.width();
                    let height = buffer.height();
                    let rgba = buffer.as_nopadding_buffer(&mut self.scratch_buf);

                    if self.last_frame_dims != Some((width, height)) {
                        self.last_frame_dims = Some((width, height));
                        info!(
                            width,
                            height,
                            ocr_lang = %ocr.language_tag(),
                            "WGC capture frame stream active"
                        );
                    }

                    let w = width.max(1) as f32;
                    let h = height.max(1) as f32;
                    let left_x0 = ((0.5 * w - 0.30 * h) / w).clamp(0.0, 0.5);
                    let left_x1 = ((0.5 * w - 0.002 * h) / w).clamp(0.0, 0.5);
                    let right_x0 = ((0.5 * w + 0.24 * h) / w).clamp(0.5, 1.0);
                    let right_x1 = ((0.5 * w + 0.52 * h) / w).clamp(0.5, 1.0);

                    let left = if !targets.left_names.is_empty() && left_x1 > left_x0 {
                        ocr.recognize_rect_scaled(
                            rgba,
                            width,
                            height,
                            left_x0,
                            TAB_BAND_Y_START,
                            left_x1,
                            TAB_BAND_Y_END,
                            true,
                            &mut self.bgra_scratch,
                        )
                        .and_then(|lines| align_tab_column_from_ocr(&lines, &targets.left_names))
                    } else {
                        None
                    };

                    let right = if !targets.right_names.is_empty() && right_x1 > right_x0 {
                        ocr.recognize_rect_scaled(
                            rgba,
                            width,
                            height,
                            right_x0,
                            TAB_BAND_Y_START,
                            right_x1,
                            TAB_BAND_Y_END,
                            true,
                            &mut self.bgra_scratch,
                        )
                        .and_then(|lines| align_tab_column_from_ocr(&lines, &targets.right_names))
                    } else {
                        None
                    };

                    (left, right)
                };

                self.tab_state.attempts = self.tab_state.attempts.saturating_add(1);

                let mut improved = false;
                if let Some((order, matched_rows, score)) = left_res {
                    let is_better = self
                        .tab_state
                        .best_left
                        .as_ref()
                        .map(|(_, prev_rows, prev_score)| {
                            matched_rows > *prev_rows
                                || (matched_rows == *prev_rows && score > *prev_score)
                        })
                        .unwrap_or(true);
                    if is_better {
                        self.tab_state.best_left = Some((order, matched_rows, score));
                        improved = true;
                    }
                }
                if let Some((order, matched_rows, score)) = right_res {
                    let is_better = self
                        .tab_state
                        .best_right
                        .as_ref()
                        .map(|(_, prev_rows, prev_score)| {
                            matched_rows > *prev_rows
                                || (matched_rows == *prev_rows && score > *prev_score)
                        })
                        .unwrap_or(true);
                    if is_better {
                        self.tab_state.best_right = Some((order, matched_rows, score));
                        improved = true;
                    }
                }

                let (game_id, left_len, right_len) = self
                    .tab_state
                    .targets
                    .as_ref()
                    .map(|t| (t.game_id, t.left_names.len(), t.right_names.len()))
                    .unwrap_or((0, 0, 0));
                let left_matched = self
                    .tab_state
                    .best_left
                    .as_ref()
                    .map(|(_, m, _)| *m)
                    .unwrap_or(0);
                let right_matched = self
                    .tab_state
                    .best_right
                    .as_ref()
                    .map(|(_, m, _)| *m)
                    .unwrap_or(0);

                if improved {
                    let left_order = self
                        .tab_state
                        .best_left
                        .as_ref()
                        .map(|(o, _, _)| o.clone())
                        .unwrap_or_else(|| (0..left_len).collect());
                    let right_order = self
                        .tab_state
                        .best_right
                        .as_ref()
                        .map(|(o, _, _)| o.clone())
                        .unwrap_or_else(|| (0..right_len).collect());
                    info!(
                        game_id,
                        ?left_order,
                        ?right_order,
                        left_matched,
                        right_matched,
                        attempts = self.tab_state.attempts,
                        "detected Tab scoreboard row order"
                    );
                    let _ = self.shared.event_tx.send(AppEvent::TabRowOrderDetected {
                        game_id,
                        left_order,
                        right_order,
                    });
                }

                let left_ready = left_len == 0 || left_matched >= left_len.min(3);
                let right_ready = right_len == 0 || right_matched >= right_len.min(3);
                if (left_ready && right_ready) || self.tab_state.attempts >= TAB_MAX_ATTEMPTS {
                    self.tab_state.locked = true;
                    self.shared
                        .tab_scan_pending
                        .store(false, Ordering::Release);
                    info!(
                        game_id,
                        left_matched,
                        right_matched,
                        attempts = self.tab_state.attempts,
                        "Tab scoreboard row alignment locked"
                    );
                }

                return Ok(());
            }

            if !self.shared.active.load(Ordering::Acquire) {
                return Ok(());
            }

            if self.last_processed.elapsed() < FRAME_PROCESS_INTERVAL {
                return Ok(());
            }

            // Skip GPU buffer mapping completely when the ARAM level/fountain gate is closed.
            if !self.active_offer && !self.shared.gate_open.load(Ordering::Acquire) {
                return Ok(());
            }

            self.last_processed = Instant::now();

            let buffer = frame.buffer()?;
            let width = buffer.width();
            let height = buffer.height();
            let rgba = buffer.as_nopadding_buffer(&mut self.scratch_buf);

            if self.last_frame_dims != Some((width, height)) {
                self.last_frame_dims = Some((width, height));
                let ocr_lang = self
                    .ocr_recognizer
                    .as_ref()
                    .map(|o| o.language_tag())
                    .unwrap_or("none");
                info!(width, height, ocr_lang, "WGC capture frame stream active");
            }

            let match_result = if let Some(ocr) = self.ocr_recognizer.as_ref() {
                let start = Instant::now();
                let hits = ocr.recognize_augment_region(
                    rgba,
                    width,
                    height,
                    &mut self.bgra_scratch,
                );
                let elapsed_ms = start.elapsed().as_millis() as u64;
                let ocr_lang = ocr.language_tag().to_owned();
                let is_chinese = ocr.is_chinese();

                match hits {
                    None => {
                        if self
                            .last_ocr_err_log
                            .map(|t| t.elapsed() >= OCR_ERR_LOG_INTERVAL)
                            .unwrap_or(true)
                        {
                            warn!(
                                width,
                                height,
                                %ocr_lang,
                                "WinRT OCR recognize_augment_region returned None"
                            );
                            self.last_ocr_err_log = Some(Instant::now());
                        }
                        if !is_chinese {
                            try_strict_phash_match(&self.shared, rgba, width, height)
                                .map(|(c, ms, l)| (c, ms, l, "phash_fallback"))
                        } else {
                            None
                        }
                    }
                    Some(lines) => {
                        let ocr_matched = self.shared.text_matcher.read().ok().and_then(|tm| {
                            tm.match_ocr_lines_to_offer_with_layout(&lines).map(|m| {
                                (m.candidates, elapsed_ms, Some(m.layout), "winrt_ocr")
                            })
                        });

                        if ocr_matched.is_some() {
                            ocr_matched
                        } else {
                            let phash_matched = if !is_chinese {
                                try_strict_phash_match(&self.shared, rgba, width, height)
                                    .map(|(c, ms, l)| (c, ms, l, "phash_fallback"))
                            } else {
                                None
                            };

                            if phash_matched.is_none() {
                                let can_log_partial = self
                                    .last_partial_diag_log
                                    .map(|t| t.elapsed() >= PARTIAL_DIAG_LOG_INTERVAL)
                                    .unwrap_or(true);
                                let can_log_heartbeat = self
                                    .last_heartbeat_log
                                    .map(|t| t.elapsed() >= HEARTBEAT_LOG_INTERVAL)
                                    .unwrap_or(true);
                                if can_log_partial || can_log_heartbeat {
                                    if let Ok(tm) = self.shared.text_matcher.read() {
                                        let diag = tm.diagnose_ocr_lines(&lines);
                                        if !diag.matched_hits.is_empty() && can_log_partial {
                                            warn!(
                                                width,
                                                height,
                                                total_lines = diag.total_lines,
                                                matched_hits = ?diag.matched_hits,
                                                row_unmatched = ?diag.row_unmatched_texts,
                                                reject_reason = diag.reject_reason,
                                                "augment OCR partial match rejected"
                                            );
                                            self.last_partial_diag_log = Some(Instant::now());
                                        } else if can_log_heartbeat {
                                            info!(
                                                width,
                                                height,
                                                total_lines = diag.total_lines,
                                                reject_reason = diag.reject_reason,
                                                %ocr_lang,
                                                "WGC augment scan heartbeat (no offer matched)"
                                            );
                                            self.last_heartbeat_log = Some(Instant::now());
                                        }
                                    }
                                }
                            }

                            phash_matched
                        }
                    }
                }
            } else {
                let phash_matched = try_strict_phash_match(&self.shared, rgba, width, height)
                    .map(|(c, ms, l)| (c, ms, l, "phash_fallback"));
                if phash_matched.is_none()
                    && self
                        .last_heartbeat_log
                        .map(|t| t.elapsed() >= HEARTBEAT_LOG_INTERVAL)
                        .unwrap_or(true)
                {
                    info!(
                        width,
                        height,
                        ocr_lang = "none",
                        "WGC augment scan heartbeat (no offer matched via pHash fallback)"
                    );
                    self.last_heartbeat_log = Some(Instant::now());
                }
                phash_matched
            };

            if let Some((candidates, capture_ms, layout, source)) = match_result {
                self.consecutive_misses = 0;
                self.active_offer = true;
                self.shared.set_offer_active(true);

                let is_duplicate = self
                    .last_emitted
                    .as_ref()
                    .map(|(prev, at)| *prev == candidates && at.elapsed() < OFFER_DEDUP_COOLDOWN)
                    .unwrap_or(false);

                if !is_duplicate {
                    info!(
                        ?candidates,
                        capture_ms,
                        ?layout,
                        width,
                        height,
                        source,
                        "detected hextech augment offer"
                    );
                    let _ = self.shared.event_tx.send(AppEvent::AugmentOfferDetected {
                        candidates: candidates.clone(),
                        capture_ms,
                        layout,
                    });
                    self.last_emitted = Some((candidates, Instant::now()));
                }
            } else if self.active_offer {
                self.consecutive_misses = self.consecutive_misses.saturating_add(1);
                if self.consecutive_misses >= CLEAR_AFTER_MISSES {
                    self.active_offer = false;
                    self.consecutive_misses = 0;
                    self.last_emitted = None;
                    self.shared.set_offer_active(false);
                    info!("augment offer modal dismissed; clearing strip");
                    let _ = self.shared.event_tx.send(AppEvent::AugmentOfferCleared);
                }
            }

            Ok(())
        }

        fn on_closed(&mut self) -> Result<(), Self::Error> {
            info!("WGC capture session closed by game window");
            Ok(())
        }
    }

    fn find_league_game_window() -> Option<CaptureWindow> {
        CaptureWindow::from_name("League of Legends (TM) Client")
            .ok()
            .or_else(|| CaptureWindow::from_contains_name("League of Legends (TM)").ok())
    }

    fn stop_control(control: Option<CaptureControl<HexCaptureHandler, BoxError>>) {
        if let Some(ctrl) = control {
            if let Err(err) = ctrl.stop() {
                debug!(error = %err, "WGC capture stop returned error");
            }
        }
    }

    pub fn run_supervisor(shared: Arc<VisionShared>) {
        let mut active_control: Option<CaptureControl<HexCaptureHandler, BoxError>> = None;
        let mut window_missing_since: Option<Instant> = None;
        let mut last_window_missing_warn: Option<Instant> = None;

        while !shared.shutdown.load(Ordering::Acquire) {
            let should_run = shared.should_run_capture();

            if !should_run {
                window_missing_since = None;
                last_window_missing_warn = None;
                if active_control.is_some() {
                    info!("hibernating WGC capture session");
                    stop_control(active_control.take());
                }
                let guard = match shared.wake_lock.lock() {
                    Ok(g) => g,
                    Err(_) => break,
                };
                if !shared.shutdown.load(Ordering::Acquire) && !shared.should_run_capture() {
                    drop(shared.wake_cv.wait(guard));
                }
                continue;
            }

            if active_control
                .as_ref()
                .map(|c| c.is_finished())
                .unwrap_or(false)
            {
                stop_control(active_control.take());
            }

            if active_control.is_none() {
                if let Some(window) = find_league_game_window() {
                    window_missing_since = None;
                    last_window_missing_warn = None;
                    let settings = Settings::new(
                        window,
                        CursorCaptureSettings::Default,
                        DrawBorderSettings::WithoutBorder,
                        SecondaryWindowSettings::Default,
                        MinimumUpdateIntervalSettings::Default,
                        DirtyRegionSettings::Default,
                        ColorFormat::Rgba8,
                        Arc::clone(&shared),
                    );
                    match HexCaptureHandler::start_free_threaded(settings) {
                        Ok(control) => {
                            info!("started WGC capture on League of Legends game window");
                            active_control = Some(control);
                        }
                        Err(err) => {
                            warn!(error = %err, "failed to start WGC capture; retrying");
                        }
                    }
                } else {
                    let now = Instant::now();
                    let since = *window_missing_since.get_or_insert(now);
                    if now.saturating_duration_since(since) >= WINDOW_MISSING_WARN_DELAY
                        && last_window_missing_warn
                            .map(|t| now.saturating_duration_since(t) >= WINDOW_MISSING_WARN_INTERVAL)
                            .unwrap_or(true)
                    {
                        warn!("WGC active but game window 'League of Legends (TM)' not found for >=5s; retrying");
                        last_window_missing_warn = Some(now);
                    }
                }
            }

            let wait_dur = if active_control.is_some() {
                ACTIVE_POLL_INTERVAL
            } else {
                WINDOW_RETRY_INTERVAL
            };
            if let Ok(guard) = shared.wake_lock.lock() {
                drop(shared.wake_cv.wait_timeout(guard, wait_dur));
            }
        }

        stop_control(active_control.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aram_gate_opens_on_opening_death_respawn_and_milestones_and_closes_outside() {
        let mut gate = AramAugmentGate::default();

        // Opening window (<= 120s) is open.
        assert!(gate.update(
            &VisionGameContext {
                game_mode: "KIWI".to_owned(),
                level: 3,
                game_time_secs: 30.0,
                is_dead: false,
                respawn_timer: 0.0,
            },
            false,
        ));

        // Mid-game outside any window -> closed.
        assert!(!gate.update(
            &VisionGameContext {
                game_mode: "KIWI".to_owned(),
                level: 6,
                game_time_secs: 200.0,
                is_dead: false,
                respawn_timer: 0.0,
            },
            false,
        ));

        // Crossing level 7 opens a 15s milestone window.
        assert!(gate.update(
            &VisionGameContext {
                game_mode: "KIWI".to_owned(),
                level: 7,
                game_time_secs: 220.0,
                is_dead: false,
                respawn_timer: 0.0,
            },
            false,
        ));
        // After 15s -> closed again.
        assert!(!gate.update(
            &VisionGameContext {
                game_mode: "KIWI".to_owned(),
                level: 7,
                game_time_secs: 236.0,
                is_dead: false,
                respawn_timer: 0.0,
            },
            false,
        ));

        // Death opens immediately and respawn keeps 45s fountain window open.
        assert!(gate.update(
            &VisionGameContext {
                game_mode: "KIWI".to_owned(),
                level: 8,
                game_time_secs: 300.0,
                is_dead: true,
                respawn_timer: 10.0,
            },
            false,
        ));
        assert!(gate.update(
            &VisionGameContext {
                game_mode: "KIWI".to_owned(),
                level: 8,
                game_time_secs: 340.0,
                is_dead: false,
                respawn_timer: 0.0,
            },
            false,
        ));
        // Beyond respawn + 45s -> closed again unless an offer is active.
        assert!(!gate.update(
            &VisionGameContext {
                game_mode: "KIWI".to_owned(),
                level: 8,
                game_time_secs: 390.0,
                is_dead: false,
                respawn_timer: 0.0,
            },
            false,
        ));
        assert!(gate.update(
            &VisionGameContext {
                game_mode: "KIWI".to_owned(),
                level: 8,
                game_time_secs: 391.0,
                is_dead: false,
                respawn_timer: 0.0,
            },
            true,
        ));

        // New match reset: game time drops back to 10s -> opening window opens again.
        assert!(gate.update(
            &VisionGameContext {
                game_mode: "KIWI".to_owned(),
                level: 3,
                game_time_secs: 10.0,
                is_dead: false,
                respawn_timer: 0.0,
            },
            false,
        ));
    }

    #[test]
    fn mode_whitelist_accepts_aram_mayhem_variants_and_rejects_classic() {
        assert!(mode_supports_augments(""));
        assert!(mode_supports_augments("KIWI"));
        assert!(mode_supports_augments("ARAM"));
        assert!(mode_supports_augments("ARAM_MAYHEM"));
        assert!(mode_supports_augments("CHERRY"));
        assert!(mode_supports_augments("ULTBOOK"));
        assert!(mode_supports_augments("HEXTECH_TEST"));
        assert!(!mode_supports_augments("CLASSIC"));
        assert!(!mode_supports_augments("TFT"));
        assert!(!mode_supports_augments("PRACTICETOOL"));
        assert!(!mode_supports_augments("TUTORIAL"));
    }

    #[test]
    fn set_loading_targets_deduplicates_and_controls_capture_wakeup() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let manifest = Arc::new(RwLock::new(hex_core::RoiManifest::default()));
        let mut controller = VisionController::start(Vec::new(), Vec::new(), manifest, tx);

        assert!(!controller.shared.should_run_capture());
        assert_eq!(controller.shared.loading_targets_gen.load(Ordering::Acquire), 0);

        let targets = LoadingAlignmentTargets {
            game_id: 99,
            team_one: vec![crate::ocr::LoadingSlotTarget {
                name_zh: "卡莎".to_owned(),
                title_zh: "虚空之女".to_owned(),
                alias: "Kaisa".to_owned(),
                summoner_name: "p1".to_owned(),
            }],
            team_two: Vec::new(),
        };

        controller.set_loading_targets(Some(targets.clone()));
        assert!(controller.shared.should_run_capture());
        assert_eq!(controller.shared.loading_targets_gen.load(Ordering::Acquire), 1);

        // Identical target update must not bump generation (preserves locked/attempt state).
        controller.set_loading_targets(Some(targets));
        assert_eq!(controller.shared.loading_targets_gen.load(Ordering::Acquire), 1);

        // Clearing targets disables pending capture when augment mode is inactive.
        controller.set_loading_targets(None);
        assert!(!controller.shared.should_run_capture());
        assert_eq!(controller.shared.loading_targets_gen.load(Ordering::Acquire), 2);

        controller.shutdown();
    }

    #[test]
    fn set_tab_targets_deduplicates_and_rearms_after_clear() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let manifest = Arc::new(RwLock::new(hex_core::RoiManifest::default()));
        let mut controller = VisionController::start(Vec::new(), Vec::new(), manifest, tx);

        assert!(!controller.shared.should_run_capture());
        assert_eq!(controller.shared.tab_targets_gen.load(Ordering::Acquire), 0);

        let targets = TabAlignmentTargets {
            game_id: 505,
            left_names: vec!["烟花花火".to_owned(), "奥术先驱".to_owned()],
            right_names: vec!["输也很厉害了".to_owned(), "BattlecastXerat".to_owned()],
        };

        controller.set_tab_targets(Some(targets.clone()));
        assert!(controller.shared.should_run_capture());
        assert_eq!(controller.shared.tab_targets_gen.load(Ordering::Acquire), 1);

        // Identical active targets must not bump generation while Tab stays held.
        controller.set_tab_targets(Some(targets.clone()));
        assert_eq!(controller.shared.tab_targets_gen.load(Ordering::Acquire), 1);

        // Releasing Tab clears targets and allows WGC to hibernate.
        controller.set_tab_targets(None);
        assert!(!controller.shared.should_run_capture());
        assert_eq!(controller.shared.tab_targets_gen.load(Ordering::Acquire), 2);

        // Holding Tab again re-arms scan and bumps generation.
        controller.set_tab_targets(Some(targets));
        assert!(controller.shared.should_run_capture());
        assert_eq!(controller.shared.tab_targets_gen.load(Ordering::Acquire), 3);

        controller.shutdown();
    }
}
