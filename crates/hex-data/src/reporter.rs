//! Strictly anonymized end-of-game crowdsourced telemetry reporter (zero PII).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use hex_core::{EndOfGameParticipant, EndOfGameStats};
use serde::Serialize;
use tracing::debug;

const ENV_TELEMETRY_URL: &str = "HEXRS_TELEMETRY_URL";
const REQUEST_TIMEOUT_SECS: u64 = 5;
const PAYLOAD_SCHEMA_VERSION: u32 = 1;

/// Configuration for anonymized end-of-game match telemetry submission.
#[derive(Debug, Clone)]
pub struct ReporterConfig {
    pub endpoint_url: Option<String>,
    pub enabled: bool,
}

impl Default for ReporterConfig {
    fn default() -> Self {
        Self {
            endpoint_url: None,
            enabled: true,
        }
    }
}

impl ReporterConfig {
    /// Reads telemetry endpoint configuration from `HEXRS_TELEMETRY_URL`.
    pub fn from_env() -> Self {
        let endpoint_url = std::env::var(ENV_TELEMETRY_URL)
            .ok()
            .map(|raw| raw.trim().to_owned())
            .filter(|trimmed| !trimmed.is_empty());

        Self {
            endpoint_url,
            enabled: true,
        }
    }
}

/// Strictly anonymized payload containing zero summoner names, Riot IDs, PUUIDs, or match IDs.
#[derive(Debug, Serialize)]
struct AnonymizedEogPayload {
    schema_version: u32,
    client_version: &'static str,
    game_mode: String,
    queue_id: Option<i32>,
    participants: Vec<EndOfGameParticipant>,
}

/// Non-blocking HTTPS reporter for anonymized post-match telemetry.
#[derive(Clone)]
pub struct EogReporter {
    endpoint_url: Option<String>,
    enabled: Arc<AtomicBool>,
    http: reqwest::Client,
}

impl EogReporter {
    /// Creates an `EogReporter` from `ReporterConfig` with a 5-second request timeout.
    pub fn new(config: ReporterConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            endpoint_url: config.endpoint_url,
            enabled: Arc::new(AtomicBool::new(config.enabled)),
            http,
        }
    }

    /// Creates an `EogReporter` using `ReporterConfig::from_env()`.
    pub fn from_env() -> Self {
        Self::new(ReporterConfig::from_env())
    }

    /// Enables or disables telemetry submission dynamically (e.g., from the system tray toggle).
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Returns whether telemetry submission is currently enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Validates and submits anonymized EOG stats asynchronously without blocking the UI thread.
    pub fn submit_async(&self, stats: EndOfGameStats) {
        if !self.is_enabled() {
            return;
        }
        let Some(endpoint_url) = self.endpoint_url.clone() else {
            debug!("skipping EOG telemetry submit: HEXRS_TELEMETRY_URL not configured");
            return;
        };

        let Some(payload) = sanitize_payload(stats) else {
            debug!("skipping EOG telemetry submit: no valid participants");
            return;
        };

        let http = self.http.clone();
        if let Err(err) = thread::Builder::new()
            .name("hex-eog-reporter".to_owned())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };

                if let Err(err) = rt.block_on(post_payload(&http, &endpoint_url, &payload)) {
                    debug!(error = %err, "anonymized EOG telemetry upload failed");
                }
            })
        {
            debug!(error = %err, "failed to spawn EOG telemetry thread");
        }
    }

    /// Submits anonymized EOG stats inside an existing async context.
    pub async fn submit(&self, stats: EndOfGameStats) -> Result<()> {
        if !self.is_enabled() {
            return Ok(());
        }
        let Some(endpoint_url) = self.endpoint_url.as_deref() else {
            return Ok(());
        };
        let Some(payload) = sanitize_payload(stats) else {
            return Ok(());
        };
        post_payload(&self.http, endpoint_url, &payload).await
    }
}

fn sanitize_payload(stats: EndOfGameStats) -> Option<AnonymizedEogPayload> {
    let participants: Vec<EndOfGameParticipant> = stats
        .participants
        .into_iter()
        .filter(|p| p.champion_id > 0)
        .map(|p| EndOfGameParticipant {
            champion_id: p.champion_id,
            placement: p.placement,
            win: p.win,
            items: p.items.into_iter().filter(|&id| id > 0).collect(),
            augments: p.augments.into_iter().filter(|&id| id > 0).collect(),
        })
        .collect();

    if participants.is_empty() {
        return None;
    }

    Some(AnonymizedEogPayload {
        schema_version: PAYLOAD_SCHEMA_VERSION,
        client_version: env!("CARGO_PKG_VERSION"),
        game_mode: stats.game_mode,
        queue_id: stats.queue_id,
        participants,
    })
}

async fn post_payload(
    http: &reqwest::Client,
    endpoint_url: &str,
    payload: &AnonymizedEogPayload,
) -> Result<()> {
    let resp = http
        .post(endpoint_url)
        .json(payload)
        .send()
        .await
        .context("failed to POST anonymized EOG telemetry")?;

    resp.error_for_status()
        .context("EOG telemetry endpoint returned non-success status")?;

    debug!(
        game_mode = %payload.game_mode,
        participants = payload.participants.len(),
        "anonymized EOG telemetry uploaded"
    );
    Ok(())
}
