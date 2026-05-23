use clap::Parser;
use std::path::PathBuf;

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
mod report;
mod splash;
mod statusline;
mod theme;
mod update;
mod work;

use agent::{auto_import_if_stale, import_report, scan, Agent};
use cli::{AgentCommand, Cli, CodexCommand, Command, NetworkCommand, UsageCommand};
use format::UsagePeriod;
use report::{
    codex_refresh_report, codex_usage_report, providers_report, usage_by_model_report,
    usage_by_model_report_filtered, usage_limits_report, usage_period_report,
    usage_period_report_filtered,
};
use statusline::{run_install, run_statusline, run_uninstall};

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
            let db_path = refresh_and_resolve_db(db).await;
            print!("{}", providers_report(&db_path).await?);
        }
        Some(Command::Usage { command }) => run_usage_command(command).await?,
        Some(Command::Claude { command }) => run_agent_command(Agent::Claude, command).await?,
        Some(Command::Gemini { command }) => run_agent_command(Agent::Gemini, command).await?,
        Some(Command::Hermes { command }) => run_agent_command(Agent::Hermes, command).await?,
        Some(Command::Codex { command }) => run_codex_command(command).await?,
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
        Some(Command::Update { check }) => {
            // self_update wraps a *blocking* reqwest client, so calling
            // it on the tokio main thread trips "Cannot drop a runtime
            // in a context where blocking is not allowed". Hop onto the
            // blocking pool, where dropping a sync HTTP runtime is fine.
            let out = tokio::task::spawn_blocking(move || update::run(check)).await??;
            print!("{out}");
        }
    }

    Ok(())
}

/// `skopos usage [...]` — the aggregate (cross-provider) usage commands.
async fn run_usage_command(command: Option<UsageCommand>) -> anyhow::Result<()> {
    let Some(command) = command else {
        print!("{}", usage_limits_report().await?);
        return Ok(());
    };
    match command {
        UsageCommand::Install { force } => print!("{}", run_install(force)?),
        UsageCommand::Uninstall { force } => print!("{}", run_uninstall(force)?),
        UsageCommand::ByModel { db } => {
            let db_path = refresh_and_resolve_db(db).await;
            print!("{}", usage_by_model_report(&db_path).await?);
        }
        UsageCommand::Today { db } => {
            let db_path = refresh_and_resolve_db(db).await;
            print!(
                "{}",
                usage_period_report(&db_path, UsagePeriod::Today).await?
            );
        }
        UsageCommand::Month { db } => {
            let db_path = refresh_and_resolve_db(db).await;
            print!(
                "{}",
                usage_period_report(&db_path, UsagePeriod::Month).await?
            );
        }
    }
    Ok(())
}

/// `skopos claude|gemini [...]`, and the shared half of `skopos codex`.
/// `auto_import_if_stale` is a no-op for agents without a staleness root
/// (Claude), so calling it before every read is safe.
async fn run_agent_command(agent: Agent, command: AgentCommand) -> anyhow::Result<()> {
    let report = match command {
        AgentCommand::Scan { path } => scan(agent, path)?,
        AgentCommand::Import { path, db } => {
            import_report(agent.provider(), path, &db.unwrap_or_else(default_db_path)).await?
        }
        AgentCommand::Today { db } => agent_period_report(agent, db, UsagePeriod::Today).await?,
        AgentCommand::Week { db } => agent_period_report(agent, db, UsagePeriod::Week).await?,
        AgentCommand::Month { db } => agent_period_report(agent, db, UsagePeriod::Month).await?,
        AgentCommand::Models { db } => {
            let db_path = db.unwrap_or_else(default_db_path);
            auto_import_if_stale(agent, &db_path).await;
            usage_by_model_report_filtered(&db_path, Some(agent.provider())).await?
        }
    };
    print!("{report}");
    Ok(())
}

/// `skopos codex [...]` — `usage` / `refresh` are Codex-only; the rest is
/// the same surface every agent shares.
async fn run_codex_command(command: CodexCommand) -> anyhow::Result<()> {
    let shared = match command {
        CodexCommand::Usage => {
            print!("{}", codex_usage_report()?);
            return Ok(());
        }
        CodexCommand::Refresh => {
            print!("{}", codex_refresh_report().await?);
            return Ok(());
        }
        CodexCommand::Scan { path } => AgentCommand::Scan { path },
        CodexCommand::Import { path, db } => AgentCommand::Import { path, db },
        CodexCommand::Today { db } => AgentCommand::Today { db },
        CodexCommand::Week { db } => AgentCommand::Week { db },
        CodexCommand::Month { db } => AgentCommand::Month { db },
        CodexCommand::Models { db } => AgentCommand::Models { db },
    };
    run_agent_command(Agent::Codex, shared).await
}

/// Resolve a period report for one agent, importing fresh logs first.
async fn agent_period_report(
    agent: Agent,
    db: Option<PathBuf>,
    period: UsagePeriod,
) -> anyhow::Result<String> {
    let db_path = db.unwrap_or_else(default_db_path);
    auto_import_if_stale(agent, &db_path).await;
    usage_period_report_filtered(&db_path, period, Some(agent.provider())).await
}

/// Resolve the store path for an aggregate `usage` report, first pulling
/// in any fresh Codex / Gemini / Hermes logs so the totals are not stale.
/// Claude is imported explicitly and has nothing to refresh here.
async fn refresh_and_resolve_db(db: Option<PathBuf>) -> PathBuf {
    let db_path = db.unwrap_or_else(default_db_path);
    auto_import_if_stale(Agent::Codex, &db_path).await;
    auto_import_if_stale(Agent::Gemini, &db_path).await;
    auto_import_if_stale(Agent::Hermes, &db_path).await;
    db_path
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
