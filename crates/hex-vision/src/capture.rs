//! On-demand Windows Graphics Capture (WGC) session controller (`windows-capture 2.0.1`).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};

use crossbeam_channel::Sender;
use hex_core::AppEvent;
use hex_data::AugmentHashEntry;
use tracing::{debug, info};

use crate::phash::AugmentMatcher;
use crate::roi::SharedRoiManifest;

const DEFAULT_MAX_HAMMING_DISTANCE: u32 = 12;

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

/// Returns `true` when the active game mode supports hextech augment selection modals.
pub fn mode_supports_augments(game_mode: &str) -> bool {
    let upper = game_mode.trim().to_ascii_uppercase();
    if upper.is_empty() {
        return false;
    }
    matches!(upper.as_str(), "CHERRY" | "KIWI" | "ARAM" | "ULTBOOK")
        || upper.contains("HEX")
        || upper.contains("AUGMENT")
}

struct VisionShared {
    matcher: RwLock<AugmentMatcher>,
    manifest: SharedRoiManifest,
    event_tx: Sender<AppEvent>,
    active: AtomicBool,
    shutdown: AtomicBool,
    max_distance: AtomicU32,
    game_mode: RwLock<String>,
    wake_lock: Mutex<()>,
    wake_cv: Condvar,
}

impl VisionShared {
    fn notify_supervisor(&self) {
        let _guard = self.wake_lock.lock();
        self.wake_cv.notify_all();
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
    /// Starts the vision controller in dormant state; call `set_active(true, mode)` in-game.
    pub fn start(
        matcher_hashes: Vec<AugmentHashEntry>,
        manifest: SharedRoiManifest,
        event_tx: Sender<AppEvent>,
    ) -> Self {
        let matcher = AugmentMatcher::new(matcher_hashes, DEFAULT_MAX_HAMMING_DISTANCE);
        let shared = Arc::new(VisionShared {
            matcher: RwLock::new(matcher),
            manifest,
            event_tx,
            active: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            max_distance: AtomicU32::new(DEFAULT_MAX_HAMMING_DISTANCE),
            game_mode: RwLock::new(String::new()),
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

    /// Wakes or hibernates WGC capture based on match state and whether `game_mode` has augments.
    pub fn set_active(&self, active: bool, game_mode: &str) {
        let should_run = active && mode_supports_augments(game_mode);

        if let Ok(mut guard) = self.shared.game_mode.write() {
            *guard = game_mode.to_owned();
        }

        let prev = self.shared.active.swap(should_run, Ordering::AcqRel);
        if prev != should_run {
            info!(
                active = should_run,
                game_mode,
                "hex-vision capture state changed"
            );
            self.shared.notify_supervisor();
        } else {
            debug!(active = should_run, game_mode, "hex-vision state unchanged");
        }
    }

    /// Updates the in-memory augment pHash dictionary (e.g., after a `meta.db` reload).
    pub fn update_hashes(&self, hashes: Vec<AugmentHashEntry>) {
        let max_dist = self.shared.max_distance.load(Ordering::Acquire);
        if let Ok(mut matcher) = self.shared.matcher.write() {
            matcher.update_entries(hashes);
            matcher.set_max_distance(max_dist);
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
    use crate::roi::{detect_augment_screen, select_preset};

    const FRAME_PROCESS_INTERVAL: Duration = Duration::from_millis(300);
    const OFFER_DEDUP_COOLDOWN: Duration = Duration::from_secs(4);
    const WINDOW_RETRY_INTERVAL: Duration = Duration::from_millis(1000);
    const ACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(250);

    type BoxError = Box<dyn std::error::Error + Send + Sync>;

    struct HexCaptureHandler {
        shared: Arc<VisionShared>,
        scratch_buf: Vec<u8>,
        last_processed: Instant,
        last_emitted: Option<(Vec<i32>, Instant)>,
    }

    impl GraphicsCaptureApiHandler for HexCaptureHandler {
        type Flags = Arc<VisionShared>;
        type Error = BoxError;

        fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
            Ok(Self {
                shared: ctx.flags,
                scratch_buf: Vec::new(),
                last_processed: Instant::now() - FRAME_PROCESS_INTERVAL,
                last_emitted: None,
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
                capture_control.stop();
                return Ok(());
            }

            if self.last_processed.elapsed() < FRAME_PROCESS_INTERVAL {
                return Ok(());
            }
            self.last_processed = Instant::now();

            let buffer = frame.buffer()?;
            let width = buffer.width();
            let height = buffer.height();
            let rgba = buffer.as_nopadding_buffer(&mut self.scratch_buf);

            let manifest = match self.shared.manifest.read() {
                Ok(guard) => guard.clone(),
                Err(_) => return Ok(()),
            };
            let preset = select_preset(&manifest, width, height);

            if !detect_augment_screen(rgba, width, height, preset) {
                return Ok(());
            }

            let match_result = {
                let Ok(matcher) = self.shared.matcher.read() else {
                    return Ok(());
                };
                matcher.match_offer(rgba, width, height, &preset.card_icons)
            };

            if let Some((candidates, capture_ms)) = match_result {
                let is_duplicate = self
                    .last_emitted
                    .as_ref()
                    .map(|(prev, at)| *prev == candidates && at.elapsed() < OFFER_DEDUP_COOLDOWN)
                    .unwrap_or(false);

                if !is_duplicate {
                    debug!(?candidates, capture_ms, "detected hextech augment offer");
                    let _ = self.shared.event_tx.send(AppEvent::AugmentOfferDetected {
                        candidates: candidates.clone(),
                        capture_ms,
                    });
                    self.last_emitted = Some((candidates, Instant::now()));
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
