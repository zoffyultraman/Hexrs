//! Silent Zstd hot-updater for `meta.db.zst` and `RoiManifest` with SHA-256 verification.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use crossbeam_channel::Sender;
use hex_core::{AspectRoiPreset, RoiManifest};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::debug;

use crate::db::{data_dir, db_path};

pub const DEFAULT_MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/zoffyultraman/Hexrs/data/manifest.json";
const ENV_MANIFEST_URL: &str = "HEXRS_MANIFEST_URL";
const SQLITE_MAGIC_HEADER: &[u8] = b"SQLite format 3\0";
const VERSION_FILE_NAME: &str = "version.txt";
const TEMP_DB_FILE_NAME: &str = "meta.db.tmp";

/// Configuration for the background `meta.db.zst` hot-updater.
#[derive(Debug, Clone)]
pub struct UpdaterConfig {
    pub manifest_url: String,
    pub request_timeout_secs: u64,
}

impl Default for UpdaterConfig {
    fn default() -> Self {
        Self {
            manifest_url: DEFAULT_MANIFEST_URL.to_owned(),
            request_timeout_secs: 10,
        }
    }
}

impl UpdaterConfig {
    /// Builds `UpdaterConfig` from `HEXRS_MANIFEST_URL` or the default GitHub `data` branch URL.
    pub fn from_env() -> Self {
        let manifest_url = std::env::var(ENV_MANIFEST_URL)
            .ok()
            .map(|raw| raw.trim().to_owned())
            .filter(|trimmed| !trimmed.is_empty())
            .unwrap_or_else(|| DEFAULT_MANIFEST_URL.to_owned());

        Self {
            manifest_url,
            request_timeout_secs: 10,
        }
    }
}

/// Result of checking and applying a remote `meta.db.zst` update.
#[derive(Debug, Clone, PartialEq)]
pub enum UpdateOutcome {
    AlreadyUpToDate {
        version: String,
    },
    Updated {
        version: String,
        db_path: PathBuf,
        roi_manifest: Option<RoiManifest>,
    },
}

/// Remote `manifest.json` payload supporting nested `roi_manifest` or flat aspect presets.
#[derive(Debug, Clone, Deserialize)]
pub struct ManifestPayload {
    pub version: String,
    pub db_url: String,
    pub sha256: String,
    #[serde(default)]
    pub roi_manifest: Option<RoiManifest>,
    #[serde(rename = "16_9", default)]
    pub preset_16_9: Option<AspectRoiPreset>,
    #[serde(rename = "16_10", default)]
    pub preset_16_10: Option<AspectRoiPreset>,
    #[serde(rename = "21_9", default)]
    pub preset_21_9: Option<AspectRoiPreset>,
}

impl ManifestPayload {
    /// Resolves either the nested `roi_manifest` or top-level aspect presets into a `RoiManifest`.
    pub fn resolve_roi_manifest(&self) -> Option<RoiManifest> {
        if let Some(manifest) = &self.roi_manifest {
            return Some(manifest.clone());
        }
        if self.preset_16_9.is_none()
            && self.preset_16_10.is_none()
            && self.preset_21_9.is_none()
        {
            return None;
        }
        let defaults = RoiManifest::default();
        Some(RoiManifest {
            version: self.version.clone(),
            sha256: self.sha256.clone(),
            preset_16_9: self.preset_16_9.clone().unwrap_or(defaults.preset_16_9),
            preset_16_10: self.preset_16_10.clone().unwrap_or(defaults.preset_16_10),
            preset_21_9: self.preset_21_9.clone().unwrap_or(defaults.preset_21_9),
        })
    }
}

/// Checks `manifest.json`, downloads `meta.db.zst` if newer, verifies SHA-256, decompresses, and atomically swaps `meta.db`.
pub fn check_and_apply_update(config: &UpdaterConfig) -> Result<UpdateOutcome> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to initialize tokio runtime for updater")?;
    rt.block_on(check_and_apply_update_async(config))
}

/// Spawns a background OS thread to execute `check_and_apply_update` without blocking the GPUI UI thread.
pub fn spawn_background_update(config: UpdaterConfig, tx: Sender<Result<UpdateOutcome>>) {
    if let Err(err) = thread::Builder::new()
        .name("hex-data-updater".to_owned())
        .spawn(move || {
            let outcome = check_and_apply_update(&config);
            let _ = tx.send(outcome);
        })
    {
        debug!(error = %err, "failed to spawn background hot-update thread");
    }
}

/// Async implementation of the Zstd `meta.db` hot-update check and atomic swap.
pub async fn check_and_apply_update_async(config: &UpdaterConfig) -> Result<UpdateOutcome> {
    let manifest_timeout = Duration::from_secs(config.request_timeout_secs.max(1));
    let download_timeout = Duration::from_secs(config.request_timeout_secs.max(30));

    let http = reqwest::Client::builder()
        .timeout(download_timeout)
        .build()
        .context("failed to build updater reqwest client")?;

    let manifest: ManifestPayload = http
        .get(&config.manifest_url)
        .timeout(manifest_timeout)
        .send()
        .await
        .with_context(|| format!("failed to fetch manifest from {}", config.manifest_url))?
        .error_for_status()
        .context("manifest endpoint returned error status")?
        .json()
        .await
        .context("failed to parse remote manifest.json")?;

    let remote_version = manifest.version.trim().to_owned();
    if remote_version.is_empty() {
        bail!("remote manifest.json contained an empty version");
    }

    let dir = data_dir();
    let target_db = db_path();
    let version_file = dir.join(VERSION_FILE_NAME);

    if let Some(local_version) = read_local_version(&version_file, &target_db) {
        if local_version == remote_version && target_db.exists() {
            return Ok(UpdateOutcome::AlreadyUpToDate {
                version: remote_version,
            });
        }
    }

    let compressed = http
        .get(&manifest.db_url)
        .send()
        .await
        .with_context(|| format!("failed to download meta.db.zst from {}", manifest.db_url))?
        .error_for_status()
        .context("meta.db.zst download returned error status")?
        .bytes()
        .await
        .context("failed to read meta.db.zst body")?;

    let actual_sha256 = sha256_hex(&compressed);
    let expected_sha256 = manifest.sha256.trim();
    if !expected_sha256.is_empty() && !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
        bail!(
            "SHA-256 mismatch for meta.db.zst: expected {expected_sha256}, got {actual_sha256}"
        );
    }

    let decompressed = zstd::stream::decode_all(compressed.as_ref())
        .context("failed to decompress Zstd meta.db.zst stream")?;

    if !decompressed.starts_with(SQLITE_MAGIC_HEADER) {
        bail!("decompressed payload is not a valid SQLite 3 database");
    }

    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create data dir {}", dir.display()))?;

    let tmp_path = dir.join(TEMP_DB_FILE_NAME);
    {
        let mut tmp_file = File::create(&tmp_path)
            .with_context(|| format!("failed to create temporary DB {}", tmp_path.display()))?;
        tmp_file
            .write_all(&decompressed)
            .context("failed to write decompressed meta.db.tmp")?;
        tmp_file
            .sync_all()
            .context("failed to sync meta.db.tmp to disk")?;
    }

    if let Err(err) = fs::rename(&tmp_path, &target_db) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err).with_context(|| {
            format!(
                "failed to atomically replace {} with {}",
                target_db.display(),
                tmp_path.display()
            )
        });
    }

    fs::write(&version_file, &remote_version)
        .with_context(|| format!("failed to write {}", version_file.display()))?;

    Ok(UpdateOutcome::Updated {
        version: remote_version,
        db_path: target_db,
        roi_manifest: manifest.resolve_roi_manifest(),
    })
}

fn read_local_version(version_file: &Path, target_db: &Path) -> Option<String> {
    if let Ok(raw) = fs::read_to_string(version_file) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }

    if !target_db.exists() {
        return None;
    }

    let conn = Connection::open_with_flags(target_db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    conn.query_row(
        "SELECT value FROM meta_info WHERE key = 'data_version'",
        [],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
    .map(|v| v.trim().to_owned())
    .filter(|v| !v.is_empty())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}
