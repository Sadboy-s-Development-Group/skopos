use chrono::Utc;
use clap::Parser;
use skopos_pricing::{default_overrides_path, Catalog};
use skopos_store::SkoposStore;
use std::{collections::BTreeMap, path::PathBuf};

mod agent;
mod cli;
mod codex_limits;
mod config;
mod format;
mod icons;
mod install;
mod limits;
mod local_usage;
mod network;
mod providers;
mod repl;
mod splash;
mod theme;
mod work;

use agent::{auto_import_if_stale, import_report, scan, Agent};
use cli::{AgentCommand, Cli, CodexCommand, Command, NetworkCommand, UsageCommand};
use format::{
    human_tokens, month_range, render_table, thousands, today_range, week_range, UsagePeriod,
};
use theme::{dim, purple, purple_bold};

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
            auto_import_if_stale(Agent::Codex, &db_path).await;
            auto_import_if_stale(Agent::Gemini, &db_path).await;
            print!("{}", providers_report(&db_path).await?);
        }
        Some(Command::Usage { command }) => match command {
            None => print!("{}", usage_limits_report().await?),
            Some(UsageCommand::ByModel { db }) => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_if_stale(Agent::Codex, &db_path).await;
                auto_import_if_stale(Agent::Gemini, &db_path).await;
                print!("{}", usage_by_model_report(&db_path).await?);
            }
            Some(UsageCommand::Today { db }) => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_if_stale(Agent::Codex, &db_path).await;
                auto_import_if_stale(Agent::Gemini, &db_path).await;
                print!(
                    "{}",
                    usage_period_report(&db_path, UsagePeriod::Today).await?
                );
            }
            Some(UsageCommand::Month { db }) => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_if_stale(Agent::Codex, &db_path).await;
                auto_import_if_stale(Agent::Gemini, &db_path).await;
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
            AgentCommand::Scan { path } => print!("{}", scan(Agent::Claude, path)?),
            AgentCommand::Import { path, db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                print!("{}", import_report("anthropic", path, &db_path).await?);
            }
            AgentCommand::Today { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Today, Some("anthropic"))
                        .await?
                );
            }
            AgentCommand::Week { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Week, Some("anthropic"))
                        .await?
                );
            }
            AgentCommand::Month { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Month, Some("anthropic"))
                        .await?
                );
            }
            AgentCommand::Models { db } => {
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
            CodexCommand::Scan { path } => print!("{}", scan(Agent::Codex, path)?),
            CodexCommand::Import { path, db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                print!("{}", import_report("openai", path, &db_path).await?);
            }
            CodexCommand::Today { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_if_stale(Agent::Codex, &db_path).await;
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Today, Some("openai"))
                        .await?
                );
            }
            CodexCommand::Week { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_if_stale(Agent::Codex, &db_path).await;
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Week, Some("openai"))
                        .await?
                );
            }
            CodexCommand::Month { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_if_stale(Agent::Codex, &db_path).await;
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Month, Some("openai"))
                        .await?
                );
            }
            CodexCommand::Models { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_if_stale(Agent::Codex, &db_path).await;
                print!(
                    "{}",
                    usage_by_model_report_filtered(&db_path, Some("openai")).await?
                );
            }
        },
        Some(Command::Gemini { command }) => match command {
            AgentCommand::Scan { path } => print!("{}", scan(Agent::Gemini, path)?),
            AgentCommand::Import { path, db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                print!("{}", import_report("google", path, &db_path).await?);
            }
            AgentCommand::Today { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_if_stale(Agent::Gemini, &db_path).await;
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Today, Some("google"))
                        .await?
                );
            }
            AgentCommand::Week { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_if_stale(Agent::Gemini, &db_path).await;
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Week, Some("google"))
                        .await?
                );
            }
            AgentCommand::Month { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_if_stale(Agent::Gemini, &db_path).await;
                print!(
                    "{}",
                    usage_period_report_filtered(&db_path, UsagePeriod::Month, Some("google"))
                        .await?
                );
            }
            AgentCommand::Models { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                auto_import_if_stale(Agent::Gemini, &db_path).await;
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

fn default_db_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("share")
        .join("skopos")
        .join("skopos.db")
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
        agent::import_from_home(Agent::Claude, &claude_home, &db_path)
            .await
            .unwrap();

        let report = usage_by_model_report(&db_path).await.unwrap();

        assert!(report.contains("anthropic/claude-opus-4-7"));
        assert!(report.contains("MODEL"));
        // 2 input + 5 cache_read + 3 output = 10 total tokens, 1 event.
        assert!(report.contains("10"));
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
}
