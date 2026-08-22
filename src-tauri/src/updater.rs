//! Self-update from GitHub Releases — no installer, no downtime.
//!
//! Standalone binaries make this simple: the new version is downloaded to a
//! VERSIONED SIBLING FILE next to the running exe (never overwriting it — Windows
//! locks running images anyway), then spawned. The successor performs the standard
//! two-phase takeover (warm GPU → /handover → old instance stops sACN and exits),
//! so an update is a ~one-frame hot-swap even mid-show. Old versioned binaries are
//! deleted on later startups.
//!
//! Auto-CHECK is on by default (every 6 h + at startup); auto-INSTALL is opt-in —
//! the swap is seamless, but whether to take an update mid-show is the operator's
//! call. Both are also triggerable from the UI.

use crate::state::SharedState;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

const REPO: &str = "cinderblock/empyrean-gate";
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 3600);

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

fn effective_version() -> String {
    // Test hook: fake a lower running version to exercise the full update path.
    std::env::var("EMPYREAN_FAKE_VERSION").unwrap_or_else(|_| CURRENT_VERSION.to_string())
}

fn asset_name() -> Option<&'static str> {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("empyrean-gate-windows-x64.exe")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("empyrean-gate-linux-x64")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("empyrean-gate-macos-arm64")
    } else {
        None
    }
}

fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim_start_matches('v');
    let mut it = v.split('.').map(|p| p.parse::<u32>().ok());
    Some((it.next()??, it.next()??, it.next()??))
}

fn set_update_status(state: &SharedState, available: Option<String>, note: &str) {
    let mut st = state.status.lock();
    st.update_available = available;
    st.update_state = note.to_string();
    drop(st);
    // Nudge clients so the panel refreshes promptly (status also ticks at 2 Hz).
    state.broadcast_state();
}

pub fn spawn(state: Arc<SharedState>) {
    std::thread::Builder::new()
        .name("updater".into())
        .spawn(move || updater_thread(state))
        .expect("spawn updater thread");
}

fn updater_thread(state: Arc<SharedState>) {
    // First auto-check shortly after startup, then every CHECK_INTERVAL.
    let mut next_check = Instant::now() + Duration::from_secs(30);
    let mut latest: Option<(String, String)> = None; // (version, download url)

    while !state.shutdown.load(Ordering::Relaxed) {
        let manual_check = state.update_check_requested.swap(false, Ordering::SeqCst);
        let install = state.update_install_requested.swap(false, Ordering::SeqCst);
        let auto_check = state.config.read().update.auto_check;

        if manual_check || (auto_check && Instant::now() >= next_check) {
            next_check = Instant::now() + CHECK_INTERVAL;
            match check_latest() {
                Ok(Some((version, url))) => {
                    if is_newer(&version) {
                        log::info!("update available: v{version} (running v{})", effective_version());
                        latest = Some((version.clone(), url));
                        set_update_status(&state, Some(version), "");
                        if state.config.read().update.auto_install {
                            state.update_install_requested.store(true, Ordering::SeqCst);
                        }
                    } else {
                        latest = None;
                        set_update_status(&state, None, "up to date");
                    }
                }
                Ok(None) => set_update_status(&state, None, "no release found"),
                Err(e) => {
                    log::warn!("update check failed: {e:#}");
                    set_update_status(&state, None, &format!("check failed: {e}"));
                }
            }
        }

        if install {
            if let Some((version, url)) = latest.clone() {
                set_update_status(&state, Some(version.clone()), "downloading…");
                match download_and_launch(&version, &url, &state) {
                    Ok(()) => {
                        // The successor's takeover will shut us down; just wait.
                        set_update_status(&state, Some(version), "handing over…");
                    }
                    Err(e) => {
                        log::error!("update install failed: {e:#}");
                        set_update_status(&state, Some(version), &format!("install failed: {e}"));
                    }
                }
            } else {
                set_update_status(&state, None, "no update staged — check first");
            }
        }

        std::thread::sleep(Duration::from_millis(500));
    }
}

fn is_newer(candidate: &str) -> bool {
    match (parse_version(candidate), parse_version(&effective_version())) {
        (Some(c), Some(cur)) => c > cur,
        _ => false,
    }
}

/// Latest release's (version, asset download url) for this platform.
fn check_latest() -> anyhow::Result<Option<(String, String)>> {
    let Some(asset) = asset_name() else {
        anyhow::bail!("no release asset for this platform");
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let mut resp = agent
        .get(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header("User-Agent", "empyrean-gate-updater")
        .call()?;
    let body: serde_json::Value = resp.body_mut().read_json()?;
    let tag = body["tag_name"].as_str().unwrap_or_default();
    let version = tag.trim_start_matches('v').to_string();
    if version.is_empty() {
        return Ok(None);
    }
    let url = body["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"].as_str() == Some(asset))
        .and_then(|a| a["browser_download_url"].as_str())
        .map(str::to_string);
    match url {
        Some(url) => Ok(Some((version, url))),
        None => anyhow::bail!("release v{version} has no asset '{asset}'"),
    }
}

fn versioned_path(version: &str) -> anyhow::Result<PathBuf> {
    let current = std::env::current_exe()?;
    let dir = current
        .parent()
        .ok_or_else(|| anyhow::anyhow!("current exe has no parent dir"))?;
    let ext = if cfg!(windows) { ".exe" } else { "" };
    Ok(dir.join(format!("empyrean-gate-v{version}{ext}")))
}

/// Download the new binary next to the current one and launch it; the successor
/// takes over via the standard two-phase handover and this process exits.
fn download_and_launch(version: &str, url: &str, state: &SharedState) -> anyhow::Result<()> {
    let target = versioned_path(version)?;
    let tmp = target.with_extension("download");

    log::info!("downloading v{version} from {url}");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(600)))
        .build()
        .into();
    let mut resp = agent
        .get(url)
        .header("User-Agent", "empyrean-gate-updater")
        .call()?;
    let mut reader = resp.body_mut().as_reader();
    let mut file = std::fs::File::create(&tmp)
        .map_err(|e| anyhow::anyhow!("cannot write next to the current exe ({e}); is the directory writable?"))?;
    let bytes = std::io::copy(&mut reader, &mut file)?;
    drop(file);
    anyhow::ensure!(
        bytes > 1_000_000,
        "downloaded file is implausibly small ({bytes} bytes)"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, &target)?;
    log::info!("downloaded {} ({bytes} bytes); launching successor", target.display());

    let mut cmd = std::process::Command::new(&target);
    if state.headless.load(Ordering::Relaxed) {
        cmd.arg("--headless");
    }
    cmd.spawn()
        .map_err(|e| anyhow::anyhow!("failed to launch {}: {e}", target.display()))?;
    Ok(())
}

/// Delete versioned sibling binaries older than the running version. The running
/// image can't be deleted on Windows (locked) and is skipped anyway; failures are
/// ignored — cleanup is best-effort.
pub(crate) fn cleanup_old_binaries() {
    let Ok(current_exe) = std::env::current_exe() else { return };
    let Some(dir) = current_exe.parent() else { return };
    let Some(cur) = parse_version(&effective_version()) else { return };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(rest) = name.strip_prefix("empyrean-gate-v") else { continue };
        let version_part = rest.trim_end_matches(".exe");
        if let Some(v) = parse_version(version_part) {
            if v < cur && entry.path() != current_exe {
                match std::fs::remove_file(entry.path()) {
                    Ok(()) => log::info!("cleaned up old binary {name}"),
                    Err(_) => {} // probably still running (mid-handover); next boot
                }
            }
        }
    }
}
