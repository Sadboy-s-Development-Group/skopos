use chrono::{DateTime, Datelike, TimeZone, Utc};
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
        None => print!("{}", welcome_screen(&default_db_path()).await),
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

/// Bright orange used for side-panel text, table headers and labels.
const ORANGE: (u8, u8, u8) = (255, 167, 38);

async fn welcome_screen(db_path: &std::path::Path) -> String {
    let art_lines: Vec<&str> = SKOPOS_ASCII.trim_end_matches('\n').lines().collect();
    let info_lines = match usage_snapshot(db_path).await {
        Some(snapshot) => snapshot_info_lines(&snapshot),
        None => empty_info_lines(),
    };
    let info_start = art_lines.len().saturating_sub(info_lines.len()) / 2;
    let art_width = art_lines
        .iter()
        .map(|line| visible_width(line))
        .max()
        .unwrap_or(0);
    let mut output = String::new();

    for (idx, art_line) in art_lines.iter().enumerate() {
        output.push_str(&orange_gradient_line(art_line, idx, art_lines.len()));
        if let Some(info) = idx
            .checked_sub(info_start)
            .and_then(|line| info_lines.get(line))
        {
            let padding = art_width.saturating_sub(visible_width(art_line)) + 4;
            output.push_str(&" ".repeat(padding));
            output.push_str(&info.render());
        }
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
            InfoLine::Head(text) => orange(text),
            InfoLine::Title(text) => orange_bold(text),
            InfoLine::Body(text) => dim(text),
            InfoLine::Blank => String::new(),
        }
    }
}

fn snapshot_info_lines(snapshot: &UsageSnapshot) -> Vec<InfoLine> {
    let mut lines = vec![
        InfoLine::Title("Skopos".to_string()),
        InfoLine::Body("local-first AI usage observability".to_string()),
        InfoLine::Blank,
        InfoLine::Head("This month".to_string()),
        InfoLine::Body(format!(
            "  {} tokens  ·  {} events",
            human_tokens(snapshot.month.total_tokens),
            thousands(snapshot.month.events),
        )),
        InfoLine::Blank,
        InfoLine::Head("Today".to_string()),
        InfoLine::Body(format!(
            "  {} tokens  ·  {} events",
            human_tokens(snapshot.today.total_tokens),
            thousands(snapshot.today.events),
        )),
        InfoLine::Blank,
        InfoLine::Head("Top models".to_string()),
    ];
    for model in &snapshot.top_models {
        lines.push(InfoLine::Body(format!(
            "  {:<26}{:>8}",
            model.model,
            human_tokens(model.total_tokens),
        )));
    }
    lines.push(InfoLine::Blank);
    lines.push(InfoLine::Body(
        "skopos usage by-model · today · month".to_string(),
    ));
    lines
}

fn empty_info_lines() -> Vec<InfoLine> {
    vec![
        InfoLine::Title("Skopos".to_string()),
        InfoLine::Body("local-first AI usage observability".to_string()),
        InfoLine::Blank,
        InfoLine::Head("No usage imported yet".to_string()),
        InfoLine::Blank,
        InfoLine::Head("Get started".to_string()),
        InfoLine::Body("  skopos claude import".to_string()),
        InfoLine::Body("  skopos usage by-model".to_string()),
        InfoLine::Blank,
        InfoLine::Head("Data".to_string()),
        InfoLine::Body("  ~/.local/share/skopos/skopos.db".to_string()),
    ]
}

fn orange_gradient_line(line: &str, index: usize, total_lines: usize) -> String {
    let denominator = total_lines.saturating_sub(1).max(1) as f32;
    let t = index as f32 / denominator;
    let start = (255.0, 214.0, 153.0);
    let end = (204.0, 74.0, 4.0);
    let r = lerp(start.0, end.0, t).round() as u8;
    let g = lerp(start.1, end.1, t).round() as u8;
    let b = lerp(start.2, end.2, t).round() as u8;
    rgb_text(line, r, g, b)
}

fn rgb_text(text: &str, r: u8, g: u8, b: u8) -> String {
    format!("\x1b[38;2;{r};{g};{b}m{text}\x1b[0m")
}

/// Bright-orange foreground text.
fn orange(text: &str) -> String {
    rgb_text(text, ORANGE.0, ORANGE.1, ORANGE.2)
}

/// Bold bright-orange foreground text.
fn orange_bold(text: &str) -> String {
    format!(
        "\x1b[1m\x1b[38;2;{};{};{}m{text}\x1b[0m",
        ORANGE.0, ORANGE.1, ORANGE.2
    )
}

/// Dimmed grey foreground text.
fn dim(text: &str) -> String {
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
    out.push_str(&orange(&format_row(&header_cells)));
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

/// A glance at recent usage, used to populate the splash side panel.
struct UsageSnapshot {
    month: skopos_store::UsageTotals,
    today: skopos_store::UsageTotals,
    top_models: Vec<skopos_store::UsageModelTotal>,
}

/// Read a usage snapshot for the splash. Returns `None` when there is no
/// database yet or it holds no events, so the splash never creates state.
async fn usage_snapshot(db_path: &std::path::Path) -> Option<UsageSnapshot> {
    if !db_path.exists() {
        return None;
    }
    let store = SkoposStore::connect_path(db_path).await.ok()?;
    let now = Utc::now();
    let (month_start, month_end) = month_range(now);
    let (today_start, today_end) = today_range(now);
    let month = store
        .usage_totals_between(month_start, month_end)
        .await
        .ok()?;
    let today = store
        .usage_totals_between(today_start, today_end)
        .await
        .ok()?;
    let mut top_models = store.usage_totals_by_model().await.ok()?;
    top_models.truncate(3);

    if month.events == 0 && top_models.is_empty() {
        return None;
    }

    Some(UsageSnapshot {
        month,
        today,
        top_models,
    })
}

fn today_range(now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let start = Utc
        .with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
        .unwrap();
    (start, start + chrono::Duration::days(1))
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

async fn usage_by_model_report(db_path: impl Into<PathBuf>) -> anyhow::Result<String> {
    let store = SkoposStore::connect_path(db_path.into()).await?;
    store.migrate().await?;
    let totals = store.usage_totals_by_model().await?;

    let mut report = String::new();
    report.push_str(&orange_bold("Usage by model"));
    report.push_str("\n\n");

    if totals.is_empty() {
        report.push_str(&dim(
            "  No usage imported yet — run: skopos claude import\n",
        ));
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

async fn usage_period_report(
    db_path: impl Into<PathBuf>,
    period: UsagePeriod,
) -> anyhow::Result<String> {
    let now = Utc::now();
    let (label, start, end) = match period {
        UsagePeriod::Today => {
            let (start, end) = today_range(now);
            ("today", start, end)
        }
        UsagePeriod::Month => {
            let (start, end) = month_range(now);
            ("month", start, end)
        }
    };

    let store = SkoposStore::connect_path(db_path.into()).await?;
    store.migrate().await?;
    let totals = store.usage_totals_between(start, end).await?;

    let mut report = String::new();
    report.push_str(&orange_bold(&format!("Usage {label}")));
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
            orange(&format!("{label:<8}")),
            value,
        ));
    }

    Ok(report)
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
}
