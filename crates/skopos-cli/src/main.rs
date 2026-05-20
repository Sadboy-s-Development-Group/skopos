use chrono::{DateTime, Datelike, TimeZone, Utc};
use clap::{Parser, Subcommand};
use providers::ProviderId;
use skopos_collectors::claude_code::{
    discover_claude_code_jsonl_paths, parse_usage_events_from_jsonl_path,
};
use skopos_collectors::codex::{
    discover_codex_rollout_paths, parse_usage_events_from_rollout_path,
};
use skopos_collectors::gemini::{
    discover_gemini_session_paths, parse_usage_events_from_session_path,
};
use skopos_core::UsageEvent;
use skopos_pricing::{default_overrides_path, Catalog};
use skopos_store::SkoposStore;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

mod codex_limits;
mod config;
mod icons;
mod install;
mod limits;
mod local_usage;
mod network;
mod providers;
mod repl;
mod work;

#[derive(Debug, Parser)]
#[command(name = "skopos")]
#[command(about = "Skopos CLI control plane")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
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
        command: ClaudeCommand,
    },
    /// Inspect Codex (ChatGPT) usage limits.
    Codex {
        #[command(subcommand)]
        command: CodexCommand,
    },
    /// Inspect or import Gemini CLI local usage logs.
    Gemini {
        #[command(subcommand)]
        command: GeminiCommand,
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
enum NetworkCommand {
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
enum UsageCommand {
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

#[derive(Debug, Subcommand)]
enum ClaudeCommand {
    /// Scan Claude Code JSONL transcripts and summarize token usage without writing SQLite.
    Scan {
        /// Claude home directory. Defaults to ~/.claude.
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Import Claude Code JSONL transcripts into Skopos SQLite.
    Import {
        /// Claude home directory. Defaults to ~/.claude.
        #[arg(long)]
        path: Option<PathBuf>,
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show Claude usage for today (UTC). REPL alias: `claude -t`.
    Today {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show Claude usage for the current week (UTC). REPL alias: `claude -w`.
    Week {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show Claude usage for the current month (UTC). REPL alias: `claude -m`.
    Month {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show Claude usage grouped by model. REPL alias: `claude models`.
    Models {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum CodexCommand {
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

#[derive(Debug, Subcommand)]
enum GeminiCommand {
    /// Scan Gemini session JSONLs and summarize token usage without writing SQLite.
    Scan {
        /// Gemini home directory. Defaults to ~/.gemini.
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Import Gemini session JSONLs into Skopos SQLite.
    Import {
        /// Gemini home directory. Defaults to ~/.gemini.
        #[arg(long)]
        path: Option<PathBuf>,
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show Gemini usage for today (UTC). REPL alias: `gemini -t`.
    Today {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show Gemini usage for the current week (UTC). REPL alias: `gemini -w`.
    Week {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show Gemini usage for the current month (UTC). REPL alias: `gemini -m`.
    Month {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Show Gemini usage grouped by model. REPL alias: `gemini models`.
    Models {
        /// SQLite database path. Defaults to ~/.local/share/skopos/skopos.db.
        #[arg(long)]
        db: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => repl::run(&default_db_path()).await?,
        Some(Command::Status) => println!("Skopos status: bootstrapped"),
        Some(Command::Doctor) => {
            println!("Skopos doctor");
            println!("config: ~/.config/skopos/config.toml");
            println!("data:   {}", default_db_path().display());
            println!("logs:   ~/.local/state/skopos/skopos.log");
        }
        Some(Command::Providers { db }) => {
            let db_path = db.unwrap_or_else(default_db_path);
            auto_import_codex_if_stale(&db_path).await;
            auto_import_gemini_if_stale(&db_path).await;
            print!("{}", providers_report(&db_path).await?);
        }
        Some(Command::Usage { command }) => match command {
            None => print!("{}", usage_limits_report().await?),
            Some(UsageCommand::ByModel { db }) => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_codex_if_stale(&db_path).await;
                auto_import_gemini_if_stale(&db_path).await;
                print!("{}", usage_by_model_report(&db_path).await?);
            }
            Some(UsageCommand::Today { db }) => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_codex_if_stale(&db_path).await;
                auto_import_gemini_if_stale(&db_path).await;
                print!(
                    "{}",
                    usage_period_report(&db_path, UsagePeriod::Today).await?
                );
            }
            Some(UsageCommand::Month { db }) => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_codex_if_stale(&db_path).await;
                auto_import_gemini_if_stale(&db_path).await;
                print!(
                    "{}",
                    usage_period_report(&db_path, UsagePeriod::Month).await?
                );
            }
            Some(UsageCommand::Install { force }) => {
                print!("{}", run_install(force)?);
            }
            Some(UsageCommand::Uninstall { force }) => {
                print!("{}", run_uninstall(force)?);
            }
        },
        Some(Command::Claude { command }) => match command {
            ClaudeCommand::Scan { path } => scan_claude(path)?,
            ClaudeCommand::Import { path, db } => {
                let claude_home = path.unwrap_or_else(default_claude_home);
                let db_path = db.unwrap_or_else(default_db_path);
                let report = import_claude_from_home(&claude_home, &db_path).await?;
                println!("Claude Code import");
                println!("home:       {}", claude_home.display());
                println!("db:         {}", db_path.display());
                println!("files:      {}", report.files);
                println!("seen:       {}", report.seen_events);
                println!("inserted:   {}", report.inserted_events);
                println!("duplicates: {}", report.duplicate_events);
            }
            ClaudeCommand::Today { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Today, Some("anthropic"))
                        .await?
                );
            }
            ClaudeCommand::Week { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Week, Some("anthropic"))
                        .await?
                );
            }
            ClaudeCommand::Month { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Month, Some("anthropic"))
                        .await?
                );
            }
            ClaudeCommand::Models { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                print!(
                    "{}",
                    usage_by_model_report_filtered(&db_path, Some("anthropic")).await?
                );
            }
        },
        Some(Command::Codex { command }) => match command {
            CodexCommand::Usage => print!("{}", codex_usage_report()?),
            CodexCommand::Refresh => print!("{}", codex_refresh_report().await?),
            CodexCommand::Scan { path } => scan_codex(path)?,
            CodexCommand::Import { path, db } => {
                let codex_home = path.unwrap_or_else(default_codex_home);
                let db_path = db.unwrap_or_else(default_db_path);
                let report = import_codex_from_home(&codex_home, &db_path).await?;
                println!("Codex import");
                println!("home:       {}", codex_home.display());
                println!("db:         {}", db_path.display());
                println!("files:      {}", report.files);
                println!("seen:       {}", report.seen_events);
                println!("inserted:   {}", report.inserted_events);
                println!("duplicates: {}", report.duplicate_events);
            }
            CodexCommand::Today { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_codex_if_stale(&db_path).await;
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Today, Some("openai"))
                        .await?
                );
            }
            CodexCommand::Week { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_codex_if_stale(&db_path).await;
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Week, Some("openai"))
                        .await?
                );
            }
            CodexCommand::Month { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_codex_if_stale(&db_path).await;
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Month, Some("openai"))
                        .await?
                );
            }
            CodexCommand::Models { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_codex_if_stale(&db_path).await;
                print!(
                    "{}",
                    usage_by_model_report_filtered(&db_path, Some("openai")).await?
                );
            }
        },
        Some(Command::Gemini { command }) => match command {
            GeminiCommand::Scan { path } => scan_gemini(path)?,
            GeminiCommand::Import { path, db } => {
                let gemini_home = path.unwrap_or_else(default_gemini_home);
                let db_path = db.unwrap_or_else(default_db_path);
                let report = import_gemini_from_home(&gemini_home, &db_path).await?;
                println!("Gemini import");
                println!("home:       {}", gemini_home.display());
                println!("db:         {}", db_path.display());
                println!("files:      {}", report.files);
                println!("seen:       {}", report.seen_events);
                println!("inserted:   {}", report.inserted_events);
                println!("duplicates: {}", report.duplicate_events);
            }
            GeminiCommand::Today { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_gemini_if_stale(&db_path).await;
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Today, Some("google"))
                        .await?
                );
            }
            GeminiCommand::Week { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_gemini_if_stale(&db_path).await;
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Week, Some("google"))
                        .await?
                );
            }
            GeminiCommand::Month { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_gemini_if_stale(&db_path).await;
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Month, Some("google"))
                        .await?
                );
            }
            GeminiCommand::Models { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_gemini_if_stale(&db_path).await;
                print!(
                    "{}",
                    usage_by_model_report_filtered(&db_path, Some("google")).await?
                );
            }
        },
        Some(Command::Work { provider, root }) => {
            let cfg = config::load()?;
            work::run(&cfg, provider, root)?;
        }
        Some(Command::Network { command }) => {
            let cfg = config::load()?;
            match command {
                None => network::run_dashboard(&cfg, default_db_path()).await?,
                Some(NetworkCommand::Watch { db }) => {
                    network::run_watch(&cfg, db.unwrap_or_else(default_db_path)).await?;
                }
                Some(NetworkCommand::Status { db }) => {
                    let code =
                        network::run_status(&cfg, db.unwrap_or_else(default_db_path)).await?;
                    std::process::exit(code);
                }
                Some(NetworkCommand::Install { user }) => {
                    print!("{}", network::run_install(user)?);
                }
                Some(NetworkCommand::Uninstall { user }) => {
                    print!("{}", network::run_uninstall(user)?);
                }
            }
        }
        Some(Command::Statusline) => run_statusline()?,
    }

    Ok(())
}

// ===========================================================================
// Usage / statusline subcommand handlers
// ===========================================================================

/// `skopos usage` (no subcommand): two blocks per host —
/// 1. **Current Session** from the statusline snapshot (live Claude Code state).
/// 2. **Local Activity** from `~/.claude/projects/**/*.jsonl` (last 5h / 7d
///    absolute token counts).
///
/// Anthropic does not expose the per-account 5h/7d quota % to third-party
/// tools, and reading their OAuth-only endpoint would violate the Consumer
/// Terms — so we deliberately do not show a % bar for the windowed totals.
pub(crate) async fn usage_limits_report() -> anyhow::Result<String> {
    let snapshot = limits::load_snapshot(&limits::snapshot_path())?;
    let now = Utc::now();
    let local = local_usage::aggregate(&limits::claude_home(), now)?;

    // Best-effort Codex hop: 4s budget for the whole roundtrip, then
    // fall back silently to the cached snapshot. `received_at` will
    // tell the user how stale we are.
    let codex_path = codex_limits::codex_snapshot_path();
    let codex_snapshot = match tokio::time::timeout(
        std::time::Duration::from_secs(4),
        codex_limits::fetch_codex_snapshot(),
    )
    .await
    {
        Ok(Ok(fresh)) => {
            let _ = codex_limits::save_codex_snapshot(&codex_path, &fresh);
            Some(fresh)
        }
        _ => codex_limits::load_codex_snapshot(&codex_path)
            .ok()
            .flatten(),
    };

    let mut out = String::new();
    out.push_str(&purple_bold("Usage"));
    out.push_str("\n\n");
    out.push_str(&limits::render_limits_block(snapshot.as_ref(), now));
    out.push('\n');
    out.push_str(&codex_limits::render_codex_limits_block(
        codex_snapshot.as_ref(),
        now,
    ));
    out.push('\n');
    out.push_str(&limits::render_session_block(snapshot.as_ref(), now));
    out.push('\n');
    out.push_str(&local_usage::render_local_block(&local));
    Ok(out)
}

/// `skopos codex usage`: render the cached Codex snapshot.
fn codex_usage_report() -> anyhow::Result<String> {
    let snapshot = codex_limits::load_codex_snapshot(&codex_limits::codex_snapshot_path())
        .ok()
        .flatten();
    let now = Utc::now();
    let mut out = String::new();
    out.push_str(&purple_bold("Codex usage"));
    out.push_str("\n\n");
    out.push_str(&codex_limits::render_codex_limits_block(
        snapshot.as_ref(),
        now,
    ));
    Ok(out)
}

/// `skopos codex refresh`: drive the JSON-RPC handshake and persist.
async fn codex_refresh_report() -> anyhow::Result<String> {
    let path = codex_limits::codex_snapshot_path();
    match codex_limits::fetch_codex_snapshot().await {
        Ok(snap) => {
            codex_limits::save_codex_snapshot(&path, &snap)?;
            let plan = codex_limits::plan_label(snap.plan_type.as_deref());
            let p = snap
                .primary
                .as_ref()
                .map(|w| format!("{:.0}%", w.used_percent))
                .unwrap_or_else(|| "—".to_string());
            let s = snap
                .secondary
                .as_ref()
                .map(|w| format!("{:.0}%", w.used_percent))
                .unwrap_or_else(|| "—".to_string());
            let mut out = String::new();
            out.push_str(&purple_bold("Codex refresh"));
            out.push_str("\n\n");
            out.push_str(&format!(
                "  {} {} — plan: {}, 5h {} / weekly {}\n",
                purple("snapshot saved"),
                dim(&format!("→ {}", path.display())),
                plan,
                p,
                s,
            ));
            Ok(out)
        }
        Err(err) => Err(anyhow::anyhow!(
            "failed to fetch: {err} — is `codex` installed and authenticated? Run `codex login`."
        )),
    }
}

/// `skopos statusline`: receive Claude Code's JSON on stdin, persist the
/// snapshot, and print a single line back so Claude Code has something to
/// show above the prompt.
fn run_statusline() -> anyhow::Result<()> {
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
fn run_install(force: bool) -> anyhow::Result<String> {
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
        install::InstallOutcome::Installed { backup_path } => {
            out.push_str(&purple("  installed.\n"));
            if let Some(path) = backup_path {
                out.push_str(&dim(&format!("  backup:   {}\n", path.display())));
            }
            out.push_str(&dim(
                "  open Claude Code once to capture the first snapshot.\n",
            ));
        }
        install::InstallOutcome::AlreadyInstalled => {
            out.push_str(&purple("  already installed.\n"));
        }
        install::InstallOutcome::ReplacedExisting {
            previous,
            backup_path,
        } => {
            out.push_str(&purple("  replaced an existing statusLine.\n"));
            out.push_str(&dim(&format!("  previous: {previous}\n")));
            out.push_str(&dim(&format!("  backup:   {}\n", backup_path.display())));
        }
        install::InstallOutcome::RefusedToReplace { existing } => {
            out.push_str(&purple(
                "  another statusLine is already configured — refusing to replace.\n",
            ));
            out.push_str(&dim(&format!("  existing: {existing}\n")));
            out.push_str(&dim(
                "  re-run with --force to replace it. A backup of settings.json is made first.\n",
            ));
        }
        install::InstallOutcome::Uninstalled { .. } | install::InstallOutcome::NotInstalled => {
            unreachable!("install() never returns uninstall outcomes");
        }
    }
    Ok(out)
}

/// `skopos usage uninstall`: remove the hook, preserving a backup.
fn run_uninstall(force: bool) -> anyhow::Result<String> {
    let settings = install::default_settings_path();
    let binary = install::skopos_binary_path();
    let outcome = install::uninstall(&settings, &binary, force)?;
    let mut out = String::new();
    out.push_str(&purple_bold("Uninstall statusline hook"));
    out.push_str("\n\n");
    out.push_str(&dim(&format!("  settings: {}\n", settings.display())));
    out.push('\n');
    match outcome {
        install::InstallOutcome::Uninstalled { backup_path } => {
            out.push_str(&purple("  removed.\n"));
            if let Some(path) = backup_path {
                out.push_str(&dim(&format!("  backup:   {}\n", path.display())));
            }
        }
        install::InstallOutcome::NotInstalled => {
            out.push_str(&dim(
                "  nothing to do — no Skopos statusLine was configured. Re-run with --force to remove any other hook.\n",
            ));
        }
        _ => unreachable!("uninstall() never returns install outcomes"),
    }
    Ok(out)
}

const SKOPOS_ASCII: &str = include_str!("../assets/skopos-ascii.txt");

/// Bright purple used for side-panel text, table headers and labels.
const PURPLE: (u8, u8, u8) = (189, 147, 249);

/// Horizontal gap between the ASCII art and the side panel.
const SPLASH_GAP: usize = 4;

/// Render the splash for the given terminal `width`, picking a responsive
/// layout. The ASCII art is never scaled or truncated (curated asset); the
/// layout adapts instead:
/// - wide: art and panel side by side,
/// - medium: art stacked above the panel,
/// - narrow: panel only.
pub(crate) fn welcome_screen(width: usize) -> String {
    let art_lines: Vec<&str> = SKOPOS_ASCII.trim_end_matches('\n').lines().collect();
    let info_lines = panel_info_lines();
    let art_width = art_lines
        .iter()
        .map(|line| visible_width(line))
        .max()
        .unwrap_or(0);
    let panel_width = info_lines
        .iter()
        .map(InfoLine::visible_len)
        .max()
        .unwrap_or(0);

    if width >= art_width + SPLASH_GAP + panel_width {
        render_splash_side_by_side(&art_lines, &info_lines, art_width)
    } else if width >= art_width {
        render_splash_stacked(&art_lines, &info_lines)
    } else {
        render_splash_compact(&info_lines)
    }
}

fn render_splash_side_by_side(
    art_lines: &[&str],
    info_lines: &[InfoLine],
    art_width: usize,
) -> String {
    let info_start = art_lines.len().saturating_sub(info_lines.len()) / 2;
    let mut output = String::new();
    for (idx, art_line) in art_lines.iter().enumerate() {
        output.push_str(&purple_gradient_line(art_line, idx, art_lines.len()));
        if let Some(info) = idx
            .checked_sub(info_start)
            .and_then(|line| info_lines.get(line))
        {
            let padding = art_width.saturating_sub(visible_width(art_line)) + SPLASH_GAP;
            output.push_str(&" ".repeat(padding));
            output.push_str(&info.render());
        }
        output.push('\n');
    }
    output
}

fn render_splash_stacked(art_lines: &[&str], info_lines: &[InfoLine]) -> String {
    let mut output = String::new();
    for (idx, art_line) in art_lines.iter().enumerate() {
        output.push_str(&purple_gradient_line(art_line, idx, art_lines.len()));
        output.push('\n');
    }
    output.push('\n');
    for info in info_lines {
        output.push_str(&info.render());
        output.push('\n');
    }
    output
}

fn render_splash_compact(info_lines: &[InfoLine]) -> String {
    let mut output = String::new();
    for info in info_lines {
        output.push_str(&info.render());
        output.push('\n');
    }
    output
}

/// A single line in the splash side panel, rendered with a fixed style.
enum InfoLine {
    /// Bright-orange heading.
    Head(String),
    /// Bold heading for the product name.
    Title(String),
    /// Dimmed body text (commands, values, paths).
    Body(String),
    Blank,
}

impl InfoLine {
    fn render(&self) -> String {
        match self {
            InfoLine::Head(text) => purple(text),
            InfoLine::Title(text) => purple_bold(text),
            InfoLine::Body(text) => dim(text),
            InfoLine::Blank => String::new(),
        }
    }

    /// Visible (uncoloured) width of the line, used to size the splash layout.
    fn visible_len(&self) -> usize {
        match self {
            InfoLine::Head(text) | InfoLine::Title(text) | InfoLine::Body(text) => {
                text.chars().count()
            }
            InfoLine::Blank => 0,
        }
    }
}

/// Side-panel content for the splash: branding plus the command menu. New
/// commands get a row here as they are added.
fn panel_info_lines() -> Vec<InfoLine> {
    let command = |name: &str, desc: &str| InfoLine::Body(format!("  {name:<16}{desc}"));
    vec![
        InfoLine::Title("Skopos".to_string()),
        InfoLine::Body("local-first AI usage observability".to_string()),
        InfoLine::Blank,
        InfoLine::Head("Commands".to_string()),
        command("help", "list commands"),
        command("work", "pick a project, launch CLI"),
        command("usage", "5h / weekly limit bars"),
        command("network", "connectivity dashboard"),
        command("claude -t/-w/-m", "usage by period"),
        command("claude models", "usage by model"),
        command("codex usage", "Codex 5h / weekly limits"),
        command("providers", "tracked providers"),
        command("claude import", "import Claude logs"),
        command("codex import", "import Codex rollout JSONLs"),
        command("gemini import", "import Gemini session JSONLs"),
        command("clear", "redraw splash"),
        command("exit", "quit skopos"),
        InfoLine::Blank,
        InfoLine::Head("Data".to_string()),
        InfoLine::Body("  ~/.local/share/skopos/skopos.db".to_string()),
    ]
}

fn purple_gradient_line(line: &str, index: usize, total_lines: usize) -> String {
    let denominator = total_lines.saturating_sub(1).max(1) as f32;
    let t = index as f32 / denominator;
    let start = (216.0, 180.0, 254.0);
    let end = (76.0, 29.0, 149.0);
    let r = lerp(start.0, end.0, t).round() as u8;
    let g = lerp(start.1, end.1, t).round() as u8;
    let b = lerp(start.2, end.2, t).round() as u8;
    rgb_text(line, r, g, b)
}

fn rgb_text(text: &str, r: u8, g: u8, b: u8) -> String {
    format!("\x1b[38;2;{r};{g};{b}m{text}\x1b[0m")
}

/// Bright-purple foreground text.
pub(crate) fn purple(text: &str) -> String {
    rgb_text(text, PURPLE.0, PURPLE.1, PURPLE.2)
}

/// Bold bright-purple foreground text.
pub(crate) fn purple_bold(text: &str) -> String {
    format!(
        "\x1b[1m\x1b[38;2;{};{};{}m{text}\x1b[0m",
        PURPLE.0, PURPLE.1, PURPLE.2
    )
}

/// Dimmed grey foreground text.
pub(crate) fn dim(text: &str) -> String {
    rgb_text(text, 140, 140, 140)
}

fn lerp(start: f32, end: f32, t: f32) -> f32 {
    start + (end - start) * t
}

fn visible_width(text: &str) -> usize {
    text.chars().count()
}

/// Compact human-readable token count, e.g. `250.5M`, `6.3K`, `512`.
fn human_tokens(n: i64) -> String {
    let value = n as f64;
    let abs = value.abs();
    if abs < 1_000.0 {
        n.to_string()
    } else if abs < 1_000_000.0 {
        format!("{:.1}K", value / 1_000.0)
    } else if abs < 1_000_000_000.0 {
        format!("{:.1}M", value / 1_000_000.0)
    } else {
        format!("{:.1}B", value / 1_000_000_000.0)
    }
}

/// Integer with thousands separators, e.g. `1,722`.
fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let bytes = digits.as_bytes();
    let mut out = String::new();
    for (idx, byte) in bytes.iter().enumerate() {
        if idx > 0 && (bytes.len() - idx).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*byte as char);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// Render an aligned table: column 0 left-aligned, the rest right-aligned.
/// The header row and underline are coloured; data rows stay plain.
fn render_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let columns = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|header| header.len()).collect();
    for row in rows {
        for (idx, cell) in row.iter().enumerate() {
            widths[idx] = widths[idx].max(cell.chars().count());
        }
    }

    let format_row = |cells: &[String]| -> String {
        let mut line = String::from("  ");
        for (idx, cell) in cells.iter().enumerate() {
            if idx == 0 {
                line.push_str(&format!("{:<width$}", cell, width = widths[idx]));
            } else {
                line.push_str(&format!("  {:>width$}", cell, width = widths[idx]));
            }
        }
        line
    };

    let header_cells: Vec<String> = headers.iter().map(|h| h.to_string()).collect();
    let total_width: usize = widths.iter().sum::<usize>() + 2 + (columns - 1) * 2;

    let mut out = String::new();
    out.push_str(&purple(&format_row(&header_cells)));
    out.push('\n');
    out.push_str(&dim(&format!(
        "  {}",
        "─".repeat(total_width.saturating_sub(2))
    )));
    out.push('\n');
    for row in rows {
        out.push_str(&format_row(row));
        out.push('\n');
    }
    out
}

fn today_range(now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let start = Utc
        .with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
        .unwrap();
    (start, start + chrono::Duration::days(1))
}

fn week_range(now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let (today_start, _) = today_range(now);
    let days_since_monday = now.weekday().num_days_from_monday() as i64;
    let start = today_start - chrono::Duration::days(days_since_monday);
    (start, start + chrono::Duration::days(7))
}

fn month_range(now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let start = Utc
        .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
        .unwrap();
    let end = if now.month() == 12 {
        Utc.with_ymd_and_hms(now.year() + 1, 1, 1, 0, 0, 0).unwrap()
    } else {
        Utc.with_ymd_and_hms(now.year(), now.month() + 1, 1, 0, 0, 0)
            .unwrap()
    };
    (start, end)
}

fn scan_claude(path: Option<PathBuf>) -> anyhow::Result<()> {
    let claude_home = path.unwrap_or_else(default_claude_home);
    let jsonl_paths = discover_claude_code_jsonl_paths(&claude_home)?;
    let mut model_totals: BTreeMap<String, ModelUsageSummary> = BTreeMap::new();
    let mut event_count = 0u64;

    for path in &jsonl_paths {
        for event in parse_usage_events_from_jsonl_path(path)? {
            event_count += 1;
            let summary = model_totals.entry(event.model.0).or_default();
            summary.input_tokens += event.input_tokens;
            summary.cached_input_tokens += event.cached_input_tokens.unwrap_or(0);
            summary.output_tokens += event.output_tokens;
            summary.total_tokens += event.total_tokens;
        }
    }

    println!("Claude Code scan");
    println!("home:   {}", claude_home.display());
    println!("files:  {}", jsonl_paths.len());
    println!("events: {}", event_count);

    if model_totals.is_empty() {
        println!("models: none found");
        return Ok(());
    }

    println!("models:");
    for (model, summary) in model_totals {
        println!(
            "  {model}: total={} input={} cached_input={} output={}",
            summary.total_tokens,
            summary.input_tokens,
            summary.cached_input_tokens,
            summary.output_tokens
        );
    }

    Ok(())
}

pub(crate) async fn providers_report(db_path: impl Into<PathBuf>) -> anyhow::Result<String> {
    let store = SkoposStore::connect_path(db_path.into()).await?;
    store.migrate().await?;
    let totals = store.usage_totals_by_model().await?;

    let mut report = String::new();
    report.push_str(&purple_bold("Tracked providers"));
    report.push_str("\n\n");

    if totals.is_empty() {
        report.push_str(&dim(
            "  No usage imported yet — run: skopos claude import\n",
        ));
        return Ok(report);
    }

    // Roll the per-model rows up to one row per provider.
    let mut by_provider: BTreeMap<String, (i64, i64, i64)> = BTreeMap::new();
    for total in &totals {
        let entry = by_provider.entry(total.provider.clone()).or_default();
        entry.0 += 1;
        entry.1 += total.events;
        entry.2 += total.total_tokens;
    }

    let rows: Vec<Vec<String>> = by_provider
        .into_iter()
        .map(|(provider, (models, events, tokens))| {
            vec![
                provider,
                models.to_string(),
                thousands(events),
                human_tokens(tokens),
            ]
        })
        .collect();

    report.push_str(&render_table(
        &["PROVIDER", "MODELS", "EVENTS", "TOTAL"],
        &rows,
    ));

    Ok(report)
}

pub(crate) async fn usage_by_model_report(db_path: impl Into<PathBuf>) -> anyhow::Result<String> {
    usage_by_model_report_filtered(db_path, None).await
}

pub(crate) async fn usage_by_model_report_filtered(
    db_path: impl Into<PathBuf>,
    provider: Option<&str>,
) -> anyhow::Result<String> {
    let store = SkoposStore::connect_path(db_path.into()).await?;
    store.migrate().await?;
    let totals = store.usage_totals_by_model_filtered(provider).await?;

    let mut report = String::new();
    let heading = match provider {
        Some("anthropic") => "Claude usage by model",
        Some("openai") => "Codex usage by model",
        Some("google") => "Gemini usage by model",
        _ => "Usage by model",
    };
    report.push_str(&purple_bold(heading));
    report.push_str("\n\n");

    if totals.is_empty() {
        let hint = match provider {
            Some("openai") => "  No Codex usage imported yet — run: skopos codex import\n",
            Some("google") => "  No Gemini usage imported yet — run: skopos gemini import\n",
            _ => "  No usage imported yet — run: skopos claude import\n",
        };
        report.push_str(&dim(hint));
        return Ok(report);
    }

    let rows: Vec<Vec<String>> = totals
        .iter()
        .map(|total| {
            vec![
                format!("{}/{}", total.provider, total.model),
                thousands(total.events),
                human_tokens(total.input_tokens),
                human_tokens(total.cached_input_tokens),
                human_tokens(total.output_tokens),
                human_tokens(total.total_tokens),
            ]
        })
        .collect();

    report.push_str(&render_table(
        &["MODEL", "EVENTS", "INPUT", "CACHED", "OUTPUT", "TOTAL"],
        &rows,
    ));

    Ok(report)
}

pub(crate) async fn usage_period_report(
    db_path: impl Into<PathBuf>,
    period: UsagePeriod,
) -> anyhow::Result<String> {
    usage_period_report_filtered(db_path, period, None).await
}

pub(crate) async fn usage_period_report_filtered(
    db_path: impl Into<PathBuf>,
    period: UsagePeriod,
    provider: Option<&str>,
) -> anyhow::Result<String> {
    let now = Utc::now();
    let (label, start, end) = match period {
        UsagePeriod::Today => {
            let (start, end) = today_range(now);
            ("today", start, end)
        }
        UsagePeriod::Week => {
            let (start, end) = week_range(now);
            ("this week", start, end)
        }
        UsagePeriod::Month => {
            let (start, end) = month_range(now);
            ("this month", start, end)
        }
    };

    let store = SkoposStore::connect_path(db_path.into()).await?;
    store.migrate().await?;
    let totals = store
        .usage_totals_between_filtered(start, end, provider)
        .await?;
    let by_model = store
        .usage_totals_by_model_between_filtered(start, end, provider)
        .await?;

    let heading = match provider {
        Some("anthropic") => format!("Claude usage {label}"),
        Some("openai") => format!("Codex usage {label}"),
        Some("google") => format!("Gemini usage {label}"),
        _ => format!("Usage {label}"),
    };
    let mut report = String::new();
    report.push_str(&purple_bold(&heading));
    report.push('\n');
    report.push_str(&dim(&format!(
        "  {} → {}",
        start.format("%Y-%m-%d"),
        end.format("%Y-%m-%d"),
    )));
    report.push_str("\n\n");

    let pairs = [
        ("events", thousands(totals.events)),
        ("input", human_tokens(totals.input_tokens)),
        ("cached", human_tokens(totals.cached_input_tokens)),
        ("output", human_tokens(totals.output_tokens)),
        ("total", human_tokens(totals.total_tokens)),
    ];
    for (label, value) in pairs {
        report.push_str(&format!(
            "  {}{:>10}\n",
            purple(&format!("{label:<8}")),
            value,
        ));
    }

    let catalog = Catalog::load_with_overrides(&default_overrides_path())?;
    let (cost_usd, unpriced) = estimate_period_cost(&catalog, &by_model);
    let cost_value = if totals.events == 0 {
        "—".to_string()
    } else {
        format!("${:.2}", cost_usd)
    };
    report.push_str(&format!(
        "  {}{:>10}\n",
        purple(&format!("{:<8}", "est cost")),
        cost_value,
    ));
    if !unpriced.is_empty() {
        let mut models = unpriced
            .iter()
            .map(|(p, m)| format!("{p}/{m}"))
            .collect::<Vec<_>>();
        models.sort();
        models.dedup();
        report.push_str(&dim(&format!("    no price for: {}\n", models.join(", "),)));
    }

    Ok(report)
}

/// Sum the catalog's USD estimate across each per-model row. Returns the
/// total dollars and the list of `(provider, model)` pairs that the
/// catalog does not know about (so the report can flag them).
fn estimate_period_cost(
    catalog: &Catalog,
    by_model: &[skopos_store::UsageModelTotal],
) -> (f64, Vec<(String, String)>) {
    let mut total = 0.0;
    let mut unpriced = Vec::new();
    for row in by_model {
        let input = row.input_tokens.max(0) as u64;
        let cached = row.cached_input_tokens.max(0) as u64;
        let output = row.output_tokens.max(0) as u64;
        match catalog.estimate(&row.provider, &row.model, input, Some(cached), output) {
            Some(money) => total += money.amount,
            None => unpriced.push((row.provider.clone(), row.model.clone())),
        }
    }
    (total, unpriced)
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum UsagePeriod {
    Today,
    Week,
    Month,
}

async fn import_claude_from_home(
    claude_home: impl Into<PathBuf>,
    db_path: impl Into<PathBuf>,
) -> anyhow::Result<ClaudeImportReport> {
    let claude_home = claude_home.into();
    let db_path = db_path.into();

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let store = SkoposStore::connect_path(&db_path).await?;
    store.migrate().await?;

    let jsonl_paths = discover_claude_code_jsonl_paths(&claude_home)?;
    let mut report = ClaudeImportReport {
        files: jsonl_paths.len() as u64,
        ..Default::default()
    };

    for path in jsonl_paths {
        for event in parse_usage_events_from_jsonl_path(path)? {
            report.seen_events += 1;
            let dedupe_key = claude_usage_dedupe_key(&event);
            let result = store.insert_usage_event_once(&event, &dedupe_key).await?;
            if result.inserted {
                report.inserted_events += 1;
            } else {
                report.duplicate_events += 1;
            }
        }
    }

    Ok(report)
}

fn claude_usage_dedupe_key(event: &UsageEvent) -> String {
    if let Some(uuid) = event
        .metadata
        .get("claude_code_uuid")
        .and_then(|value| value.as_str())
    {
        return format!("claude-code:uuid:{uuid}");
    }

    if let (Some(session_id), Some(request_id)) = (&event.session_id, &event.request_id) {
        return format!("claude-code:session:{session_id}:request:{request_id}");
    }

    format!(
        "claude-code:fallback:{}:{}:{}:{}:{}",
        event.timestamp.to_rfc3339(),
        event.model.0,
        event.input_tokens,
        event.cached_input_tokens.unwrap_or(0),
        event.output_tokens
    )
}

fn default_claude_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
}

fn default_codex_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex")
}

fn scan_codex(path: Option<PathBuf>) -> anyhow::Result<()> {
    let codex_home = path.unwrap_or_else(default_codex_home);
    let jsonl_paths = discover_codex_rollout_paths(&codex_home)?;
    let mut model_totals: BTreeMap<String, ModelUsageSummary> = BTreeMap::new();
    let mut event_count = 0u64;

    for path in &jsonl_paths {
        for event in parse_usage_events_from_rollout_path(path)? {
            event_count += 1;
            let summary = model_totals.entry(event.model.0).or_default();
            summary.input_tokens += event.input_tokens;
            summary.cached_input_tokens += event.cached_input_tokens.unwrap_or(0);
            summary.output_tokens += event.output_tokens;
            summary.total_tokens += event.total_tokens;
        }
    }

    println!("Codex scan");
    println!("home:   {}", codex_home.display());
    println!("files:  {}", jsonl_paths.len());
    println!("events: {}", event_count);

    if model_totals.is_empty() {
        println!("models: none found");
        return Ok(());
    }

    println!("models:");
    for (model, summary) in model_totals {
        println!(
            "  {model}: total={} input={} cached_input={} output={}",
            summary.total_tokens,
            summary.input_tokens,
            summary.cached_input_tokens,
            summary.output_tokens
        );
    }

    Ok(())
}

async fn import_codex_from_home(
    codex_home: impl Into<PathBuf>,
    db_path: impl Into<PathBuf>,
) -> anyhow::Result<CodexImportReport> {
    let codex_home = codex_home.into();
    let db_path = db_path.into();

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let store = SkoposStore::connect_path(&db_path).await?;
    store.migrate().await?;

    let jsonl_paths = discover_codex_rollout_paths(&codex_home)?;
    let mut report = CodexImportReport {
        files: jsonl_paths.len() as u64,
        ..Default::default()
    };

    for path in jsonl_paths {
        for event in parse_usage_events_from_rollout_path(path)? {
            report.seen_events += 1;
            let dedupe_key = codex_usage_dedupe_key(&event);
            let result = store.insert_usage_event_once(&event, &dedupe_key).await?;
            if result.inserted {
                report.inserted_events += 1;
            } else {
                report.duplicate_events += 1;
            }
        }
    }

    Ok(report)
}

/// Idempotent best-effort import: if `~/.codex/sessions/` has rollout
/// JSONLs whose mtime is newer than the latest `provider="openai"`
/// event in the store, run a full import. The collector already
/// dedupes per `session:turn:ts` so a full sweep is safe.
///
/// All errors are swallowed silently — this hook must never break the
/// report that called it. The caller still gets to render whatever the
/// store has.
pub(crate) async fn auto_import_codex_if_stale(db_path: &Path) {
    if let Err(err) = try_auto_import_codex(db_path).await {
        eprintln!("{}", dim(&format!("  (codex auto-import skipped: {err})")));
    }
}

async fn try_auto_import_codex(db_path: &Path) -> anyhow::Result<()> {
    use anyhow::Context;

    let codex_home = default_codex_home();
    let sessions_root = codex_home.join("sessions");
    if !sessions_root.exists() {
        return Ok(());
    }

    let store = SkoposStore::connect_path(db_path)
        .await
        .context("connect skopos store")?;
    store.migrate().await.context("migrate skopos store")?;
    let last = store
        .latest_usage_event_timestamp_for_provider("openai")
        .await
        .context("read latest openai timestamp")?;
    drop(store);

    if !jsonls_newer_than(&sessions_root, last)? {
        return Ok(());
    }

    let _ = import_codex_from_home(codex_home, db_path).await?;
    Ok(())
}

/// Walk `<root>/**/*.jsonl` looking for any file with `mtime >
/// threshold`. Returns `Ok(true)` on first hit, `Ok(false)` if nothing
/// is newer. If `threshold` is `None`, any jsonl counts as "newer"
/// (i.e. first-time import).
pub(crate) fn jsonls_newer_than(
    sessions_root: &Path,
    threshold: Option<DateTime<Utc>>,
) -> anyhow::Result<bool> {
    use std::time::SystemTime;
    let threshold_st = threshold.map(|t| {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(t.timestamp().max(0) as u64)
    });

    for entry in walkdir_jsonl(sessions_root)? {
        let mtime = std::fs::metadata(&entry)?.modified()?;
        match threshold_st {
            None => return Ok(true),
            Some(t) if mtime > t => return Ok(true),
            _ => {}
        }
    }
    Ok(false)
}

fn walkdir_jsonl(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walkdir_jsonl_inner(root, &mut out)?;
    Ok(out)
}

fn walkdir_jsonl_inner(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            walkdir_jsonl_inner(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            out.push(path);
        }
    }
    Ok(())
}

fn codex_usage_dedupe_key(event: &UsageEvent) -> String {
    match (&event.session_id, &event.request_id) {
        (Some(session), Some(turn)) => format!(
            "codex:session:{session}:turn:{turn}:ts:{}",
            event.timestamp.to_rfc3339()
        ),
        (Some(session), None) => format!(
            "codex:session:{session}:ts:{}",
            event.timestamp.to_rfc3339()
        ),
        _ => format!(
            "codex:fallback:{}:{}:{}:{}:{}",
            event.timestamp.to_rfc3339(),
            event.model.0,
            event.input_tokens,
            event.cached_input_tokens.unwrap_or(0),
            event.output_tokens,
        ),
    }
}

fn default_gemini_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".gemini")
}

fn scan_gemini(path: Option<PathBuf>) -> anyhow::Result<()> {
    let gemini_home = path.unwrap_or_else(default_gemini_home);
    let jsonl_paths = discover_gemini_session_paths(&gemini_home)?;
    let mut model_totals: BTreeMap<String, ModelUsageSummary> = BTreeMap::new();
    let mut event_count = 0u64;

    for path in &jsonl_paths {
        for event in parse_usage_events_from_session_path(path)? {
            event_count += 1;
            let summary = model_totals.entry(event.model.0).or_default();
            summary.input_tokens += event.input_tokens;
            summary.cached_input_tokens += event.cached_input_tokens.unwrap_or(0);
            summary.output_tokens += event.output_tokens;
            summary.total_tokens += event.total_tokens;
        }
    }

    println!("Gemini scan");
    println!("home:   {}", gemini_home.display());
    println!("files:  {}", jsonl_paths.len());
    println!("events: {}", event_count);

    if model_totals.is_empty() {
        println!("models: none found");
        return Ok(());
    }

    println!("models:");
    for (model, summary) in model_totals {
        println!(
            "  {model}: total={} input={} cached_input={} output={}",
            summary.total_tokens,
            summary.input_tokens,
            summary.cached_input_tokens,
            summary.output_tokens
        );
    }

    Ok(())
}

async fn import_gemini_from_home(
    gemini_home: impl Into<PathBuf>,
    db_path: impl Into<PathBuf>,
) -> anyhow::Result<GeminiImportReport> {
    let gemini_home = gemini_home.into();
    let db_path = db_path.into();

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let store = SkoposStore::connect_path(&db_path).await?;
    store.migrate().await?;

    let jsonl_paths = discover_gemini_session_paths(&gemini_home)?;
    let mut report = GeminiImportReport {
        files: jsonl_paths.len() as u64,
        ..Default::default()
    };

    for path in jsonl_paths {
        for event in parse_usage_events_from_session_path(path)? {
            report.seen_events += 1;
            let dedupe_key = gemini_usage_dedupe_key(&event);
            let result = store.insert_usage_event_once(&event, &dedupe_key).await?;
            if result.inserted {
                report.inserted_events += 1;
            } else {
                report.duplicate_events += 1;
            }
        }
    }

    Ok(report)
}

fn gemini_usage_dedupe_key(event: &UsageEvent) -> String {
    match (&event.session_id, &event.request_id) {
        (Some(session), Some(msg)) => format!("gemini:session:{session}:msg:{msg}"),
        (Some(session), None) => format!(
            "gemini:session:{session}:ts:{}",
            event.timestamp.to_rfc3339()
        ),
        _ => format!(
            "gemini:fallback:{}:{}:{}:{}",
            event.timestamp.to_rfc3339(),
            event.model.0,
            event.input_tokens,
            event.output_tokens,
        ),
    }
}

/// Idempotent best-effort import: if `~/.gemini/tmp/` has session
/// JSONLs whose mtime is newer than the latest `provider="google"`
/// event in the store, run a full import. The collector already
/// dedupes per `session:msg` so a full sweep is safe.
///
/// All errors are swallowed silently — this hook must never break the
/// report that called it.
pub(crate) async fn auto_import_gemini_if_stale(db_path: &Path) {
    if let Err(err) = try_auto_import_gemini(db_path).await {
        eprintln!("{}", dim(&format!("  (gemini auto-import skipped: {err})")));
    }
}

async fn try_auto_import_gemini(db_path: &Path) -> anyhow::Result<()> {
    use anyhow::Context;

    let gemini_home = default_gemini_home();
    let tmp_root = gemini_home.join("tmp");
    if !tmp_root.exists() {
        return Ok(());
    }

    let store = SkoposStore::connect_path(db_path)
        .await
        .context("connect skopos store")?;
    store.migrate().await.context("migrate skopos store")?;
    let last = store
        .latest_usage_event_timestamp_for_provider("google")
        .await
        .context("read latest google timestamp")?;
    drop(store);

    if !jsonls_newer_than(&tmp_root, last)? {
        return Ok(());
    }

    let _ = import_gemini_from_home(gemini_home, db_path).await?;
    Ok(())
}

fn default_db_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("share")
        .join("skopos")
        .join("skopos.db")
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ClaudeImportReport {
    files: u64,
    seen_events: u64,
    inserted_events: u64,
    duplicate_events: u64,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct CodexImportReport {
    files: u64,
    seen_events: u64,
    inserted_events: u64,
    duplicate_events: u64,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct GeminiImportReport {
    files: u64,
    seen_events: u64,
    inserted_events: u64,
    duplicate_events: u64,
}

#[derive(Debug, Default)]
struct ModelUsageSummary {
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use skopos_store::SkoposStore;

    #[tokio::test]
    async fn usage_by_model_report_reads_persisted_events() {
        let temp_dir = std::env::temp_dir().join(format!(
            "skopos-cli-usage-report-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join("skopos.db");
        let store = SkoposStore::connect_path(&db_path).await.unwrap();
        store.migrate().await.unwrap();
        let claude_home = temp_dir.join(".claude");
        let claude_project_dir = claude_home.join("projects").join("-tmp-project");
        std::fs::create_dir_all(&claude_project_dir).unwrap();
        std::fs::write(
            claude_project_dir.join("session.jsonl"),
            r#"{"message":{"model":"claude-opus-4-7","id":"msg_a","role":"assistant","usage":{"input_tokens":2,"cache_read_input_tokens":5,"output_tokens":3}},"timestamp":"2026-05-13T19:58:08.012Z","cwd":"/tmp/project","sessionId":"s1"}"#,
        )
        .unwrap();
        import_claude_from_home(&claude_home, &db_path)
            .await
            .unwrap();

        let report = usage_by_model_report(&db_path).await.unwrap();

        assert!(report.contains("anthropic/claude-opus-4-7"));
        assert!(report.contains("MODEL"));
        // 2 input + 5 cache_read + 3 output = 10 total tokens, 1 event.
        assert!(report.contains("10"));
    }

    #[test]
    fn human_tokens_uses_compact_suffixes() {
        assert_eq!(human_tokens(512), "512");
        assert_eq!(human_tokens(6_316_399), "6.3M");
        assert_eq!(human_tokens(250_473_138), "250.5M");
    }

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(1_722), "1,722");
        assert_eq!(thousands(100), "100");
        assert_eq!(thousands(1_822_000), "1,822,000");
    }

    #[test]
    fn estimate_period_cost_sums_per_model_and_flags_unpriced() {
        use skopos_store::UsageModelTotal;

        let catalog = Catalog::defaults();
        let rows = vec![
            UsageModelTotal {
                provider: "openai".to_string(),
                model: "gpt-5.5".to_string(),
                events: 1,
                input_tokens: 1_000_000,
                cached_input_tokens: 0,
                output_tokens: 0,
                total_tokens: 1_000_000,
            },
            UsageModelTotal {
                provider: "mystery".to_string(),
                model: "ghost".to_string(),
                events: 1,
                input_tokens: 100,
                cached_input_tokens: 0,
                output_tokens: 100,
                total_tokens: 200,
            },
        ];

        let (total, unpriced) = estimate_period_cost(&catalog, &rows);
        assert!((total - 5.0).abs() < 1e-9);
        assert_eq!(unpriced, vec![("mystery".to_string(), "ghost".to_string())]);
    }

    #[test]
    fn jsonls_newer_than_handles_empty_and_mtime_cases() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{Duration, SystemTime};

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let temp_dir = std::env::temp_dir().join(format!(
            "skopos-cli-auto-import-test-{}-{}",
            std::process::id(),
            unique,
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let sessions = temp_dir.join("sessions");
        let day_dir = sessions.join("2026").join("05").join("18");
        std::fs::create_dir_all(&day_dir).unwrap();

        // 1. Empty sessions dir (no jsonl files) → false.
        assert!(!jsonls_newer_than(&sessions, None).unwrap());

        // 2. One rollout, no threshold → true.
        let rollout = day_dir.join("rollout-test.jsonl");
        std::fs::write(&rollout, "{}\n").unwrap();
        assert!(jsonls_newer_than(&sessions, None).unwrap());

        // Pin the rollout's mtime to a known instant so the comparisons below
        // don't race with the filesystem clock.
        let pinned = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&rollout)
            .unwrap();
        file.set_modified(pinned).unwrap();
        drop(file);

        // 3. Threshold strictly newer than the file → false.
        let newer_threshold = Utc.timestamp_opt((1_700_000_000 + 60) as i64, 0).unwrap();
        assert!(!jsonls_newer_than(&sessions, Some(newer_threshold)).unwrap());

        // 4. Threshold strictly older than the file → true.
        let older_threshold = Utc.timestamp_opt((1_700_000_000 - 60) as i64, 0).unwrap();
        assert!(jsonls_newer_than(&sessions, Some(older_threshold)).unwrap());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
