//! The agentic CLIs Skopos imports token logs from.
//!
//! Claude Code, Codex and Gemini each store their transcripts in a
//! different place, in a different shape, and need a different stable
//! dedupe key. [`Agent`] is the one place that knowledge lives; the
//! scan / import / auto-import orchestration below is written once and
//! dispatched over the enum, instead of being copy-pasted per provider.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use skopos_collectors::{claude_code, codex, gemini};
use skopos_core::UsageEvent;
use skopos_store::SkoposStore;

use crate::limits::home_dir;
use crate::theme::dim;

/// One agentic CLI whose local logs Skopos can scan and import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Agent {
    Claude,
    Codex,
    Gemini,
}

impl Agent {
    /// Resolve the `provider` column value used in the store back to an
    /// agent. Returns `None` for providers Skopos cannot import from.
    pub(crate) fn from_provider(provider: &str) -> Option<Agent> {
        match provider {
            "anthropic" => Some(Agent::Claude),
            "openai" => Some(Agent::Codex),
            "google" => Some(Agent::Gemini),
            _ => None,
        }
    }

    /// The `provider` column value this agent's events are stored under.
    pub(crate) fn provider(self) -> &'static str {
        match self {
            Agent::Claude => "anthropic",
            Agent::Codex => "openai",
            Agent::Gemini => "google",
        }
    }

    /// Lower-case CLI name, used in user-facing messages.
    fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Gemini => "gemini",
        }
    }

    /// Heading printed above a `scan` summary.
    fn scan_label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code scan",
            Agent::Codex => "Codex scan",
            Agent::Gemini => "Gemini scan",
        }
    }

    /// Heading printed above an `import` report.
    fn import_label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code import",
            Agent::Codex => "Codex import",
            Agent::Gemini => "Gemini import",
        }
    }

    /// Default home directory the agent writes its logs under.
    fn default_home(self) -> PathBuf {
        let dir = match self {
            Agent::Claude => ".claude",
            Agent::Codex => ".codex",
            Agent::Gemini => ".gemini",
        };
        home_dir().join(dir)
    }

    /// Subdirectory whose JSONL mtimes signal a fresh import is worthwhile.
    /// `None` for agents that are only ever imported on explicit request.
    fn staleness_root(self, home: &Path) -> Option<PathBuf> {
        match self {
            Agent::Claude => None,
            Agent::Codex => Some(home.join("sessions")),
            Agent::Gemini => Some(home.join("tmp")),
        }
    }

    /// Discover the agent's log files under `home`.
    fn discover(self, home: &Path) -> anyhow::Result<Vec<PathBuf>> {
        match self {
            Agent::Claude => claude_code::discover_claude_code_jsonl_paths(home),
            Agent::Codex => codex::discover_codex_rollout_paths(home),
            Agent::Gemini => gemini::discover_gemini_session_paths(home),
        }
    }

    /// Parse one of the agent's log files into usage events.
    fn parse(self, path: &Path) -> anyhow::Result<Vec<UsageEvent>> {
        match self {
            Agent::Claude => claude_code::parse_usage_events_from_jsonl_path(path),
            Agent::Codex => codex::parse_usage_events_from_rollout_path(path),
            Agent::Gemini => gemini::parse_usage_events_from_session_path(path),
        }
    }

    /// Derive a stable dedupe key for one event.
    fn dedupe_key(self, event: &UsageEvent) -> String {
        match self {
            Agent::Claude => claude_dedupe_key(event),
            Agent::Codex => codex_dedupe_key(event),
            Agent::Gemini => gemini_dedupe_key(event),
        }
    }
}

/// Outcome of importing one agent's logs into the store.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ImportReport {
    pub files: u64,
    pub seen_events: u64,
    pub inserted_events: u64,
    pub duplicate_events: u64,
}

#[derive(Debug, Default)]
struct ModelUsageSummary {
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
}

// ===========================================================================
// Scan — summarize logs without writing SQLite
// ===========================================================================

/// Scan an agent's logs and return a per-model token summary. `home`
/// overrides the agent's default home directory when `Some`.
pub(crate) fn scan(agent: Agent, home: Option<PathBuf>) -> anyhow::Result<String> {
    let home = home.unwrap_or_else(|| agent.default_home());
    let paths = agent.discover(&home)?;
    let mut model_totals: BTreeMap<String, ModelUsageSummary> = BTreeMap::new();
    let mut event_count = 0u64;

    for path in &paths {
        for event in agent.parse(path)? {
            event_count += 1;
            let summary = model_totals.entry(event.model.0).or_default();
            summary.input_tokens += event.input_tokens;
            summary.cached_input_tokens += event.cached_input_tokens.unwrap_or(0);
            summary.output_tokens += event.output_tokens;
            summary.total_tokens += event.total_tokens;
        }
    }

    let mut out = String::new();
    out.push_str(&format!("{}\n", agent.scan_label()));
    out.push_str(&format!("home:   {}\n", home.display()));
    out.push_str(&format!("files:  {}\n", paths.len()));
    out.push_str(&format!("events: {event_count}\n"));

    if model_totals.is_empty() {
        out.push_str("models: none found\n");
        return Ok(out);
    }

    out.push_str("models:\n");
    for (model, summary) in model_totals {
        out.push_str(&format!(
            "  {model}: total={} input={} cached_input={} output={}\n",
            summary.total_tokens,
            summary.input_tokens,
            summary.cached_input_tokens,
            summary.output_tokens,
        ));
    }
    Ok(out)
}

// ===========================================================================
// Import — persist logs into SQLite
// ===========================================================================

/// Import an agent's logs under `home` into `db_path`, deduplicating on
/// the agent's stable key.
pub(crate) async fn import_from_home(
    agent: Agent,
    home: &Path,
    db_path: &Path,
) -> anyhow::Result<ImportReport> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let store = SkoposStore::connect_path(db_path).await?;
    store.migrate().await?;

    let paths = agent.discover(home)?;
    let mut report = ImportReport {
        files: paths.len() as u64,
        ..Default::default()
    };

    for path in paths {
        for event in agent.parse(&path)? {
            report.seen_events += 1;
            let key = agent.dedupe_key(&event);
            if store.insert_usage_event_once(&event, &key).await?.inserted {
                report.inserted_events += 1;
            } else {
                report.duplicate_events += 1;
            }
        }
    }

    Ok(report)
}

/// Run an import for one agent and return the formatted report block.
/// Shared by the `skopos <agent> import` subcommands and the REPL.
/// `home` overrides the agent's default home directory when `Some`.
pub(crate) async fn import_report(
    provider: &str,
    home: Option<PathBuf>,
    db_path: &Path,
) -> anyhow::Result<String> {
    let agent = Agent::from_provider(provider)
        .ok_or_else(|| anyhow::anyhow!("import: unknown provider {provider:?}"))?;
    let home = home.unwrap_or_else(|| agent.default_home());
    let r = import_from_home(agent, &home, db_path).await?;

    Ok(format!(
        "{label}\n\
         home:       {}\n\
         db:         {}\n\
         files:      {files}\n\
         seen:       {seen}\n\
         inserted:   {inserted}\n\
         duplicates: {duplicates}\n",
        home.display(),
        db_path.display(),
        label = agent.import_label(),
        files = r.files,
        seen = r.seen_events,
        inserted = r.inserted_events,
        duplicates = r.duplicate_events,
    ))
}

/// Idempotent best-effort import: if the agent's log directory holds
/// JSONL files newer than the latest event stored for it, run a full
/// import. The dedupe keys make a full sweep safe.
///
/// All errors are swallowed — this hook must never break the report that
/// triggered it. The caller still renders whatever the store has.
pub(crate) async fn auto_import_if_stale(agent: Agent, db_path: &Path) {
    if let Err(err) = try_auto_import(agent, db_path).await {
        eprintln!(
            "{}",
            dim(&format!("  ({} auto-import skipped: {err})", agent.name()))
        );
    }
}

async fn try_auto_import(agent: Agent, db_path: &Path) -> anyhow::Result<()> {
    use anyhow::Context;

    let home = agent.default_home();
    let Some(root) = agent.staleness_root(&home) else {
        return Ok(());
    };
    if !root.exists() {
        return Ok(());
    }

    let store = SkoposStore::connect_path(db_path)
        .await
        .context("connect skopos store")?;
    store.migrate().await.context("migrate skopos store")?;
    let last = store
        .latest_usage_event_timestamp_for_provider(agent.provider())
        .await
        .with_context(|| format!("read latest {} timestamp", agent.provider()))?;
    drop(store);

    if !jsonls_newer_than(&root, last)? {
        return Ok(());
    }

    let _ = import_from_home(agent, &home, db_path).await?;
    Ok(())
}

/// Walk `<root>/**/*.jsonl` looking for any file with `mtime > threshold`.
/// Returns `Ok(true)` on the first hit. If `threshold` is `None`, any
/// jsonl counts as "newer" (first-time import).
fn jsonls_newer_than(
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

// ===========================================================================
// Dedupe keys — one per agent, dispatched by Agent::dedupe_key
// ===========================================================================

fn claude_dedupe_key(event: &UsageEvent) -> String {
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

fn codex_dedupe_key(event: &UsageEvent) -> String {
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

fn gemini_dedupe_key(event: &UsageEvent) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

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
