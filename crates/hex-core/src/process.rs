//! Process monitor for `LeagueClientUx.exe` and `League of Legends.exe` via `sysinfo` + lockfile fallback.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tracing::{debug, info};

const LCU_PROCESS_NAMES: &[&str] = &["LeagueClientUx.exe", "LeagueClientUx"];
const GAME_PROCESS_NAMES: &[&str] = &["League of Legends.exe", "League of Legends"];

/// Local LCU API connection credentials extracted from process arguments or lockfile.
#[derive(Clone, PartialEq, Eq)]
pub struct LcuCredentials {
    pub port: u16,
    pub token: String,
    pub pid: u32,
}

impl fmt::Debug for LcuCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LcuCredentials")
            .field("port", &self.port)
            .field("token", &"<redacted>")
            .field("pid", &self.pid)
            .finish()
    }
}

/// Snapshot of currently running League client and game processes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProcessSnapshot {
    pub lcu: Option<LcuCredentials>,
    pub lcu_pid: Option<u32>,
    pub lcu_pids: Vec<u32>,
    pub game_running: bool,
    pub game_pid: Option<u32>,
}

/// Polls OS processes without reading game memory or injecting into target processes.
pub struct ProcessMonitor {
    system: System,
    last_lcu_port: Option<u16>,
    last_game_running: bool,
}

impl Default for ProcessMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessMonitor {
    /// Creates a new process monitor backed by `sysinfo::System`.
    pub fn new() -> Self {
        Self {
            system: System::new(),
            last_lcu_port: None,
            last_game_running: false,
        }
    }

    /// Refreshes process metadata (including cmdlines and exe paths) and extracts LCU/game state.
    pub fn poll(&mut self) -> ProcessSnapshot {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_cmd(UpdateKind::OnlyIfNotSet)
                .with_exe(UpdateKind::OnlyIfNotSet),
        );

        let mut snapshot = ProcessSnapshot::default();
        let mut lcu_exe_paths: Vec<PathBuf> = Vec::new();

        for process in self.system.processes().values() {
            let name = process.name().to_string_lossy();
            let pid = process.pid().as_u32();

            if matches_any_ignore_case(&name, LCU_PROCESS_NAMES) {
                snapshot.lcu_pids.push(pid);
                if let Some((port, token)) = parse_cmd_credentials(process.cmd()) {
                    snapshot.lcu = Some(LcuCredentials { port, token, pid });
                    snapshot.lcu_pid = Some(pid);
                } else if snapshot.lcu_pid.is_none() {
                    snapshot.lcu_pid = Some(pid);
                }

                if let Some(exe) = process.exe() {
                    lcu_exe_paths.push(exe.to_path_buf());
                }
            } else if matches_any_ignore_case(&name, GAME_PROCESS_NAMES) {
                snapshot.game_running = true;
                snapshot.game_pid = Some(pid);
            }
        }

        if snapshot.lcu.is_none() {
            if let Some(creds) = try_lockfile_fallback(&self.system, &lcu_exe_paths, snapshot.lcu_pid)
            {
                debug!(
                    pid = creds.pid,
                    port = creds.port,
                    "resolved LCU credentials via lockfile fallback"
                );
                if snapshot.lcu_pid.is_none() {
                    snapshot.lcu_pid = Some(creds.pid);
                }
                snapshot.lcu = Some(creds);
            }
        }

        let current_lcu_port = snapshot.lcu.as_ref().map(|c| c.port);
        if current_lcu_port != self.last_lcu_port {
            match (&snapshot.lcu, self.last_lcu_port) {
                (Some(creds), _) => {
                    info!(
                        pid = creds.pid,
                        port = creds.port,
                        lcu_pid_count = snapshot.lcu_pids.len(),
                        "detected LCU client credentials"
                    );
                }
                (None, Some(prev_port)) => {
                    info!(prev_port, "LCU client exited");
                }
                (None, None) => {}
            }
            self.last_lcu_port = current_lcu_port;
        }

        if snapshot.game_running != self.last_game_running {
            if snapshot.game_running {
                info!(pid = ?snapshot.game_pid, "detected League of Legends.exe game process");
            } else {
                info!("League of Legends.exe game process exited");
            }
            self.last_game_running = snapshot.game_running;
        }

        snapshot
    }
}

fn matches_any_ignore_case(candidate: &str, targets: &[&str]) -> bool {
    targets
        .iter()
        .any(|target| candidate.eq_ignore_ascii_case(target))
}

/// Extracts `(--app-port, --remoting-auth-token)` from `LeagueClientUx.exe` command-line args.
pub fn parse_cmd_credentials(args: &[OsString]) -> Option<(u16, String)> {
    let mut port: Option<u16> = None;
    let mut token: Option<String> = None;

    for raw in args {
        let arg = raw.to_string_lossy();
        let trimmed = arg.trim().trim_matches('"');
        if let Some(val) = trimmed.strip_prefix("--app-port=") {
            if let Ok(parsed) = val.trim_matches('"').parse::<u16>() {
                port = Some(parsed);
            }
        } else if let Some(val) = trimmed.strip_prefix("--remoting-auth-token=") {
            let cleaned = val.trim_matches('"');
            if !cleaned.is_empty() {
                token = Some(cleaned.to_owned());
            }
        }
    }

    match (port, token) {
        (Some(p), Some(t)) => Some((p, t)),
        _ => None,
    }
}

/// Parses a Riot/WeGame LCU `lockfile` (`name:pid:port:token:protocol`).
pub fn parse_lockfile_contents(contents: &str) -> Option<(u32, u16, String)> {
    let line = contents.lines().next()?.trim();
    let mut parts = line.split(':');
    let _name = parts.next()?;
    let pid = parts.next()?.trim().parse::<u32>().ok()?;
    let port = parts.next()?.trim().parse::<u16>().ok()?;
    let token = parts.next()?.trim();
    let _protocol = parts.next()?;

    if token.is_empty() {
        return None;
    }

    Some((pid, port, token.to_owned()))
}

fn try_lockfile_fallback(
    system: &System,
    lcu_exe_paths: &[PathBuf],
    observed_pid: Option<u32>,
) -> Option<LcuCredentials> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut push_unique = |path: PathBuf| {
        if seen.insert(path.clone()) {
            candidates.push(path);
        }
    };

    for exe in lcu_exe_paths {
        if let Some(dir) = exe.parent() {
            push_unique(dir.join("lockfile"));
            if let Some(parent) = dir.parent() {
                push_unique(parent.join("lockfile"));
                push_unique(parent.join("LeagueClient").join("lockfile"));
            }
        }
    }

    for install_dir in query_registry_install_paths() {
        push_unique(install_dir.join("LeagueClient").join("lockfile"));
        push_unique(install_dir.join("lockfile"));
        if let Some(parent) = install_dir.parent() {
            push_unique(parent.join("LeagueClient").join("lockfile"));
        }
    }

    for drive in 'C'..='Z' {
        let root = format!("{drive}:\\");
        let root_path = Path::new(&root);
        if !root_path.exists() {
            continue;
        }
        for rel in [
            r"WeGameApps\英雄联盟\LeagueClient\lockfile",
            r"WeGameApps\英雄联盟\lockfile",
            r"英雄联盟\LeagueClient\lockfile",
            r"Riot Games\League of Legends\lockfile",
            r"Program Files\Riot Games\League of Legends\lockfile",
            r"Program Files (x86)\WeGameApps\英雄联盟\LeagueClient\lockfile",
            r"Program Files (x86)\WeGameApps\英雄联盟\lockfile",
            r"Games\WeGameApps\英雄联盟\LeagueClient\lockfile",
            r"Program Files\Tencent Games\英雄联盟\LeagueClient\lockfile",
        ] {
            push_unique(root_path.join(rel));
        }
    }

    for path in candidates {
        if let Ok(contents) = fs::read_to_string(&path) {
            if let Some((lock_pid, port, token)) = parse_lockfile_contents(&contents) {
                let pid_alive =
                    observed_pid.is_some() || system.process(Pid::from_u32(lock_pid)).is_some();
                if pid_alive {
                    return Some(LcuCredentials {
                        port,
                        token,
                        pid: observed_pid.unwrap_or(lock_pid),
                    });
                }
            }
        }
    }

    None
}

#[cfg(target_os = "windows")]
fn query_registry_install_paths() -> Vec<PathBuf> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
        HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
    };

    const QUERIES: &[(HKEY, &str, &str)] = &[
        (HKEY_CURRENT_USER, r"Software\Tencent\LOL", "InstallPath"),
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\WOW6432Node\Tencent\LOL",
            "InstallPath",
        ),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Tencent\LOL", "InstallPath"),
        (
            HKEY_CURRENT_USER,
            r"Software\Riot Games\League of Legends",
            "InstallPath",
        ),
        (
            HKEY_CURRENT_USER,
            r"Software\Riot Games\League of Legends",
            "Path",
        ),
    ];

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn read_reg_sz(root: HKEY, subkey: &str, value_name: &str) -> Option<String> {
        let subkey_w = to_wide(subkey);
        let value_w = to_wide(value_name);
        let mut hkey = HKEY::default();

        let open_err = unsafe {
            RegOpenKeyExW(
                root,
                PCWSTR::from_raw(subkey_w.as_ptr()),
                None,
                KEY_READ,
                &mut hkey,
            )
        };
        if open_err != ERROR_SUCCESS {
            return None;
        }

        let mut val_type = REG_VALUE_TYPE::default();
        let mut byte_len: u32 = 0;
        let size_err = unsafe {
            RegQueryValueExW(
                hkey,
                PCWSTR::from_raw(value_w.as_ptr()),
                None,
                Some(&mut val_type),
                None,
                Some(&mut byte_len),
            )
        };
        if size_err != ERROR_SUCCESS
            || byte_len == 0
            || (val_type != REG_SZ && val_type != REG_EXPAND_SZ)
        {
            unsafe {
                let _ = RegCloseKey(hkey);
            }
            return None;
        }

        let mut buf = vec![0_u16; (byte_len as usize / 2) + 2];
        let mut actual_len = (buf.len() * 2) as u32;
        let query_err = unsafe {
            RegQueryValueExW(
                hkey,
                PCWSTR::from_raw(value_w.as_ptr()),
                None,
                Some(&mut val_type),
                Some(buf.as_mut_ptr() as *mut u8),
                Some(&mut actual_len),
            )
        };
        unsafe {
            let _ = RegCloseKey(hkey);
        }

        if query_err != ERROR_SUCCESS {
            return None;
        }

        let u16_len = (actual_len as usize / 2).min(buf.len());
        let decoded = String::from_utf16_lossy(&buf[..u16_len]);
        let trimmed = decoded.trim_end_matches('\0').trim().to_owned();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }

    let mut paths = Vec::new();
    for &(root, subkey, value_name) in QUERIES {
        if let Some(raw) = read_reg_sz(root, subkey, value_name) {
            paths.push(PathBuf::from(raw));
        }
    }
    paths
}

#[cfg(not(target_os = "windows"))]
fn query_registry_install_paths() -> Vec<PathBuf> {
    Vec::new()
}
