//! Process monitor for `LeagueClientUx.exe`, `LeagueClient.exe`, and `League of Legends.exe`
//! using `PROCESS_QUERY_LIMITED_INFORMATION` (zero `ReadProcessMemory`) + shared lockfile fallback.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tracing::{debug, info};

const LCU_PROCESS_NAMES: &[&str] = &["LeagueClientUx.exe", "LeagueClientUx"];
const CLIENT_PROCESS_NAMES: &[&str] = &["LeagueClient.exe", "LeagueClient"];
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
    pub client_pids: Vec<u32>,
    pub game_running: bool,
    pub game_pid: Option<u32>,
    pub is_elevated: bool,
    pub lcu_diagnostic: Option<String>,
}

/// Returns whether the current process is running with elevated (Administrator) privileges.
#[cfg(target_os = "windows")]
pub fn is_current_process_elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = HANDLE::default();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }

    let mut elevation = TOKEN_ELEVATION::default();
    let mut ret_len: u32 = 0;
    let res = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        )
    };
    unsafe {
        let _ = CloseHandle(token);
    }

    res.is_ok() && elevation.TokenIsElevated != 0
}

/// Non-Windows stub returning `false`.
#[cfg(not(target_os = "windows"))]
pub fn is_current_process_elevated() -> bool {
    false
}

/// Polls OS processes without reading target process memory or injecting into target processes.
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

    /// Enumerates PIDs/names via `sysinfo` (no `ReadProcessMemory`) and queries cmdlines/paths natively.
    pub fn poll(&mut self) -> ProcessSnapshot {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );

        let mut snapshot = ProcessSnapshot {
            is_elevated: is_current_process_elevated(),
            ..Default::default()
        };

        for process in self.system.processes().values() {
            let name = process.name().to_string_lossy();
            let pid = process.pid().as_u32();

            if matches_any_ignore_case(&name, LCU_PROCESS_NAMES) {
                snapshot.lcu_pids.push(pid);
            } else if matches_any_ignore_case(&name, CLIENT_PROCESS_NAMES) {
                snapshot.client_pids.push(pid);
            } else if matches_any_ignore_case(&name, GAME_PROCESS_NAMES) {
                snapshot.game_running = true;
                snapshot.game_pid = Some(pid);
            }
        }

        if let Some(&first_ux) = snapshot.lcu_pids.first() {
            snapshot.lcu_pid = Some(first_ux);
        }

        let mut cmdline_errors: Vec<String> = Vec::new();
        for &pid in &snapshot.lcu_pids {
            match query_process_cmdline_limited(pid) {
                Ok(cmdline) => {
                    if let Some((port, token)) = parse_raw_cmdline_credentials(&cmdline) {
                        snapshot.lcu = Some(LcuCredentials { port, token, pid });
                        snapshot.lcu_pid = Some(pid);
                        break;
                    }
                }
                Err(err) => {
                    cmdline_errors.push(format!("PID {pid}: {err}"));
                }
            }
        }

        if snapshot.lcu.is_none() {
            let mut exe_paths: Vec<PathBuf> = Vec::new();
            for &pid in snapshot.lcu_pids.iter().chain(snapshot.client_pids.iter()) {
                if let Some(path) = query_process_image_path_limited(pid) {
                    exe_paths.push(path);
                }
            }

            let (lock_creds, checked_paths) = try_lockfile_fallback(
                &self.system,
                &exe_paths,
                snapshot.lcu_pid,
                &snapshot.client_pids,
            );
            if let Some(creds) = lock_creds {
                debug!(
                    pid = creds.pid,
                    port = creds.port,
                    "resolved LCU credentials via lockfile fallback"
                );
                if snapshot.lcu_pid.is_none() {
                    snapshot.lcu_pid = Some(creds.pid);
                }
                snapshot.lcu = Some(creds);
            } else if !snapshot.lcu_pids.is_empty() || !snapshot.client_pids.is_empty() {
                let mut parts = cmdline_errors;
                parts.push(format!("lockfile未命中({checked_paths}路径)"));
                snapshot.lcu_diagnostic = Some(parts.join("; "));
            } else {
                snapshot.lcu_diagnostic = Some("未检测到 LeagueClientUx.exe".to_owned());
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
                        is_elevated = snapshot.is_elevated,
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

/// Extracts `(--app-port, --remoting-auth-token)` from a raw command-line string.
pub fn parse_raw_cmdline_credentials(cmdline: &str) -> Option<(u16, String)> {
    let port = extract_flag_value(cmdline, "--app-port=")
        .and_then(|val| val.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse::<u16>().ok());
    let token = extract_flag_value(cmdline, "--remoting-auth-token=")
        .map(|val| val.to_owned())
        .filter(|s| !s.is_empty());

    match (port, token) {
        (Some(p), Some(t)) => Some((p, t)),
        _ => None,
    }
}

fn extract_flag_value<'a>(cmdline: &'a str, flag: &str) -> Option<&'a str> {
    let start = cmdline.find(flag)? + flag.len();
    let rest = &cmdline[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
        .unwrap_or(rest.len());
    let slice = &rest[..end];
    if slice.is_empty() {
        None
    } else {
        Some(slice)
    }
}

/// Extracts `(--app-port, --remoting-auth-token)` from an `OsString` argument slice.
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

#[cfg(target_os = "windows")]
fn read_lockfile_shared(path: &Path) -> std::io::Result<String> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_ALL: u32 = 0x0000_0007; // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE

    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_ALL)
        .open(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok(contents)
}

#[cfg(not(target_os = "windows"))]
fn read_lockfile_shared(path: &Path) -> std::io::Result<String> {
    let mut file = OpenOptions::new().read(true).open(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok(contents)
}

#[cfg(target_os = "windows")]
fn query_process_cmdline_limited(pid: u32) -> std::result::Result<String, String> {
    use windows::Win32::Foundation::{CloseHandle, GetLastError};
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;
    const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
    const STATUS_BUFFER_TOO_SMALL: i32 = 0xC000_0023_u32 as i32;
    const STATUS_BUFFER_OVERFLOW: i32 = 0x8000_0005_u32 as i32;

    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryInformationProcess(
            ProcessHandle: *mut core::ffi::c_void,
            ProcessInformationClass: u32,
            ProcessInformation: *mut core::ffi::c_void,
            ProcessInformationLength: u32,
            ReturnLength: *mut u32,
        ) -> i32;
    }

    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *const u16,
    }

    let handle = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
        Ok(h) => h,
        Err(_) => {
            let err_code = unsafe { GetLastError().0 };
            return if err_code == 5 {
                Err("OpenProcess err=5 (请以管理员身份运行)".to_owned())
            } else {
                Err(format!("OpenProcess err={err_code}"))
            };
        }
    };

    let mut buf = vec![0_u64; 1024]; // 8192 bytes, 8-byte aligned for UNICODE_STRING
    let mut status: i32 = -1;

    for _ in 0..2 {
        let mut return_len: u32 = 0;
        let buf_bytes = (buf.len() * std::mem::size_of::<u64>()) as u32;
        status = unsafe {
            NtQueryInformationProcess(
                handle.0,
                PROCESS_COMMAND_LINE_INFORMATION,
                buf.as_mut_ptr().cast(),
                buf_bytes,
                &mut return_len,
            )
        };

        if status == STATUS_INFO_LENGTH_MISMATCH
            || status == STATUS_BUFFER_TOO_SMALL
            || status == STATUS_BUFFER_OVERFLOW
        {
            let needed_bytes = (return_len as usize).max(buf_bytes as usize * 2);
            let needed_u64s = needed_bytes.div_ceil(std::mem::size_of::<u64>());
            buf.resize(needed_u64s, 0);
            continue;
        }
        break;
    }

    let result = if status >= 0 {
        let buf_start = buf.as_ptr() as usize;
        let buf_end = buf_start + buf.len() * std::mem::size_of::<u64>();
        if buf_end - buf_start < std::mem::size_of::<UnicodeString>() {
            Err("NtQuery(60) buffer too small".to_owned())
        } else {
            let ustr = unsafe { &*(buf.as_ptr() as *const UnicodeString) };
            let u16_len = (ustr.length as usize) / 2;
            let ptr_addr = ustr.buffer as usize;
            let ptr_end = ptr_addr.saturating_add(u16_len * std::mem::size_of::<u16>());

            if ustr.buffer.is_null() || u16_len == 0 {
                Err("NtQuery(60) 返回空命令行".to_owned())
            } else if ptr_addr < buf_start || ptr_end > buf_end {
                Err("NtQuery(60) UNICODE_STRING 越界".to_owned())
            } else {
                let slice = unsafe { std::slice::from_raw_parts(ustr.buffer, u16_len) };
                Ok(String::from_utf16_lossy(slice))
            }
        }
    } else {
        Err(format!("NtQuery(60) status=0x{:08X}", status as u32))
    };

    unsafe {
        let _ = CloseHandle(handle);
    }

    result
}

#[cfg(not(target_os = "windows"))]
fn query_process_cmdline_limited(_pid: u32) -> std::result::Result<String, String> {
    Err("non-windows platform".to_owned())
}

#[cfg(target_os = "windows")]
fn query_process_image_path_limited(pid: u32) -> Option<PathBuf> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = vec![0_u16; 1024];
    let mut len = buf.len() as u32;
    let res = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    unsafe {
        let _ = CloseHandle(handle);
    }
    res.ok()?;

    let path_str = String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]);
    if path_str.is_empty() {
        None
    } else {
        Some(PathBuf::from(path_str))
    }
}

#[cfg(not(target_os = "windows"))]
fn query_process_image_path_limited(_pid: u32) -> Option<PathBuf> {
    None
}

fn try_lockfile_fallback(
    system: &System,
    exe_paths: &[PathBuf],
    observed_pid: Option<u32>,
    client_pids: &[u32],
) -> (Option<LcuCredentials>, usize) {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut push_unique = |path: PathBuf| {
        if seen.insert(path.clone()) {
            candidates.push(path);
        }
    };

    for exe in exe_paths {
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

    let checked_count = candidates.len();
    for path in candidates {
        if let Ok(contents) = read_lockfile_shared(&path) {
            if let Some((lock_pid, port, token)) = parse_lockfile_contents(&contents) {
                let pid_alive = observed_pid.is_some()
                    || !client_pids.is_empty()
                    || system.process(Pid::from_u32(lock_pid)).is_some();
                if pid_alive {
                    return (
                        Some(LcuCredentials {
                            port,
                            token,
                            pid: observed_pid.unwrap_or(lock_pid),
                        }),
                        checked_count,
                    );
                }
            }
        }
    }

    (None, checked_count)
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
                Some(buf.as_mut_ptr().cast()),
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
