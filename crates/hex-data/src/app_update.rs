//! Client application version parsing, ordering, and GitHub Releases update checker.

use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use reqwest::header::{ACCEPT, USER_AGENT};
use serde::Deserialize;
use tracing::debug;

pub const DEFAULT_RELEASES_API_URL: &str =
    "https://api.github.com/repos/zoffyultraman/Hexrs/releases?per_page=10";
pub const DEFAULT_RELEASES_PAGE_URL: &str = "https://github.com/zoffyultraman/Hexrs/releases";
const ENV_RELEASES_API_URL: &str = "HEXRS_RELEASES_API_URL";

/// Configuration for checking remote GitHub Releases for newer Hexrs client builds.
#[derive(Debug, Clone)]
pub struct AppUpdateConfig {
    pub releases_api_url: String,
    pub request_timeout_secs: u64,
}

impl Default for AppUpdateConfig {
    fn default() -> Self {
        Self {
            releases_api_url: DEFAULT_RELEASES_API_URL.to_owned(),
            request_timeout_secs: 10,
        }
    }
}

impl AppUpdateConfig {
    /// Builds `AppUpdateConfig` from `HEXRS_RELEASES_API_URL` or the default GitHub Releases API URL.
    pub fn from_env() -> Self {
        let releases_api_url = std::env::var(ENV_RELEASES_API_URL)
            .ok()
            .map(|raw| raw.trim().to_owned())
            .filter(|trimmed| !trimmed.is_empty())
            .unwrap_or_else(|| DEFAULT_RELEASES_API_URL.to_owned());
        Self {
            releases_api_url,
            request_timeout_secs: 10,
        }
    }
}

/// Outcome of comparing the running client version against published GitHub Releases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppUpdateOutcome {
    UpToDate {
        current_version: String,
        latest_version: String,
        release_url: String,
    },
    UpdateAvailable {
        current_version: String,
        latest_version: String,
        release_url: String,
        download_url: Option<String>,
        is_prerelease: bool,
        published_at: String,
    },
}

/// Release maturity rank ordered from earliest pre-release to stable release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VersionChannel {
    Alpha = 1,
    Beta = 2,
    Rc = 3,
    Stable = 4,
}

/// Parsed semantic/channel application version with deterministic ordering.
///
/// Field declaration order defines `Ord`:
/// `(major, minor, patch, channel, prerelease_num, dev_commits_ahead)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ParsedAppVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub channel: VersionChannel,
    pub prerelease_num: u64,
    pub dev_commits_ahead: u64,
}

impl ParsedAppVersion {
    /// Returns the numeric `(major, minor, patch)` tuple without channel or dev-commit metadata.
    pub fn core_tuple(self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }

    /// Parses tags such as `alpha_0.0.4`, `alpha_0.0.4-12-g9fcc012`, `beta_0.1.0`, `rc_0.2.0`,
    /// `v0.1.0`, `0.1.0`, `v0.1.0-12-g9fcc012`, `v1.0.0-alpha.2`, `v1.0.0-beta.1`, `v1.0.0-rc.1`.
    pub fn parse(raw: &str) -> Option<Self> {
        let mut s = raw.trim();
        if s.is_empty() {
            return None;
        }

        // Strip optional trailing "-dirty" marker if present (UTF-8 boundary-safe).
        if s.len() > 6
            && s.get(s.len() - 6..)
                .map(|tail| tail.eq_ignore_ascii_case("-dirty"))
                .unwrap_or(false)
        {
            s = &s[..s.len() - 6];
        }

        // Strip optional `git describe` suffix: `-<commits>-g<hex>`.
        let mut dev_commits_ahead = 0u64;
        if let Some((before_g, g_part)) = s.rsplit_once('-') {
            if g_part.len() >= 2
                && (g_part.starts_with('g') || g_part.starts_with('G'))
                && g_part[1..].chars().all(|c| c.is_ascii_hexdigit())
            {
                if let Some((base, commits_str)) = before_g.rsplit_once('-') {
                    if let Ok(commits) = commits_str.parse::<u64>() {
                        dev_commits_ahead = commits;
                        s = base;
                    }
                }
            }
        }

        // Strip optional build metadata (`+...`).
        if let Some((before_plus, _)) = s.split_once('+') {
            s = before_plus;
        }

        let mut channel = VersionChannel::Stable;
        let lower_prefix = s.to_ascii_lowercase();
        if lower_prefix.starts_with("alpha_") || lower_prefix.starts_with("alpha-") {
            channel = VersionChannel::Alpha;
            s = &s[6..];
        } else if lower_prefix.starts_with("beta_") || lower_prefix.starts_with("beta-") {
            channel = VersionChannel::Beta;
            s = &s[5..];
        } else if lower_prefix.starts_with("rc_") || lower_prefix.starts_with("rc-") {
            channel = VersionChannel::Rc;
            s = &s[3..];
        }

        if s.starts_with('v') || s.starts_with('V') {
            s = &s[1..];
        }

        let (core_str, prerelease_str) = match s.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (s, None),
        };

        let parts: Vec<&str> = core_str.split('.').collect();
        if !(2..=3).contains(&parts.len()) {
            return None;
        }
        let major = parts[0].parse::<u64>().ok()?;
        let minor = parts[1].parse::<u64>().ok()?;
        let patch = if parts.len() == 3 {
            parts[2].parse::<u64>().ok()?
        } else {
            0
        };

        let mut prerelease_num = 0u64;
        if let Some(pre) = prerelease_str {
            let pre_lower = pre.trim().to_ascii_lowercase();
            let rest = if let Some(r) = pre_lower.strip_prefix("alpha") {
                channel = VersionChannel::Alpha;
                r
            } else if let Some(r) = pre_lower.strip_prefix("beta") {
                channel = VersionChannel::Beta;
                r
            } else if let Some(r) = pre_lower.strip_prefix("rc") {
                channel = VersionChannel::Rc;
                r
            } else {
                return None;
            };
            let num_str = rest.trim_start_matches(['.', '_', '-']);
            if !num_str.is_empty() {
                prerelease_num = num_str.parse::<u64>().ok()?;
            }
        }

        Some(Self {
            major,
            minor,
            patch,
            channel,
            prerelease_num,
            dev_commits_ahead,
        })
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    draft: bool,
    prerelease: bool,
    published_at: String,
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct GithubReleaseAsset {
    name: String,
    browser_download_url: String,
}

fn is_stable_release(release: &GithubRelease, parsed: ParsedAppVersion) -> bool {
    !release.prerelease && parsed.channel == VersionChannel::Stable
}

fn evaluate_releases(releases: &[GithubRelease], current_version: &str) -> AppUpdateOutcome {
    let current_trimmed = current_version.trim();

    let mut best_stable: Option<(ParsedAppVersion, &GithubRelease)> = None;
    let mut best_prerelease: Option<(ParsedAppVersion, &GithubRelease)> = None;
    for release in releases {
        if release.draft {
            continue;
        }
        let Some(parsed) = ParsedAppVersion::parse(&release.tag_name) else {
            continue;
        };
        if is_stable_release(release, parsed) {
            if best_stable
                .as_ref()
                .map(|(best_ver, _)| parsed > *best_ver)
                .unwrap_or(true)
            {
                best_stable = Some((parsed, release));
            }
        } else if best_prerelease
            .as_ref()
            .map(|(best_ver, _)| parsed > *best_ver)
            .unwrap_or(true)
        {
            best_prerelease = Some((parsed, release));
        }
    }

    let cur_parsed = ParsedAppVersion::parse(current_trimmed);
    let current_is_stable = cur_parsed
        .map(|v| v.channel == VersionChannel::Stable)
        .unwrap_or(false);

    let selected = if current_is_stable {
        match best_stable {
            Some((sv, sr)) => Some((sv, sr, true)),
            None => {
                return AppUpdateOutcome::UpToDate {
                    current_version: current_trimmed.to_owned(),
                    latest_version: current_trimmed.to_owned(),
                    release_url: DEFAULT_RELEASES_PAGE_URL.to_owned(),
                };
            }
        }
    } else {
        match (best_stable, best_prerelease) {
            (Some((sv, sr)), Some((pv, pr))) => {
                let p_pub = pr.published_at.trim();
                let s_pub = sr.published_at.trim();
                if pv.core_tuple() > sv.core_tuple()
                    && !p_pub.is_empty()
                    && !s_pub.is_empty()
                    && p_pub > s_pub
                {
                    Some((pv, pr, false))
                } else {
                    Some((sv, sr, true))
                }
            }
            (Some((sv, sr)), None) => Some((sv, sr, true)),
            (None, Some((pv, pr))) => Some((pv, pr, false)),
            (None, None) => None,
        }
    };

    let Some((best_ver, best_release, target_is_stable)) = selected else {
        return AppUpdateOutcome::UpToDate {
            current_version: current_trimmed.to_owned(),
            latest_version: current_trimmed.to_owned(),
            release_url: DEFAULT_RELEASES_PAGE_URL.to_owned(),
        };
    };

    let latest_version = best_release.tag_name.trim().to_owned();
    let release_url = {
        let html = best_release.html_url.trim();
        if html.is_empty() {
            DEFAULT_RELEASES_PAGE_URL.to_owned()
        } else {
            html.to_owned()
        }
    };

    let is_newer = match cur_parsed {
        Some(cur_ver) => {
            if target_is_stable && cur_ver.channel != VersionChannel::Stable {
                best_ver >= cur_ver
                    || (cur_ver.channel == VersionChannel::Alpha
                        && cur_ver.major == 0
                        && cur_ver.minor == 0)
            } else {
                best_ver > cur_ver
            }
        }
        None => false,
    };

    if is_newer {
        let download_url = best_release
            .assets
            .iter()
            .find(|asset| {
                let url = asset.browser_download_url.trim();
                let name = asset.name.trim();
                !url.is_empty()
                    && (name.to_ascii_lowercase().ends_with(".zip")
                        || url.to_ascii_lowercase().ends_with(".zip"))
            })
            .map(|asset| asset.browser_download_url.trim().to_owned());

        AppUpdateOutcome::UpdateAvailable {
            current_version: current_trimmed.to_owned(),
            latest_version,
            release_url,
            download_url,
            is_prerelease: !target_is_stable,
            published_at: best_release.published_at.trim().to_owned(),
        }
    } else {
        AppUpdateOutcome::UpToDate {
            current_version: current_trimmed.to_owned(),
            latest_version,
            release_url,
        }
    }
}

/// Async check against the configured GitHub Releases API endpoint.
pub async fn check_app_update_async(
    config: &AppUpdateConfig,
    current_version: &str,
) -> Result<AppUpdateOutcome> {
    let timeout = Duration::from_secs(config.request_timeout_secs.max(1));
    let http = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .context("failed to build app update reqwest client")?;

    let ua_version = {
        let trimmed = current_version.trim();
        if trimmed.is_empty() {
            "unknown"
        } else {
            trimmed
        }
    };
    let user_agent = format!("Hexrs/{ua_version}");

    let releases: Vec<GithubRelease> = http
        .get(&config.releases_api_url)
        .header(USER_AGENT, user_agent)
        .header(ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .with_context(|| {
            format!(
                "failed to fetch GitHub releases from {}",
                config.releases_api_url
            )
        })?
        .error_for_status()
        .context("GitHub releases API returned error status")?
        .json()
        .await
        .context("failed to parse GitHub releases JSON")?;

    Ok(evaluate_releases(&releases, current_version))
}

/// Synchronous wrapper running `check_app_update_async` on a single-threaded Tokio runtime.
pub fn check_app_update(
    config: &AppUpdateConfig,
    current_version: &str,
) -> Result<AppUpdateOutcome> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to initialize tokio runtime for app update check")?;
    rt.block_on(check_app_update_async(config, current_version))
}

/// Spawns a background OS thread to check for newer client releases without blocking the UI thread.
pub fn spawn_app_update_check(
    config: AppUpdateConfig,
    current_version: String,
    tx: Sender<Result<AppUpdateOutcome>>,
) {
    if let Err(err) = thread::Builder::new()
        .name("hex-data-app-update".to_owned())
        .spawn(move || {
            let outcome = check_app_update(&config, &current_version);
            let _ = tx.send(outcome);
        })
    {
        debug!(error = %err, "failed to spawn background app update check thread");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_orders_channel_semver_and_git_describe_versions() {
        let chain = [
            "alpha_0.0.4",
            "alpha_0.0.4-12-g9fcc012",
            "alpha_0.0.5",
            "beta_0.1.0",
            "rc_0.1.0",
            "v0.1.0",
            "v0.1.0-12-g9fcc012",
            "rc_0.2.0",
            "v1.0.0-alpha.2",
            "v1.0.0-beta.1",
            "v1.0.0-rc.1",
            "v1.0.0",
        ];
        let parsed: Vec<ParsedAppVersion> = chain
            .iter()
            .map(|raw| {
                ParsedAppVersion::parse(raw)
                    .unwrap_or_else(|| panic!("expected version `{raw}` to parse"))
            })
            .collect();

        for window in parsed.windows(2) {
            assert!(
                window[0] < window[1],
                "expected {:?} < {:?}",
                window[0],
                window[1]
            );
        }

        // `0.1.0` equals `v0.1.0`.
        assert_eq!(
            ParsedAppVersion::parse("0.1.0"),
            ParsedAppVersion::parse("v0.1.0")
        );

        // Invalid / bare-SHA / non-ASCII strings return None without panicking.
        assert_eq!(ParsedAppVersion::parse(""), None);
        assert_eq!(ParsedAppVersion::parse("9fcc012"), None);
        assert_eq!(ParsedAppVersion::parse("manual"), None);
        assert_eq!(ParsedAppVersion::parse("版本v1.0"), None);
    }

    #[test]
    fn evaluate_releases_skips_drafts_picks_highest_version_and_extracts_zip_asset() {
        let json = r#"[
            {
                "tag_name": "v9.9.9",
                "html_url": "https://github.com/zoffyultraman/Hexrs/releases/tag/v9.9.9",
                "draft": true,
                "prerelease": false,
                "published_at": "2026-11-30T00:00:00Z",
                "assets": [
                    {"name": "Hexrs-v9.9.9.zip", "browser_download_url": "https://example.com/draft.zip"}
                ]
            },
            {
                "tag_name": "alpha_0.0.4",
                "html_url": "https://github.com/zoffyultraman/Hexrs/releases/tag/alpha_0.0.4",
                "draft": false,
                "prerelease": true,
                "published_at": "2026-11-28T00:00:00Z",
                "assets": []
            },
            {
                "tag_name": "alpha_0.0.5",
                "html_url": "https://github.com/zoffyultraman/Hexrs/releases/tag/alpha_0.0.5",
                "draft": false,
                "prerelease": true,
                "published_at": "2026-11-29T01:00:00Z",
                "assets": [
                    {"name": "SHA256SUMS.txt", "browser_download_url": "https://example.com/sums.txt"},
                    {"name": "Hexrs-alpha_0.0.5-windows-x86_64.zip", "browser_download_url": "https://example.com/Hexrs-alpha_0.0.5.zip"}
                ]
            },
            {
                "tag_name": "beta_0.0.5",
                "html_url": "https://github.com/zoffyultraman/Hexrs/releases/tag/beta_0.0.5",
                "draft": false,
                "prerelease": true,
                "published_at": "2026-11-29T02:00:00Z",
                "assets": [
                    {"name": "Hexrs-beta_0.0.5-windows-x86_64.zip", "browser_download_url": "https://example.com/Hexrs-beta_0.0.5.zip"}
                ]
            }
        ]"#;
        let releases: Vec<GithubRelease> = serde_json::from_str(json).expect("parse releases");

        // Older dev build ahead of alpha_0.0.4 sees beta_0.0.5 as highest non-draft update.
        let outcome = evaluate_releases(&releases, "alpha_0.0.4-12-g9fcc012");
        assert_eq!(
            outcome,
            AppUpdateOutcome::UpdateAvailable {
                current_version: "alpha_0.0.4-12-g9fcc012".to_owned(),
                latest_version: "beta_0.0.5".to_owned(),
                release_url: "https://github.com/zoffyultraman/Hexrs/releases/tag/beta_0.0.5"
                    .to_owned(),
                download_url: Some("https://example.com/Hexrs-beta_0.0.5.zip".to_owned()),
                is_prerelease: true,
                published_at: "2026-11-29T02:00:00Z".to_owned(),
            }
        );

        // Already on beta_0.0.5 (or newer v0.1.0) -> UpToDate.
        assert_eq!(
            evaluate_releases(&releases, "beta_0.0.5"),
            AppUpdateOutcome::UpToDate {
                current_version: "beta_0.0.5".to_owned(),
                latest_version: "beta_0.0.5".to_owned(),
                release_url: "https://github.com/zoffyultraman/Hexrs/releases/tag/beta_0.0.5"
                    .to_owned(),
            }
        );

        // Empty release list safely falls back to UpToDate with default releases page URL.
        assert_eq!(
            evaluate_releases(&[], "alpha_0.0.4"),
            AppUpdateOutcome::UpToDate {
                current_version: "alpha_0.0.4".to_owned(),
                latest_version: "alpha_0.0.4".to_owned(),
                release_url: DEFAULT_RELEASES_PAGE_URL.to_owned(),
            }
        );
    }

    #[test]
    fn stable_v0_0_1_supersedes_legacy_alpha_0_0_4_and_never_downgrades_stable() {
        let make_rel = |tag: &str, prerelease: bool, published_at: &str| GithubRelease {
            tag_name: tag.to_owned(),
            html_url: format!("https://github.com/zoffyultraman/Hexrs/releases/tag/{tag}"),
            draft: false,
            prerelease,
            published_at: published_at.to_owned(),
            assets: vec![GithubReleaseAsset {
                name: format!("Hexrs-{tag}-windows-x86_64.zip"),
                browser_download_url: format!("https://example.com/Hexrs-{tag}.zip"),
            }],
        };

        // 1. [v0.0.1 (stable, 2026-11-29), alpha_0.0.4 (prerelease, 2026-11-26)]
        let stable_and_legacy_alpha = vec![
            make_rel("v0.0.1", false, "2026-11-29T10:00:00Z"),
            make_rel("alpha_0.0.4", true, "2026-11-26T10:00:00Z"),
        ];
        assert_eq!(
            evaluate_releases(&stable_and_legacy_alpha, "v0.0.1"),
            AppUpdateOutcome::UpToDate {
                current_version: "v0.0.1".to_owned(),
                latest_version: "v0.0.1".to_owned(),
                release_url: "https://github.com/zoffyultraman/Hexrs/releases/tag/v0.0.1"
                    .to_owned(),
            }
        );
        assert_eq!(
            evaluate_releases(&stable_and_legacy_alpha, "v0.0.1-5-g1234567"),
            AppUpdateOutcome::UpToDate {
                current_version: "v0.0.1-5-g1234567".to_owned(),
                latest_version: "v0.0.1".to_owned(),
                release_url: "https://github.com/zoffyultraman/Hexrs/releases/tag/v0.0.1"
                    .to_owned(),
            }
        );
        for legacy_cur in ["alpha_0.0.4", "alpha_0.0.4-12-g9fcc012"] {
            assert_eq!(
                evaluate_releases(&stable_and_legacy_alpha, legacy_cur),
                AppUpdateOutcome::UpdateAvailable {
                    current_version: legacy_cur.to_owned(),
                    latest_version: "v0.0.1".to_owned(),
                    release_url: "https://github.com/zoffyultraman/Hexrs/releases/tag/v0.0.1"
                        .to_owned(),
                    download_url: Some("https://example.com/Hexrs-v0.0.1.zip".to_owned()),
                    is_prerelease: false,
                    published_at: "2026-11-29T10:00:00Z".to_owned(),
                }
            );
        }

        // 2. Only [alpha_0.0.4 (prerelease)] (no stable published yet) -> stable v0.0.1 stays UpToDate.
        let alpha_only = vec![make_rel("alpha_0.0.4", true, "2026-11-26T10:00:00Z")];
        assert_eq!(
            evaluate_releases(&alpha_only, "v0.0.1"),
            AppUpdateOutcome::UpToDate {
                current_version: "v0.0.1".to_owned(),
                latest_version: "v0.0.1".to_owned(),
                release_url: DEFAULT_RELEASES_PAGE_URL.to_owned(),
            }
        );

        // 3. [beta_0.0.2 (prerelease, 2026-12-01), v0.0.1 (stable, 2026-11-29)]
        let newer_beta_and_stable = vec![
            make_rel("beta_0.0.2", true, "2026-12-01T10:00:00Z"),
            make_rel("v0.0.1", false, "2026-11-29T10:00:00Z"),
        ];
        assert_eq!(
            evaluate_releases(&newer_beta_and_stable, "v0.0.1"),
            AppUpdateOutcome::UpToDate {
                current_version: "v0.0.1".to_owned(),
                latest_version: "v0.0.1".to_owned(),
                release_url: "https://github.com/zoffyultraman/Hexrs/releases/tag/v0.0.1"
                    .to_owned(),
            }
        );
        assert_eq!(
            evaluate_releases(&newer_beta_and_stable, "beta_0.0.2"),
            AppUpdateOutcome::UpToDate {
                current_version: "beta_0.0.2".to_owned(),
                latest_version: "beta_0.0.2".to_owned(),
                release_url: "https://github.com/zoffyultraman/Hexrs/releases/tag/beta_0.0.2"
                    .to_owned(),
            }
        );

        // 4. [v0.0.2 (stable, 2026-12-02), v0.0.1 (stable, 2026-11-29), alpha_0.0.4]
        let two_stables_and_alpha = vec![
            make_rel("v0.0.2", false, "2026-12-02T10:00:00Z"),
            make_rel("v0.0.1", false, "2026-11-29T10:00:00Z"),
            make_rel("alpha_0.0.4", true, "2026-11-26T10:00:00Z"),
        ];
        assert_eq!(
            evaluate_releases(&two_stables_and_alpha, "v0.0.1"),
            AppUpdateOutcome::UpdateAvailable {
                current_version: "v0.0.1".to_owned(),
                latest_version: "v0.0.2".to_owned(),
                release_url: "https://github.com/zoffyultraman/Hexrs/releases/tag/v0.0.2"
                    .to_owned(),
                download_url: Some("https://example.com/Hexrs-v0.0.2.zip".to_owned()),
                is_prerelease: false,
                published_at: "2026-12-02T10:00:00Z".to_owned(),
            }
        );
    }
}
