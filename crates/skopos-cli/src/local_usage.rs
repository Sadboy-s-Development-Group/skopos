//! Local view of Claude Code usage from `~/.claude/projects/**/*.jsonl`.
//!
//! Anthropic does not expose 5h/7d quota % to third-party tools (and we
//! decided not to scrape the OAuth-only endpoint — see commit message and
//! the project memory for the legal reasoning). What we *can* do without
//! touching anyone's API is sum the tokens recorded in our own local
//! transcripts and surface the absolute counts per window.
//!
//! The bars look quota-shaped but they only show input vs. total within
//! each window; we never invent a denominator we don't have.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};

use skopos_collectors::claude_code::{
    discover_claude_code_jsonl_paths, parse_usage_events_from_jsonl_path,
};
use skopos_core::UsageEvent;

use crate::format::{human_tokens, thousands};
use crate::theme::{dim, purple, purple_bold};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct TokenTotals {
    pub events: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct LocalUsage {
    pub last_5h: TokenTotals,
    pub last_7d: TokenTotals,
    /// Scanned JSONL files (after mtime pruning), for telemetry only.
    pub files_scanned: u64,
}

/// Aggregate tokens across the 5h and 7d windows ending at `now`. Files
/// last-modified before the 7d window are skipped entirely.
pub(crate) fn aggregate(claude_home: &Path, now: DateTime<Utc>) -> anyhow::Result<LocalUsage> {
    let paths = discover_claude_code_jsonl_paths(claude_home)?;
    let seven_days_ago = now - Duration::days(7);
    let five_hours_ago = now - Duration::hours(5);

    let mut usage = LocalUsage::default();
    for path in paths {
        if file_mtime(&path)
            .map(|mtime| mtime < seven_days_ago)
            .unwrap_or(false)
        {
            continue;
        }
        usage.files_scanned += 1;
        for event in parse_usage_events_from_jsonl_path(&path)? {
            if event.timestamp < seven_days_ago {
                continue;
            }
            add(&mut usage.last_7d, &event);
            if event.timestamp >= five_hours_ago {
                add(&mut usage.last_5h, &event);
            }
        }
    }
    Ok(usage)
}

fn add(totals: &mut TokenTotals, event: &UsageEvent) {
    totals.events += 1;
    totals.input_tokens += event.input_tokens;
    totals.cached_input_tokens += event.cached_input_tokens.unwrap_or(0);
    totals.output_tokens += event.output_tokens;
    totals.total_tokens += event.total_tokens;
}

fn file_mtime(path: &PathBuf) -> Option<DateTime<Utc>> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let secs = modified
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    DateTime::from_timestamp(secs, 0)
}

// ===========================================================================
// Rendering
// ===========================================================================

/// Render the "Local Activity" block.
pub(crate) fn render_local_block(usage: &LocalUsage) -> String {
    let mut out = String::new();
    out.push_str(&purple_bold("  Local Activity"));
    out.push_str(&dim(&format!(
        "  ({} JSONL file{} scanned)\n",
        usage.files_scanned,
        if usage.files_scanned == 1 { "" } else { "s" }
    )));

    if usage.last_7d.events == 0 {
        out.push_str(&dim("    no Claude Code events in the last 7 days.\n"));
        return out;
    }

    out.push_str(&row("last 5h", &usage.last_5h));
    out.push_str(&row("last 7d", &usage.last_7d));
    out.push_str(&dim(
        "    note: absolute token counts from local JSONL — not the same units as the Limits % above.\n",
    ));
    out
}

fn row(label: &str, totals: &TokenTotals) -> String {
    if totals.events == 0 {
        return format!(
            "    {}  {}\n",
            purple(&format!("{label:<8}")),
            dim("no events"),
        );
    }
    let breakdown = format!(
        "{} inp · {} cached · {} out · {} events",
        human_tokens(totals.input_tokens as i64),
        human_tokens(totals.cached_input_tokens as i64),
        human_tokens(totals.output_tokens as i64),
        thousands(totals.events as i64),
    );
    format!(
        "    {}  {:>7} tokens   {}\n",
        purple(&format!("{label:<8}")),
        human_tokens(totals.total_tokens as i64),
        dim(&breakdown),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn temp_dir(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("skopos-local-usage-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_jsonl(dir: &Path, name: &str, events: &[(&str, &str, u64, u64, u64)]) -> PathBuf {
        let project_dir = dir.join("projects").join("test-project");
        fs::create_dir_all(&project_dir).unwrap();
        let path = project_dir.join(name);
        let mut body = String::new();
        for (timestamp, model, input, cached, output) in events {
            body.push_str(&format!(
                r#"{{"message":{{"model":"{model}","id":"m{timestamp}","role":"assistant","usage":{{"input_tokens":{input},"cache_read_input_tokens":{cached},"output_tokens":{output}}}}},"timestamp":"{timestamp}","cwd":"/x","sessionId":"s1"}}"#
            ));
            body.push('\n');
        }
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn aggregator_separates_5h_and_7d_windows() {
        let dir = temp_dir("windows");
        let now = ts("2026-05-17T10:00:00Z");
        write_jsonl(
            &dir,
            "session.jsonl",
            &[
                // inside the 5h window:
                ("2026-05-17T09:00:00Z", "claude-opus-4-7", 100, 200, 50),
                // inside 7d but outside 5h:
                ("2026-05-15T10:00:00Z", "claude-opus-4-7", 1000, 2000, 500),
                // outside 7d — must be ignored:
                ("2026-04-01T10:00:00Z", "claude-opus-4-7", 9999, 9999, 9999),
            ],
        );

        let usage = aggregate(&dir, now).unwrap();
        assert_eq!(usage.last_5h.events, 1);
        assert_eq!(usage.last_5h.input_tokens, 100);
        assert_eq!(usage.last_5h.cached_input_tokens, 200);
        assert_eq!(usage.last_5h.output_tokens, 50);
        // 7d window includes both recent events.
        assert_eq!(usage.last_7d.events, 2);
        assert_eq!(usage.last_7d.input_tokens, 1100);
        assert_eq!(usage.last_7d.cached_input_tokens, 2200);
        assert_eq!(usage.last_7d.output_tokens, 550);
    }

    #[test]
    fn aggregator_with_no_jsonl_files_returns_zero() {
        let dir = temp_dir("empty");
        fs::create_dir_all(dir.join("projects")).unwrap();
        let now = ts("2026-05-17T10:00:00Z");
        let usage = aggregate(&dir, now).unwrap();
        assert_eq!(usage.last_5h, TokenTotals::default());
        assert_eq!(usage.last_7d, TokenTotals::default());
        assert_eq!(usage.files_scanned, 0);
    }

    #[test]
    fn render_local_block_handles_empty_state() {
        let body = render_local_block(&LocalUsage::default());
        assert!(body.contains("no Claude Code events"));
    }

    #[test]
    fn render_local_block_lists_both_rows_when_events_exist() {
        let usage = LocalUsage {
            last_5h: TokenTotals {
                events: 12,
                input_tokens: 240_000,
                cached_input_tokens: 800_000,
                output_tokens: 160_000,
                total_tokens: 1_200_000,
            },
            last_7d: TokenTotals {
                events: 84,
                input_tokens: 1_800_000,
                cached_input_tokens: 5_500_000,
                output_tokens: 1_100_000,
                total_tokens: 8_400_000,
            },
            files_scanned: 3,
        };
        let body = render_local_block(&usage);
        assert!(body.contains("last 5h"));
        assert!(body.contains("last 7d"));
        assert!(body.contains("1.2M"));
        assert!(body.contains("8.4M"));
        assert!(body.contains("3 JSONL files"));
    }
}
