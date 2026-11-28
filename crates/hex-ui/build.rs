use std::process::Command;

fn run_git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn main() {
    println!("cargo:rerun-if-env-changed=HEXRS_APP_VERSION");
    println!("cargo:rerun-if-env-changed=HEXRS_GIT_SHA");

    let app_version = non_empty_env("HEXRS_APP_VERSION")
        .or_else(|| run_git(&["describe", "--tags", "--exact-match"]))
        .or_else(|| run_git(&["describe", "--tags", "--abbrev=7", "--always"]))
        .unwrap_or_else(|| format!("v{}", env!("CARGO_PKG_VERSION")));

    let git_sha = non_empty_env("HEXRS_GIT_SHA")
        .or_else(|| run_git(&["rev-parse", "--short=7", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_owned());

    println!("cargo:rustc-env=HEXRS_APP_VERSION={app_version}");
    println!("cargo:rustc-env=HEXRS_GIT_SHA={git_sha}");
}
