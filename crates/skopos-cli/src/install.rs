//! Install / uninstall the Skopos statusline hook in `~/.claude/settings.json`.
//!
//! Claude Code lets a single `statusLine` command receive the per-request
//! `rate_limits` JSON on stdin. Skopos uses that as its source of truth for
//! `/usage` percentages. This module:
//! - reads the existing `settings.json` (or starts an empty object),
//! - backs it up next to itself,
//! - writes the new `statusLine` block pointing at the running `skopos` binary,
//! - and undoes the change on `uninstall`.
//!
//! Only the `statusLine` key is touched — every other field round-trips
//! through `serde_json::Value` so the user's settings are not reformatted
//! beyond the one edit.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{json, Value};

use crate::limits::{claude_settings_path, home_dir};

/// Result of a single install/uninstall operation, used to print a clear
/// summary to the user.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InstallOutcome {
    Installed {
        backup_path: Option<PathBuf>,
    },
    AlreadyInstalled,
    ReplacedExisting {
        previous: String,
        backup_path: PathBuf,
    },
    /// A different `statusLine` is already configured and the caller did
    /// not pass `--force`. No changes were written.
    RefusedToReplace {
        existing: String,
    },
    Uninstalled {
        backup_path: Option<PathBuf>,
    },
    NotInstalled,
}

/// Add (or replace) the `statusLine` entry pointing at `skopos_binary`.
/// `existing_hook_action` controls what to do when the user already has
/// a different statusLine configured.
pub(crate) fn install(
    settings_path: &Path,
    skopos_binary: &Path,
    replace_existing: bool,
) -> anyhow::Result<InstallOutcome> {
    let mut root = read_settings(settings_path)?;
    let desired_command = format!("{} statusline", skopos_binary.display());

    let already_ours = root
        .get("statusLine")
        .and_then(|v| v.get("command"))
        .and_then(|v| v.as_str())
        .map(|cmd| cmd == desired_command)
        .unwrap_or(false);
    if already_ours {
        return Ok(InstallOutcome::AlreadyInstalled);
    }

    let previous_block = root.get("statusLine").cloned();
    if let Some(prev) = &previous_block {
        if !is_same_command(prev, &desired_command) && !replace_existing {
            return Ok(InstallOutcome::RefusedToReplace {
                existing: prev.to_string(),
            });
        }
    }
    let backup = backup_settings(settings_path)?;

    root["statusLine"] = json!({
        "type": "command",
        "command": desired_command,
    });
    write_settings(settings_path, &root)?;

    match previous_block {
        Some(prev) if !is_same_command(&prev, &desired_command) => {
            Ok(InstallOutcome::ReplacedExisting {
                previous: prev.to_string(),
                backup_path: backup.expect("backup made when file existed"),
            })
        }
        _ => Ok(InstallOutcome::Installed {
            backup_path: backup,
        }),
    }
}

/// Remove a `statusLine` entry that points at `skopos statusline`. Leaves
/// any other `statusLine` alone unless `force` is set.
pub(crate) fn uninstall(
    settings_path: &Path,
    skopos_binary: &Path,
    force: bool,
) -> anyhow::Result<InstallOutcome> {
    if !settings_path.exists() {
        return Ok(InstallOutcome::NotInstalled);
    }
    let mut root = read_settings(settings_path)?;
    let desired_command = format!("{} statusline", skopos_binary.display());

    let is_ours = root
        .get("statusLine")
        .map(|block| is_same_command(block, &desired_command))
        .unwrap_or(false);
    if !is_ours && !force {
        return Ok(InstallOutcome::NotInstalled);
    }
    let backup = backup_settings(settings_path)?;
    if let Value::Object(map) = &mut root {
        map.remove("statusLine");
    }
    write_settings(settings_path, &root)?;
    Ok(InstallOutcome::Uninstalled {
        backup_path: backup,
    })
}

/// Read `~/.claude/settings.json` as a JSON object, or fall back to `{}`
/// if the file is missing. A malformed file is reported, not overwritten.
fn read_settings(path: &Path) -> anyhow::Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let raw = fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(json!({}));
    }
    let value: Value = serde_json::from_str(&raw)
        .map_err(|err| anyhow::anyhow!("{} is not valid JSON: {err}", path.display()))?;
    if !value.is_object() {
        anyhow::bail!("{} must contain a JSON object", path.display());
    }
    Ok(value)
}

fn write_settings(path: &Path, value: &Value) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(value)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, body + "\n")?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Copy `settings.json` to `settings.json.skopos.bak-<UTC>` so the user can
/// always step back. Returns the backup path, or `None` when the source
/// did not exist.
fn backup_settings(path: &Path) -> anyhow::Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let backup = path.with_file_name(format!(
        "{}.skopos.bak-{stamp}",
        path.file_name().unwrap_or_default().to_string_lossy(),
    ));
    fs::copy(path, &backup)?;
    Ok(Some(backup))
}

fn is_same_command(block: &Value, command: &str) -> bool {
    block
        .get("command")
        .and_then(|v| v.as_str())
        .map(|c| c == command)
        .unwrap_or(false)
}

/// Best-effort absolute path to the currently-running `skopos` binary,
/// used as the `statusLine` command. Falls back to `~/.cargo/bin/skopos`
/// (a common cargo-install location) and then to the bare name.
pub(crate) fn skopos_binary_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Ok(canon) = fs::canonicalize(&exe) {
            return canon;
        }
        return exe;
    }
    let fallback = home_dir().join(".cargo").join("bin").join("skopos");
    if fallback.exists() {
        return fallback;
    }
    PathBuf::from("skopos")
}

/// Convenience: the production settings path.
pub(crate) fn default_settings_path() -> PathBuf {
    claude_settings_path()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("skopos-install-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn install_into_empty_settings_writes_status_line() {
        let dir = temp_dir("empty");
        let settings = dir.join("settings.json");
        let binary = dir.join("bin/skopos");
        let outcome = install(&settings, &binary, false).unwrap();
        assert_eq!(outcome, InstallOutcome::Installed { backup_path: None });
        let written: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        let expected = format!("{} statusline", binary.display());
        assert_eq!(written["statusLine"]["command"], Value::String(expected));
        assert_eq!(
            written["statusLine"]["type"],
            Value::String("command".into())
        );
    }

    #[test]
    fn install_preserves_other_keys() {
        let dir = temp_dir("preserve");
        let settings = dir.join("settings.json");
        fs::write(&settings, r#"{"theme":"dark","enabledPlugins":{"a":true}}"#).unwrap();
        let binary = dir.join("bin/skopos");
        install(&settings, &binary, false).unwrap();
        let written: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(written["theme"], Value::String("dark".into()));
        assert_eq!(written["enabledPlugins"]["a"], Value::Bool(true));
        assert!(written["statusLine"].is_object());
    }

    #[test]
    fn install_is_idempotent_when_command_matches() {
        let dir = temp_dir("idempotent");
        let settings = dir.join("settings.json");
        let binary = dir.join("bin/skopos");
        install(&settings, &binary, false).unwrap();
        let second = install(&settings, &binary, false).unwrap();
        assert_eq!(second, InstallOutcome::AlreadyInstalled);
    }

    #[test]
    fn install_refuses_to_replace_foreign_hook_without_force() {
        let dir = temp_dir("refuse");
        let settings = dir.join("settings.json");
        let original = r#"{"statusLine":{"type":"command","command":"my-prompt"}}"#;
        fs::write(&settings, original).unwrap();
        let binary = dir.join("bin/skopos");
        let outcome = install(&settings, &binary, false).unwrap();
        match outcome {
            InstallOutcome::RefusedToReplace { existing } => {
                assert!(existing.contains("my-prompt"));
            }
            other => panic!("expected RefusedToReplace, got {other:?}"),
        }
        // File untouched.
        assert_eq!(fs::read_to_string(&settings).unwrap(), original);
    }

    #[test]
    fn install_reports_when_an_existing_hook_is_replaced() {
        let dir = temp_dir("replace");
        let settings = dir.join("settings.json");
        fs::write(
            &settings,
            r#"{"statusLine":{"type":"command","command":"my-prompt"}}"#,
        )
        .unwrap();
        let binary = dir.join("bin/skopos");
        let outcome = install(&settings, &binary, true).unwrap();
        match outcome {
            InstallOutcome::ReplacedExisting {
                previous,
                backup_path,
            } => {
                assert!(previous.contains("my-prompt"));
                assert!(backup_path.exists());
                let backed_up = fs::read_to_string(&backup_path).unwrap();
                assert!(backed_up.contains("my-prompt"));
            }
            other => panic!("expected ReplacedExisting, got {other:?}"),
        }
    }

    #[test]
    fn uninstall_removes_only_when_command_matches() {
        let dir = temp_dir("uninstall-ours");
        let settings = dir.join("settings.json");
        let binary = dir.join("bin/skopos");
        install(&settings, &binary, false).unwrap();
        let outcome = uninstall(&settings, &binary, false).unwrap();
        match outcome {
            InstallOutcome::Uninstalled { backup_path } => {
                assert!(backup_path.is_some());
            }
            other => panic!("expected Uninstalled, got {other:?}"),
        }
        let written: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert!(written.get("statusLine").is_none());
    }

    #[test]
    fn uninstall_keeps_unknown_hook_unless_forced() {
        let dir = temp_dir("uninstall-foreign");
        let settings = dir.join("settings.json");
        fs::write(
            &settings,
            r#"{"statusLine":{"type":"command","command":"someone-else"}}"#,
        )
        .unwrap();
        let binary = dir.join("bin/skopos");
        assert_eq!(
            uninstall(&settings, &binary, false).unwrap(),
            InstallOutcome::NotInstalled
        );
        let still_there: Value =
            serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(
            still_there["statusLine"]["command"],
            Value::String("someone-else".into())
        );
    }
}
