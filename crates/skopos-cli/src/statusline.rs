//! The Claude Code statusline hook lifecycle.
//!
//! `skopos usage install` registers `skopos statusline` as a `statusLine`
//! command in `~/.claude/settings.json`; Claude Code then pipes a JSON
//! payload to it on every render, which `run_statusline` persists as the
//! session snapshot the usage reports read back.

use chrono::Utc;

use crate::install::{self, InstallOutcome};
use crate::limits;
use crate::theme::{dim, purple, purple_bold};

/// `skopos statusline`: receive Claude Code's JSON on stdin, persist the
/// snapshot, and print a single line back so Claude Code has something to
/// show above the prompt.
pub(crate) fn run_statusline() -> anyhow::Result<()> {
    let payload = limits::read_stdin_to_string(std::io::stdin())?;
    // Always keep a copy of the last raw payload — useful when the schema
    // drifts between Claude Code versions and parsing yields empty windows.
    let _ = limits::save_last_payload(&payload);
    let (plan, tier) = limits::read_plan_labels(&limits::claude_credentials_path());
    let snapshot = limits::snapshot_from_statusline_json(&payload, plan, tier, Utc::now())?;
    limits::save_snapshot(&limits::snapshot_path(), &snapshot)?;
    // Stdout becomes Claude Code's statusline. Newline-free per spec.
    print!("{}", limits::render_statusline_line(&snapshot));
    Ok(())
}

/// `skopos usage install`: register the statusline hook, with backup.
pub(crate) fn run_install(force: bool) -> anyhow::Result<String> {
    let settings = install::default_settings_path();
    let binary = install::skopos_binary_path();
    let outcome = install::install(&settings, &binary, force)?;
    let mut out = String::new();
    out.push_str(&purple_bold("Install statusline hook"));
    out.push_str("\n\n");
    out.push_str(&dim(&format!("  settings: {}\n", settings.display())));
    out.push_str(&dim(&format!("  binary:   {}\n", binary.display())));
    out.push('\n');
    match outcome {
        InstallOutcome::Installed { backup_path } => {
            out.push_str(&purple("  installed.\n"));
            if let Some(path) = backup_path {
                out.push_str(&dim(&format!("  backup:   {}\n", path.display())));
            }
            out.push_str(&dim(
                "  open Claude Code once to capture the first snapshot.\n",
            ));
        }
        InstallOutcome::AlreadyInstalled => {
            out.push_str(&purple("  already installed.\n"));
        }
        InstallOutcome::ReplacedExisting {
            previous,
            backup_path,
        } => {
            out.push_str(&purple("  replaced an existing statusLine.\n"));
            out.push_str(&dim(&format!("  previous: {previous}\n")));
            out.push_str(&dim(&format!("  backup:   {}\n", backup_path.display())));
        }
        InstallOutcome::RefusedToReplace { existing } => {
            out.push_str(&purple(
                "  another statusLine is already configured — refusing to replace.\n",
            ));
            out.push_str(&dim(&format!("  existing: {existing}\n")));
            out.push_str(&dim(
                "  re-run with --force to replace it. A backup of settings.json is made first.\n",
            ));
        }
        InstallOutcome::Uninstalled { .. } | InstallOutcome::NotInstalled => {
            unreachable!("install() never returns uninstall outcomes");
        }
    }
    Ok(out)
}

/// `skopos usage uninstall`: remove the hook, preserving a backup.
pub(crate) fn run_uninstall(force: bool) -> anyhow::Result<String> {
    let settings = install::default_settings_path();
    let binary = install::skopos_binary_path();
    let outcome = install::uninstall(&settings, &binary, force)?;
    let mut out = String::new();
    out.push_str(&purple_bold("Uninstall statusline hook"));
    out.push_str("\n\n");
    out.push_str(&dim(&format!("  settings: {}\n", settings.display())));
    out.push('\n');
    match outcome {
        InstallOutcome::Uninstalled { backup_path } => {
            out.push_str(&purple("  removed.\n"));
            if let Some(path) = backup_path {
                out.push_str(&dim(&format!("  backup:   {}\n", path.display())));
            }
        }
        InstallOutcome::NotInstalled => {
            out.push_str(&dim(
                "  nothing to do — no Skopos statusLine was configured. Re-run with --force to remove any other hook.\n",
            ));
        }
        _ => unreachable!("uninstall() never returns install outcomes"),
    }
    Ok(out)
}
