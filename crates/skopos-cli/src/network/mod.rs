//! `skopos network` — internet connectivity tracker.
//!
//! A background daemon (`skopos network watch`, normally run by systemd)
//! pings a set of websites on a fixed interval and records every probe
//! sample plus every outage into the Skopos SQLite store. `skopos network`
//! with no subcommand opens a live TUI dashboard over that history,
//! classifying the link as stable / moderate / severe; `skopos network
//! status` prints the same verdict once and exits.
//!
//! This split — a persistent recorder plus a stateless reader — is what
//! lets the tool count interruptions on an always-on server whether or
//! not anyone has the dashboard open.

mod health;
mod install;
mod probe;
mod tui;
mod watch;

use std::path::PathBuf;

use crate::config::Config;

/// `skopos network` — open the live dashboard.
pub(crate) async fn run_dashboard(cfg: &Config, db: PathBuf) -> anyhow::Result<()> {
    tui::run(cfg, db).await
}

/// `skopos network watch` — run the probe daemon in the foreground.
pub(crate) async fn run_watch(cfg: &Config, db: PathBuf) -> anyhow::Result<()> {
    watch::run(cfg, db).await
}

/// `skopos network status` — print a one-shot verdict; returns the process
/// exit code (0 stable, 1 moderate, 2 severe).
pub(crate) async fn run_status(cfg: &Config, db: PathBuf) -> anyhow::Result<i32> {
    tui::run_status(cfg, db).await
}

/// `skopos network install` — write the systemd unit for the daemon.
pub(crate) fn run_install(user: bool) -> anyhow::Result<String> {
    install::run_install(user)
}

/// `skopos network uninstall` — remove the systemd unit.
pub(crate) fn run_uninstall(user: bool) -> anyhow::Result<String> {
    install::run_uninstall(user)
}

// ===========================================================================
// Shared rendering helpers (visible to the child modules)
// ===========================================================================

/// Truecolor foreground text.
fn rgb(text: &str, (r, g, b): (u8, u8, u8)) -> String {
    format!("\x1b[38;2;{r};{g};{b}m{text}\x1b[0m")
}

/// Bold truecolor foreground text.
fn rgb_bold(text: &str, (r, g, b): (u8, u8, u8)) -> String {
    format!("\x1b[1m\x1b[38;2;{r};{g};{b}m{text}\x1b[0m")
}

/// Best-effort hostname of this machine.
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            std::env::var("HOSTNAME")
                .ok()
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_else(|| "host".to_string())
}
