//! On-demand Windows Graphics Capture (WGC) session controller (`windows-capture 2.0.1`).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};

use crossbeam_channel::Sender;
use hex_core::AppEvent;
use hex_data::{AugmentHashEntry, AugmentNameEntry};
use tracing::{debug, info};

use crate::ocr::AugmentTextMatcher;
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
    matches!(upper.as_str(), "CHERRY" | "KIWI" | "ARAM" | "ULTBOOK")
        || upper.contains("HEX")
        || upper.contains("AUGMENT")
}

struct VisionShared {
    matcher: RwLock<AugmentMatcher>,
    text_matcher: RwLock<AugmentTextMatcher>,
    manifest: SharedRoiManifest,
    event_tx: Sender<AppEvent>,
    active: AtomicBool,
    offer_active: AtomicBool,
    gate_open: AtomicBool,
    shutdown: AtomicBool,
    max_distance: AtomicU32,
    game_ctx: RwLock<VisionGameContext>,
    gate: Mutex<AramAugmentGate>,
    wake_lock: Mutex<()>,
    wake_cv: Condvar,
}

impl VisionShared {
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
            shutdown: AtomicBool::new(false),
            max_distance: AtomicU32::new(DEFAULT_MAX_HAMMING_DISTANCE),
            game_ctx: RwLock::new(VisionGameContext::default()),
            gate: Mutex::new(AramAugmentGate::default()),
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
                    tracing::warn!(error = %err, "failed to spawn hex-vision supervisor thread");
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
        let should_run = active && mode_supports_augments(&ctx.game_mode);
        let game_mode_log = ctx.game_mode.clone();

        if let Ok(mut guard) = self.shared.game_ctx.write() {
            *guard = ctx;
        }

        let prev = self.shared.active.swap(should_run, Ordering::AcqRel);
        if !should_run {
            self.shared.offer_active.store(false, Ordering::Release);
            if let Ok(mut gate) = self.shared.gate.lock() {
                gate.reset();
            }
        }
        self.shared.refresh_gate();

        if prev != should_run {
            info!(
                active = should_run,
                game_mode = %game_mode_log,
                gate_open = self.shared.gate_open.load(Ordering::Acquire),
                "hex-vision capture state changed"
            );
            if !should_run {
                let _ = self.shared.event_tx.send(AppEvent::AugmentOfferCleared);
            }
            self.shared.notify_supervisor();
        } else {
            debug!(
                active = should_run,
                game_mode = %game_mode_log,
                gate_open = self.shared.gate_open.load(Ordering::Acquire),
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

    /// Stops any active capture session and shuts down the supervisor thread.
    pub fn shutdown(&mut self) {
        self.shared.active.store(false, Ordering::Release);
        self.shared.offer_active.store(false, Ordering::Release);
        self.shared.gate_open.store(false, Ordering::Release);
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

    use hex_core::AppEvent;
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
    use crate::ocr::WinOcrRecognizer;
    use crate::roi::{detect_augment_screen, select_preset};

    const FRAME_PROCESS_INTERVAL: Duration = Duration::from_millis(400);
    const OFFER_DEDUP_COOLDOWN: Duration = Duration::from_secs(4);
    const CLEAR_AFTER_MISSES: u8 = 3;
    const WINDOW_RETRY_INTERVAL: Duration = Duration::from_millis(1000);
    const ACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(250);
    const FALLBACK_MAX_DISTANCE: u32 = 8;

    type BoxError = Box<dyn std::error::Error + Send + Sync>;

    struct HexCaptureHandler {
        shared: Arc<VisionShared>,
        ocr_recognizer: Option<WinOcrRecognizer>,
        scratch_buf: Vec<u8>,
        bgra_scratch: Vec<u8>,
        last_processed: Instant,
        last_emitted: Option<(Vec<i32>, Instant)>,
        active_offer: bool,
        consecutive_misses: u8,
    }

    impl GraphicsCaptureApiHandler for HexCaptureHandler {
        type Flags = Arc<VisionShared>;
        type Error = BoxError;

        fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
            let ocr_recognizer = WinOcrRecognizer::new();
            if ocr_recognizer.is_some() {
                info!("WinRT OcrEngine initialized on WGC capture thread");
            } else {
                warn!("WinRT OcrEngine unavailable; falling back to strict ROI pHash");
            }

            Ok(Self {
                shared: ctx.flags,
                ocr_recognizer,
                scratch_buf: Vec::new(),
                bgra_scratch: Vec::new(),
                last_processed: Instant::now() - FRAME_PROCESS_INTERVAL,
                last_emitted: None,
                active_offer: false,
                consecutive_misses: 0,
            })
        }

        fn on_frame_arrived(
            &mut self,
            frame: &mut Frame,
            capture_control: InternalCaptureControl,
        ) -> Result<(), Self::Error> {
            if !self.shared.active.load(Ordering::Acquire)
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

            let match_result = if let Some(ocr) = self.ocr_recognizer.as_ref() {
                let start = Instant::now();
                let hits = ocr.recognize_augment_region(
                    rgba,
                    width,
                    height,
                    &mut self.bgra_scratch,
                );
                let elapsed_ms = start.elapsed().as_millis() as u64;
                hits.and_then(|lines| {
                    let text_matcher = self.shared.text_matcher.read().ok()?;
                    text_matcher
                        .match_ocr_lines_to_offer_with_layout(&lines)
                        .map(|matched| (matched.candidates, elapsed_ms, Some(matched.layout)))
                })
            } else {
                let manifest = match self.shared.manifest.read() {
                    Ok(guard) => guard.clone(),
                    Err(_) => return Ok(()),
                };
                let preset = select_preset(&manifest, width, height);
                if detect_augment_screen(rgba, width, height, preset) {
                    let Ok(mut matcher) = self.shared.matcher.read().map(|g| g.clone()) else {
                        return Ok(());
                    };
                    let strict_dist = self
                        .shared
                        .max_distance
                        .load(Ordering::Acquire)
                        .min(FALLBACK_MAX_DISTANCE);
                    matcher.set_max_distance(strict_dist);
                    matcher
                        .match_offer(rgba, width, height, &preset.card_icons)
                        .map(|(candidates, ms)| (candidates, ms, None))
                } else {
                    None
                }
            };

            if let Some((candidates, capture_ms, layout)) = match_result {
                self.consecutive_misses = 0;
                self.active_offer = true;
                self.shared.set_offer_active(true);

                let is_duplicate = self
                    .last_emitted
                    .as_ref()
                    .map(|(prev, at)| *prev == candidates && at.elapsed() < OFFER_DEDUP_COOLDOWN)
                    .unwrap_or(false);

                if !is_duplicate {
                    debug!(?candidates, capture_ms, ?layout, "detected hextech augment offer");
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
                    debug!("augment offer modal dismissed; clearing strip");
                    let _ = self.shared.event_tx.send(AppEvent::AugmentOfferCleared);
                }
            }

            Ok(())
        }

        fn on_closed(&mut self) -> Result<(), Self::Error> {
            debug!("WGC capture session closed by game window");
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

        while !shared.shutdown.load(Ordering::Acquire) {
            let should_run = shared.active.load(Ordering::Acquire);

            if !should_run {
                if active_control.is_some() {
                    info!("hibernating WGC capture session");
                    stop_control(active_control.take());
                }
                let guard = match shared.wake_lock.lock() {
                    Ok(g) => g,
                    Err(_) => break,
                };
                if !shared.shutdown.load(Ordering::Acquire)
                    && !shared.active.load(Ordering::Acquire)
                {
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
}
