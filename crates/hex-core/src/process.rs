//! Process monitor for `LeagueClientUx.exe` and `League of Legends.exe` via `sysinfo`.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use sysinfo::{ProcessesToUpdate, System};
use tracing::debug;

const LCU_PROCESS_NAMES: &[&str] = &["LeagueClientUx.exe", "LeagueClientUx"];
const GAME_PROCESS_NAMES: &[&str] = &["League of Legends.exe", "League of Legends"];

const FALLBACK_LOCKFILE_PATHS: &[&str] = &[
    r"C:\Riot Games\League of Legends\lockfile",
    r"D:\Riot Games\League of Legends\lockfile",
    r"C:\Program Files\Riot Games\League of Legends\lockfile",
    r"C:\WeGameApps\英雄联盟\LeagueClient\lockfile",
    r"D:\WeGameApps\英雄联盟\LeagueClient\lockfile",
    r"C:\Program Files (x86)\WeGameApps\英雄联盟\LeagueClient\lockfile",
    r"D:\Program Files (x86)\WeGameApps\英雄联盟\LeagueClient\lockfile",
];

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
    pub game_running: bool,
    pub game_pid: Option<u32>,
}

/// Polls OS processes without reading game memory or injecting into target processes.
pub struct ProcessMonitor {
    system: System,
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
        }
    }

    /// Refreshes process metadata and extracts LCU credentials and game process state.
    pub fn poll(&mut self) -> ProcessSnapshot {
        self.system.refresh_processes(ProcessesToUpdate::All, true);

        let mut snapshot = ProcessSnapshot::default();
        let mut lcu_exe_path: Option<PathBuf> = None;

        for process in self.system.processes().values() {
            let name = process.name().to_string_lossy();
            let pid = process.pid().as_u32();

            if matches_any_ignore_case(&name, LCU_PROCESS_NAMES) {
                snapshot.lcu_pid = Some(pid);
                if let Some((port, token)) = parse_cmd_credentials(process.cmd()) {
                    snapshot.lcu = Some(LcuCredentials { port, token, pid });
                } else if lcu_exe_path.is_none() {
                    lcu_exe_path = process.exe().map(Path::to_path_buf);
                }
            } else if matches_any_ignore_case(&name, GAME_PROCESS_NAMES) {
                snapshot.game_running = true;
                snapshot.game_pid = Some(pid);
            }
        }

        if snapshot.lcu.is_none() && snapshot.lcu_pid.is_some() {
            if let Some(creds) = try_lockfile_fallback(lcu_exe_path.as_deref(), snapshot.lcu_pid) {
                debug!(
                    pid = creds.pid,
                    port = creds.port,
                    "resolved LCU credentials via lockfile fallback"
                );
                snapshot.lcu = Some(creds);
            }
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
    lcu_exe_path: Option<&Path>,
    observed_pid: Option<u32>,
) -> Option<LcuCredentials> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(exe) = lcu_exe_path {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("lockfile"));
            if let Some(parent) = dir.parent() {
                candidates.push(parent.join("lockfile"));
            }
        }
    }
    for path in FALLBACK_LOCKFILE_PATHS {
        candidates.push(PathBuf::from(path));
    }

    for path in candidates {
        if let Ok(contents) = fs::read_to_string(&path) {
            if let Some((lock_pid, port, token)) = parse_lockfile_contents(&contents) {
                return Some(LcuCredentials {
                    port,
                    token,
                    pid: observed_pid.unwrap_or(lock_pid),
                });
            }
        }
    }

    None
}
