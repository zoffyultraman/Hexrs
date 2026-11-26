//! Asynchronous local PNG icon cache for champion portraits and recommended items.

use std::collections::HashSet;
use std::fs::{self, File};
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossbeam_channel::Sender;
use tracing::debug;

use crate::db::data_dir;

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
const DEFAULT_DD_VERSION: &str = "15.22.1";
const DD_VERSIONS_URL: &str = "https://ddragon.leagueoflegends.com/api/versions.json";

#[derive(Debug, Default)]
struct IconCacheState {
    in_flight: HashSet<String>,
    failed: HashSet<String>,
    dd_version: Option<String>,
}

/// Thread-safe, non-blocking disk cache fetching champion and item PNG icons in the background.
#[derive(Clone)]
pub struct IconCache {
    cache_dir: PathBuf,
    state: Arc<Mutex<IconCacheState>>,
    http: reqwest::Client,
    on_ready_tx: Sender<()>,
}

impl IconCache {
    /// Creates an `IconCache` backed by `%LOCALAPPDATA%/Hexrs/cache/icons`.
    pub fn new(on_ready_tx: Sender<()>) -> Self {
        let cache_dir = data_dir().join("cache").join("icons");
        let _ = fs::create_dir_all(&cache_dir);
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            cache_dir,
            state: Arc::new(Mutex::new(IconCacheState::default())),
            http,
            on_ready_tx,
        }
    }

    /// Returns the cached PNG path for `hero_id` if ready, or triggers a background fetch.
    pub fn champion_icon(&self, hero_id: i32, alias: Option<&str>) -> Option<PathBuf> {
        if hero_id <= 0 {
            return None;
        }

        let target = self.cache_dir.join(format!("champ_{hero_id}.png"));
        if is_valid_cached_png(&target) {
            return Some(target);
        }

        let key = format!("champ:{hero_id}");
        if !self.try_mark_in_flight(&key) {
            return None;
        }

        let alias_owned = alias
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

        self.spawn_icon_fetch(key, target, move |http, state| async move {
            let cdragon_url = format!(
                "https://raw.communitydragon.org/latest/plugins/rcp-be-lol-game-data/global/default/v1/champion-icons/{hero_id}.png"
            );
            let mut png_bytes = fetch_png_bytes(&http, &cdragon_url).await;

            if png_bytes.is_none() {
                if let Some(alias_name) = alias_owned.as_deref() {
                    let ver = resolve_dd_version(&http, &state).await;
                    let dd_url = format!(
                        "https://ddragon.leagueoflegends.com/cdn/{ver}/img/champion/{alias_name}.png"
                    );
                    png_bytes = fetch_png_bytes(&http, &dd_url).await;
                }
            }

            png_bytes
        });

        None
    }

    /// Returns the cached PNG path for `item_id` if ready, or triggers a background fetch.
    pub fn item_icon(&self, item_id: i32) -> Option<PathBuf> {
        if item_id <= 0 {
            return None;
        }

        let target = self.cache_dir.join(format!("item_{item_id}.png"));
        if is_valid_cached_png(&target) {
            return Some(target);
        }

        let key = format!("item:{item_id}");
        if !self.try_mark_in_flight(&key) {
            return None;
        }

        self.spawn_icon_fetch(key, target, move |http, state| async move {
            let ver = resolve_dd_version(&http, &state).await;
            let dd_url =
                format!("https://ddragon.leagueoflegends.com/cdn/{ver}/img/item/{item_id}.png");
            fetch_png_bytes(&http, &dd_url).await
        });

        None
    }

    fn spawn_icon_fetch<F, Fut>(&self, key: String, target: PathBuf, fetcher: F)
    where
        F: FnOnce(reqwest::Client, Arc<Mutex<IconCacheState>>) -> Fut + Send + 'static,
        Fut: Future<Output = Option<Vec<u8>>>,
    {
        let key_for_thread = key.clone();
        let state = Arc::clone(&self.state);
        let http = self.http.clone();
        let on_ready_tx = self.on_ready_tx.clone();

        if thread::Builder::new()
            .name("hex-icon-fetch".to_owned())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    finish_fetch(&state, &key_for_thread, false);
                    return;
                };

                let ok = rt.block_on(async {
                    if let Some(bytes) = fetcher(http, Arc::clone(&state)).await {
                        match write_atomic_png(&target, &bytes) {
                            Ok(()) => return true,
                            Err(err) => {
                                debug!(error = %err, path = %target.display(), "failed to write cached icon");
                            }
                        }
                    }
                    false
                });

                finish_fetch(&state, &key_for_thread, ok);
                if ok {
                    let _ = on_ready_tx.send(());
                }
            })
            .is_err()
        {
            finish_fetch(&self.state, &key, false);
        }
    }

    fn try_mark_in_flight(&self, key: &str) -> bool {
        let Ok(mut guard) = self.state.lock() else {
            return false;
        };
        if guard.in_flight.contains(key) || guard.failed.contains(key) {
            return false;
        }
        guard.in_flight.insert(key.to_owned());
        true
    }
}

fn finish_fetch(state: &Mutex<IconCacheState>, key: &str, succeeded: bool) {
    if let Ok(mut guard) = state.lock() {
        guard.in_flight.remove(key);
        if !succeeded {
            guard.failed.insert(key.to_owned());
        }
    }
}

fn is_valid_cached_png(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_file() && meta.len() > PNG_MAGIC.len() as u64)
        .unwrap_or(false)
}

async fn resolve_dd_version(http: &reqwest::Client, state: &Mutex<IconCacheState>) -> String {
    if let Ok(guard) = state.lock() {
        if let Some(ver) = guard.dd_version.as_ref() {
            return ver.clone();
        }
    }

    let fetched = match http.get(DD_VERSIONS_URL).send().await {
        Ok(resp) => match resp.error_for_status() {
            Ok(ok_resp) => ok_resp
                .json::<Vec<String>>()
                .await
                .ok()
                .and_then(|list| list.into_iter().next())
                .filter(|v| !v.trim().is_empty()),
            Err(_) => None,
        },
        Err(_) => None,
    };

    if let Some(ver) = fetched {
        if let Ok(mut guard) = state.lock() {
            guard.dd_version = Some(ver.clone());
        }
        ver
    } else {
        DEFAULT_DD_VERSION.to_owned()
    }
}

async fn fetch_png_bytes(http: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let resp = http.get(url).send().await.ok()?.error_for_status().ok()?;
    let bytes = resp.bytes().await.ok()?;
    if bytes.starts_with(PNG_MAGIC) {
        Some(bytes.to_vec())
    } else {
        debug!(%url, "downloaded icon did not match PNG magic header");
        None
    }
}

fn write_atomic_png(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp_name = format!(
        "{}.tmp",
        target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("icon.png")
    );
    let tmp_path = target.with_file_name(tmp_name);

    {
        let mut file = File::create(&tmp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }

    if let Err(err) = fs::rename(&tmp_path, target) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    Ok(())
}
