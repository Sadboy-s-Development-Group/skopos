//! The `skopos` command-line surface — every `clap` type in one place.
//!
//! `claude` and `gemini` share [`AgentCommand`] because their subcommand
//! sets are identical; `codex` keeps its own [`CodexCommand`] only
//! because it carries the extra `usage` / `refresh` commands.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::providers::ProviderId;

#[derive(Debug, Parser)]
#[command(name = "skopos")]
#[command(about = "Skopos CLI control plane")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Show local Skopos status.
    Status,
    /// Print the planned local data paths.
    Doctor,
    /// List AI providers tracked in the local store. REPL alias: `providers`.
    Providers {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Inspect persisted AI usage. With no subcommand, shows rate-limit
    /// progress bars per provider (from the statusline snapshot).
    Usage {
        #[command(subcommand)]
        command: Option<UsageCommand>,
    },
    /// Inspect or import Claude Code local usage logs.
    Claude {
        #[command(subcommand)]
        command: AgentCommand,
    },
    /// Inspect Codex (ChatGPT) usage limits.
    Codex {
        #[command(subcommand)]
        command: CodexCommand,
    },
    /// Inspect or import Gemini CLI local usage logs.
    Gemini {
        #[command(subcommand)]
        command: AgentCommand,
    },
    /// Pick a project and hand the terminal over to an agentic CLI.
    Work {
        /// Provider to launch. Defaults to the one in ~/.config/skopos/config.toml.
        #[arg(long)]
        provider: Option<ProviderId>,
        /// Project root to list. Defaults to the one in ~/.config/skopos/config.toml.
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Track internet connectivity. With no subcommand, opens the live
    /// network-health dashboard.
    Network {
        #[command(subcommand)]
        command: Option<NetworkCommand>,
    },
    /// Read the statusline JSON Claude Code pipes on stdin, persist the
    /// rate-limit snapshot, and print a compact one-line view. Registered
    /// by `skopos usage install`; not meant to be invoked by hand.
    #[command(hide = true)]
    Statusline,
}

#[derive(Debug, Subcommand)]
pub(crate) enum NetworkCommand {
    /// Run the connectivity probe loop in the foreground. This is what the
    /// systemd unit runs; not normally invoked by hand.
    Watch {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Print a one-shot network-health verdict and exit (0 stable, 1
    /// moderate, 2 severe). Handy for MOTD banners and scripts.
    Status {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Generate the systemd unit for the probe daemon.
    Install {
        /// Write a no-root per-user unit under ~/.config/systemd/user
        /// instead of staging a system unit.
        #[arg(long)]
        user: bool,
    },
    /// Remove the systemd unit for the probe daemon.
    Uninstall {
        /// Target the per-user unit rather than the system unit.
        #[arg(long)]
        user: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum UsageCommand {
    /// Show all-time usage grouped by provider/model.
    ByModel {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show usage for today in UTC.
    Today {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show usage for the current month in UTC.
    Month {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Register the Skopos statusline hook in ~/.claude/settings.json so
    /// rate-limit % data is captured while Claude Code runs.
    Install {
        /// Replace an existing statusLine hook if one is already set.
        #[arg(long)]
        force: bool,
    },
    /// Remove the Skopos statusline hook from ~/.claude/settings.json.
    Uninstall {
        /// Remove whatever statusLine is configured, even if it isn't ours.
        #[arg(long)]
        force: bool,
    },
}

/// Subcommands shared by `skopos claude` and `skopos gemini`.
#[derive(Debug, Subcommand)]
pub(crate) enum AgentCommand {
    /// Scan the agent's local transcripts and summarize token usage
    /// without writing SQLite.
    Scan {
        /// Agent home directory. Defaults to the agent's standard path.
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Import the agent's local transcripts into Skopos SQLite.
    Import {
        /// Agent home directory. Defaults to the agent's standard path.
        #[arg(long)]
        path: Option<PathBuf>,
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show usage for today (UTC). REPL alias: `-t`.
    Today {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show usage for the current week (UTC). REPL alias: `-w`.
    Week {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show usage for the current month (UTC). REPL alias: `-m`.
    Month {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show usage grouped by model. REPL alias: `models`.
    Models {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum CodexCommand {
    /// Show Codex 5h / weekly limit bars from the cached snapshot
    /// (refreshed on demand by `skopos codex refresh` or `skopos usage`).
    Usage,
    /// Fetch a fresh snapshot from the local Codex app-server and persist it.
    Refresh,
    /// Scan Codex rollout JSONLs and summarize token usage without writing SQLite.
    Scan {
        /// Codex home directory. Defaults to ~/.codex.
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Import Codex rollout JSONLs into Skopos SQLite.
    Import {
        /// Codex home directory. Defaults to ~/.codex.
        #[arg(long)]
        path: Option<PathBuf>,
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show Codex usage for today (UTC). REPL alias: `codex -t`.
    Today {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show Codex usage for the current week (UTC). REPL alias: `codex -w`.
    Week {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show Codex usage for the current month (UTC). REPL alias: `codex -m`.
    Month {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show Codex usage grouped by model. REPL alias: `codex models`.
    Models {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
}
