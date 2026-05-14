use chrono::{Datelike, TimeZone, Utc};
use clap::{Parser, Subcommand};
use skopos_collectors::claude_code::{
    discover_claude_code_jsonl_paths, parse_usage_events_from_jsonl_path,
};
use skopos_core::UsageEvent;
use skopos_store::SkoposStore;
use std::{collections::BTreeMap, path::PathBuf};

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
    /// Inspect persisted AI usage.
    Usage {
        #[command(subcommand)]
        command: UsageCommand,
    },
    /// Inspect or import Claude Code local usage logs.
    Claude {
        #[command(subcommand)]
        command: ClaudeCommand,
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => print!("{}", welcome_screen()),
        Some(Command::Status) => println!("Skopos status: bootstrapped"),
        Some(Command::Doctor) => {
            println!("Skopos doctor");
            println!("config: ~/.config/skopos/config.toml");
            println!("data:   {}", default_db_path().display());
            println!("logs:   ~/.local/state/skopos/skopos.log");
        }
        Some(Command::Usage { command }) => match command {
            UsageCommand::ByModel { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                print!("{}", usage_by_model_report(&db_path).await?);
            }
            UsageCommand::Today { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                print!(
                    "{}",
                    usage_period_report(&db_path, UsagePeriod::Today).await?
                );
            }
            UsageCommand::Month { db } => {
                let db_path = db.unwrap_or_else(default_db_path);
                print!(
                    "{}",
                    usage_period_report(&db_path, UsagePeriod::Month).await?
                );
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
        },
    }

    Ok(())
}

const SKOPOS_ASCII: &str = include_str!("../assets/skopos-ascii.txt");

fn welcome_screen() -> String {
    let art_lines: Vec<&str> = SKOPOS_ASCII.trim_end_matches('\n').lines().collect();
    let info_lines = [
        "Skopos",
        "local-first AI usage observability",
        "",
        "Commands",
        "  skopos claude import",
        "  skopos usage by-model",
        "  skopos usage today",
        "  skopos usage month",
        "",
        "Data",
        "  ~/.local/share/skopos/skopos.db",
    ];
    let info_start = art_lines.len().saturating_sub(info_lines.len()) / 2;
    let art_width = art_lines
        .iter()
        .map(|line| visible_width(line))
        .max()
        .unwrap_or(0);
    let mut output = String::new();

    for (idx, art_line) in art_lines.iter().enumerate() {
        output.push_str(&purple_gradient_line(art_line, idx, art_lines.len()));
        if let Some(info) = idx
            .checked_sub(info_start)
            .and_then(|line| info_lines.get(line))
        {
            let padding = art_width.saturating_sub(visible_width(art_line)) + 4;
            output.push_str(&" ".repeat(padding));
            output.push_str(&purple_text(info, 189, 147, 249));
        }
        output.push('\n');
    }

    output
}

fn purple_gradient_line(line: &str, index: usize, total_lines: usize) -> String {
    let denominator = total_lines.saturating_sub(1).max(1) as f32;
    let t = index as f32 / denominator;
    let start = (216.0, 180.0, 254.0);
    let end = (76.0, 29.0, 149.0);
    let r = lerp(start.0, end.0, t).round() as u8;
    let g = lerp(start.1, end.1, t).round() as u8;
    let b = lerp(start.2, end.2, t).round() as u8;
    purple_text(line, r, g, b)
}

fn purple_text(text: &str, r: u8, g: u8, b: u8) -> String {
    format!("\x1b[38;2;{r};{g};{b}m{text}\x1b[0m")
}

fn lerp(start: f32, end: f32, t: f32) -> f32 {
    start + (end - start) * t
}

fn visible_width(text: &str) -> usize {
    text.chars().count()
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

async fn usage_by_model_report(db_path: impl Into<PathBuf>) -> anyhow::Result<String> {
    let store = SkoposStore::connect_path(db_path.into()).await?;
    store.migrate().await?;
    let totals = store.usage_totals_by_model().await?;
    let mut report = String::from("Usage by model\n");

    if totals.is_empty() {
        report.push_str("models: none found\n");
        return Ok(report);
    }

    for total in totals {
        report.push_str(&format!(
            "{} {}: events={} total={} input={} cached_input={} output={}\n",
            total.provider,
            total.model,
            total.events,
            total.total_tokens,
            total.input_tokens,
            total.cached_input_tokens,
            total.output_tokens
        ));
    }

    Ok(report)
}

async fn usage_period_report(
    db_path: impl Into<PathBuf>,
    period: UsagePeriod,
) -> anyhow::Result<String> {
    let now = Utc::now();
    let (label, start, end) = match period {
        UsagePeriod::Today => {
            let start = Utc
                .with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
                .unwrap();
            ("today", start, start + chrono::Duration::days(1))
        }
        UsagePeriod::Month => {
            let start = Utc
                .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
                .unwrap();
            let end = if now.month() == 12 {
                Utc.with_ymd_and_hms(now.year() + 1, 1, 1, 0, 0, 0).unwrap()
            } else {
                Utc.with_ymd_and_hms(now.year(), now.month() + 1, 1, 0, 0, 0)
                    .unwrap()
            };
            ("month", start, end)
        }
    };

    let store = SkoposStore::connect_path(db_path.into()).await?;
    store.migrate().await?;
    let totals = store.usage_totals_between(start, end).await?;

    Ok(format!(
        "Usage {label}\nrange: {} -> {}\nevents={} total={} input={} cached_input={} output={}\n",
        start.to_rfc3339(),
        end.to_rfc3339(),
        totals.events,
        totals.total_tokens,
        totals.input_tokens,
        totals.cached_input_tokens,
        totals.output_tokens
    ))
}

#[derive(Debug, Clone, Copy)]
enum UsagePeriod {
    Today,
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

        assert!(report.contains("claude-opus-4-7"));
        assert!(report.contains("total=10"));
        assert!(report.contains("events=1"));
    }
}
