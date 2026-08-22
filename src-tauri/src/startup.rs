//! Per-user Windows launch-at-login integration.
//!
//! The shortcut is refreshed on every launch, before updater cleanup. That matters
//! because self-updates run from versioned sibling executables: a shortcut created
//! by an older version must be retargeted before that old binary is removed.

#[cfg(any(windows, test))]
const SHORTCUT_NAME: &str = "Empyrean Gate.lnk";

pub struct StartupOutcome {
    pub supported: bool,
    pub enabled: bool,
    pub state: String,
    pub succeeded: bool,
}

impl StartupOutcome {
    pub fn publish(self, status: &mut crate::protocol::RuntimeStatus) {
        status.startup_supported = self.supported;
        status.startup_enabled = self.enabled;
        status.startup_state = self.state;
    }
}

pub fn supported() -> bool {
    cfg!(windows)
}

/// Reflect the configured preference into the OS and update the user-facing status.
/// Non-Windows builds deliberately report an unsupported no-op.
pub fn reconcile(enabled: bool, headless: bool) -> StartupOutcome {
    match set_enabled(enabled, headless) {
        Ok(actual) => StartupOutcome {
            supported: supported(),
            enabled: actual,
            state: if supported() {
                if actual {
                    "Launches automatically for this Windows user.".into()
                } else {
                    "Launch at startup is off.".into()
                }
            } else {
                "Launch at startup is only supported on Windows; no changes were made.".into()
            },
            succeeded: supported(),
        },
        Err(e) => StartupOutcome {
            supported: supported(),
            enabled: shortcut_exists(),
            state: format!("Could not update launch at startup: {e}"),
            succeeded: false,
        },
    }
}

pub fn set_enabled(enabled: bool, headless: bool) -> anyhow::Result<bool> {
    #[cfg(windows)]
    {
        let shortcut = shortcut_path()?;
        if !enabled {
            match std::fs::remove_file(&shortcut) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            return Ok(false);
        }

        let exe = std::env::current_exe()?;
        let script = std::env::temp_dir().join(format!(
            "empyrean-gate-startup-{}.ps1",
            uuid::Uuid::new_v4()
        ));
        let mut script_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&script)?;
        std::io::Write::write_all(
            &mut script_file,
            r#"param([string]$ShortcutPath, [string]$TargetPath, [string]$LaunchArgs)
$ErrorActionPreference = 'Stop'
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($ShortcutPath)
$shortcut.TargetPath = $TargetPath
$shortcut.WorkingDirectory = Split-Path -Parent $TargetPath
$shortcut.Arguments = $LaunchArgs
$shortcut.Description = 'Empyrean Gate lighting controller'
$shortcut.Save()
"#
            .as_bytes(),
        )?;
        drop(script_file);
        let launch_args = if headless { "--headless" } else { "" };
        let result = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&script)
            .arg(&shortcut)
            .arg(&exe)
            .arg(launch_args)
            .status();
        let _ = std::fs::remove_file(&script);
        let result = result?;
        anyhow::ensure!(result.success(), "PowerShell exited with {result}");
        anyhow::ensure!(
            shortcut.is_file(),
            "Windows did not create the startup shortcut"
        );
        Ok(true)
    }
    #[cfg(not(windows))]
    {
        let _ = (enabled, headless);
        Ok(false)
    }
}

pub fn shortcut_exists() -> bool {
    #[cfg(windows)]
    {
        shortcut_path().is_ok_and(|path| path.is_file())
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
fn shortcut_path() -> anyhow::Result<std::path::PathBuf> {
    let appdata =
        std::env::var_os("APPDATA").ok_or_else(|| anyhow::anyhow!("APPDATA is not set"))?;
    Ok(std::path::PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup")
        .join(SHORTCUT_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(windows))]
    fn non_windows_is_an_explicit_no_op() {
        assert!(!supported());
        assert_eq!(set_enabled(true, false).unwrap(), false);
        assert!(!shortcut_exists());

        let outcome = reconcile(true, false);
        assert!(!outcome.succeeded);
        let mut status = crate::protocol::RuntimeStatus::default();
        outcome.publish(&mut status);
        assert!(!status.startup_supported);
        assert!(!status.startup_enabled);
        assert!(status.startup_state.contains("only supported on Windows"));
    }

    #[test]
    fn shortcut_name_is_stable_across_binary_versions() {
        assert_eq!(SHORTCUT_NAME, "Empyrean Gate.lnk");
    }
}
