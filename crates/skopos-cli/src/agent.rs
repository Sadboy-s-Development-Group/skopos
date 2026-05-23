//! The agentic CLIs Skopos imports token logs from.
//!
//! Claude Code, Codex, Gemini and Hermes each store their transcripts
//! in a different place, in a different shape, and need a different
//! stable dedupe key. [`Agent`] is the one place that knowledge lives;
//! the scan / import / auto-import orchestration below is written once
//! and dispatched over the enum, instead of being copy-pasted per
//! provider.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use skopos_collectors::{claude_code, codex, gemini, hermes};
use skopos_core::UsageEvent;
use skopos_store::{SkoposStore, UpsertOutcome};

use crate::limits::home_dir;
use crate::theme::dim;

/// One agentic CLI whose local logs Skopos can scan and import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Agent {
    Claude,
    Codex,
    Gemini,
    Hermes,
}

/// How the import loop should reconcile a parsed event with whatever
/// is already in the store under the same dedupe key.
///
/// `InsertOnce` — Claude Code, Codex and Gemini all write *append-only*
///     transcripts: once a log line exists, its token counts never
///     change. Re-importing the same line should be a no-op (the
///     `INSERT OR IGNORE` path).
/// `UpsertByDedupeKey` — Hermes rewrites a single session row in place
///     while the session is alive, so the same dedupe key may legally
///     come back with larger totals. The store's monotonic upsert
///     guarantees we only ever grow, never shrink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InsertStrategy {
    InsertOnce,
    UpsertByDedupeKey,
}

/// How [`try_auto_import`] decides "is there new agent activity since
/// the last imported event?". The shape is per-agent because different
/// CLIs store their logs in fundamentally different layouts: a tree of
/// JSONL files (Codex, Gemini) or a single SQLite database (Hermes).
enum StalenessSignal {
    /// Agent is never auto-imported; only on explicit `skopos <agent> import`.
    None,
    /// Walk `<root>/**/*.jsonl` and trip if any file's mtime > threshold.
    JsonlTree(PathBuf),
    /// Trip if this single file's mtime > threshold.
    SingleFile(PathBuf),
}

impl Agent {
    /// Resolve the `provider` column value used in the store back to an
    /// agent. Returns `None` for providers Skopos cannot import from.
    pub(crate) fn from_provider(provider: &str) -> Option<Agent> {
        match provider {
            "anthropic" => Some(Agent::Claude),
            "openai" => Some(Agent::Codex),
            "google" => Some(Agent::Gemini),
            "hermes" => Some(Agent::Hermes),
            _ => None,
        }
    }

    /// The `provider` column value this agent's events are stored under.
    pub(crate) fn provider(self) -> &'static str {
        match self {
            Agent::Claude => "anthropic",
            Agent::Codex => "openai",
            Agent::Gemini => "google",
            Agent::Hermes => hermes::HERMES_PROVIDER,
        }
    }

    /// Lower-case CLI name, used in user-facing messages.
    fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Gemini => "gemini",
            Agent::Hermes => "hermes",
        }
    }

    /// Heading printed above a `scan` summary.
    fn scan_label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code scan",
            Agent::Codex => "Codex scan",
            Agent::Gemini => "Gemini scan",
            Agent::Hermes => "Hermes scan",
        }
    }

    /// Heading printed above an `import` report.
    fn import_label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code import",
            Agent::Codex => "Codex import",
            Agent::Gemini => "Gemini import",
            Agent::Hermes => "Hermes import",
        }
    }

    /// Default home directory the agent writes its logs under.
    fn default_home(self) -> PathBuf {
        let dir = match self {
            Agent::Claude => ".claude",
            Agent::Codex => ".codex",
            Agent::Gemini => ".gemini",
            Agent::Hermes => ".hermes",
        };
        home_dir().join(dir)
    }

    /// What [`try_auto_import`] should look at to decide whether the
    /// agent has fresh activity since the last stored event.
    fn staleness_signal(self, home: &Path) -> StalenessSignal {
        match self {
            Agent::Claude => StalenessSignal::None,
            Agent::Codex => StalenessSignal::JsonlTree(home.join("sessions")),
            Agent::Gemini => StalenessSignal::JsonlTree(home.join("tmp")),
            Agent::Hermes => StalenessSignal::SingleFile(home.join("state.db")),
        }
    }

    /// How the import loop should treat repeated dedupe keys. Hermes
    /// needs upsert because its session rows mutate in place; every
    /// other agent's logs are append-only and insert-once is correct.
    fn insert_strategy(self) -> InsertStrategy {
        match self {
            Agent::Claude | Agent::Codex | Agent::Gemini => InsertStrategy::InsertOnce,
            Agent::Hermes => InsertStrategy::UpsertByDedupeKey,
        }
    }

    /// Discover the agent's log files under `home`.
    fn discover(self, home: &Path) -> anyhow::Result<Vec<PathBuf>> {
        match self {
            Agent::Claude => claude_code::discover_claude_code_jsonl_paths(home),
            Agent::Codex => codex::discover_codex_rollout_paths(home),
            Agent::Gemini => gemini::discover_gemini_session_paths(home),
            Agent::Hermes => hermes::discover_hermes_state_db_paths(home),
        }
    }

    /// Parse one of the agent's log files into usage events.
    fn parse(self, path: &Path) -> anyhow::Result<Vec<UsageEvent>> {
        match self {
            Agent::Claude => claude_code::parse_usage_events_from_jsonl_path(path),
            Agent::Codex => codex::parse_usage_events_from_rollout_path(path),
            Agent::Gemini => gemini::parse_usage_events_from_session_path(path),
            Agent::Hermes => hermes::parse_usage_events_from_hermes_db(path),
        }
    }

    /// Derive a stable dedupe key for one event.
    fn dedupe_key(self, event: &UsageEvent) -> String {
        match self {
            Agent::Claude => claude_dedupe_key(event),
            Agent::Codex => codex_dedupe_key(event),
            Agent::Gemini => gemini_dedupe_key(event),
            Agent::Hermes => hermes_dedupe_key(event),
        }
    }
}

/// Outcome of importing one agent's logs into the store.
///
/// `inserted_events` — brand-new dedupe keys that produced fresh rows.
/// `updated_events` — Hermes-style: a row was already in the store and
///     its tokens were refreshed in place because the source grew. Stays
///     `0` for append-only agents.
/// `duplicate_events` — the dedupe key already existed and nothing
///     changed (for [`InsertStrategy::InsertOnce`] this means an exact
///     re-import; for [`InsertStrategy::UpsertByDedupeKey`] it means the
///     source has not grown since the last import).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ImportReport {
    pub files: u64,
    pub seen_events: u64,
    pub inserted_events: u64,
    pub updated_events: u64,
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

    let strategy = agent.insert_strategy();
    for path in paths {
        for event in agent.parse(&path)? {
            report.seen_events += 1;
            let key = agent.dedupe_key(&event);
            match strategy {
                InsertStrategy::InsertOnce => {
                    if store.insert_usage_event_once(&event, &key).await?.inserted {
                        report.inserted_events += 1;
                    } else {
                        report.duplicate_events += 1;
                    }
                }
                InsertStrategy::UpsertByDedupeKey => {
                    match store.upsert_usage_event_by_dedupe_key(&event, &key).await? {
                        UpsertOutcome::Inserted => report.inserted_events += 1,
                        UpsertOutcome::Updated => report.updated_events += 1,
                        UpsertOutcome::Unchanged => report.duplicate_events += 1,
                    }
                }
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
         updated:    {updated}\n\
         duplicates: {duplicates}\n",
        home.display(),
        db_path.display(),
        label = agent.import_label(),
        files = r.files,
        seen = r.seen_events,
        inserted = r.inserted_events,
        updated = r.updated_events,
        duplicates = r.duplicate_events,
    ))
}

/// Idempotent best-effort import: if the agent's local logs hold
/// activity newer than the latest event stored for it, run a full
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
    let signal = agent.staleness_signal(&home);
    if matches!(signal, StalenessSignal::None) {
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

    let is_stale = match signal {
        StalenessSignal::None => return Ok(()),
        StalenessSignal::JsonlTree(root) => {
            if !root.exists() {
                return Ok(());
            }
            jsonls_newer_than(&root, last)?
        }
        StalenessSignal::SingleFile(path) => {
            if !path.exists() {
                return Ok(());
            }
            file_newer_than(&path, last)?
        }
    };
    if !is_stale {
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
    let threshold_st = threshold.map(systemtime_at_epoch);
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

/// Return `Ok(true)` if `path`'s mtime is strictly newer than `threshold`,
/// or if `threshold` is `None` (first-time import). Hermes uses this to
/// gate on its single `state.db`.
fn file_newer_than(path: &Path, threshold: Option<DateTime<Utc>>) -> anyhow::Result<bool> {
    let mtime = std::fs::metadata(path)?.modified()?;
    Ok(match threshold {
        None => true,
        Some(t) => mtime > systemtime_at_epoch(t),
    })
}

fn systemtime_at_epoch(t: DateTime<Utc>) -> std::time::SystemTime {
    std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(t.timestamp().max(0) as u64)
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

/// Hermes stores one row per session that we only import once `ended_at`
/// is set — so the session id alone is a stable, final key.
fn hermes_dedupe_key(event: &UsageEvent) -> String {
    match &event.session_id {
        Some(session) => format!("hermes:session:{session}"),
        None => format!(
            "hermes:fallback:{}:{}:{}:{}:{}",
            event.timestamp.to_rfc3339(),
            event.model.0,
            event.input_tokens,
            event.cached_input_tokens.unwrap_or(0),
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

    #[test]
    fn file_newer_than_handles_threshold_and_pinned_mtime() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{Duration, SystemTime};

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let temp_dir = std::env::temp_dir().join(format!(
            "skopos-cli-file-newer-test-{}-{}",
            std::process::id(),
            unique,
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let target = temp_dir.join("state.db");
        std::fs::write(&target, b"\x00").unwrap();
        let pinned = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&target)
            .unwrap()
            .set_modified(pinned)
            .unwrap();

        // No threshold (first-time import) → always true.
        assert!(file_newer_than(&target, None).unwrap());

        // Threshold strictly newer than the file → false.
        let newer = Utc.timestamp_opt(1_700_000_000 + 60, 0).unwrap();
        assert!(!file_newer_than(&target, Some(newer)).unwrap());

        // Threshold strictly older than the file → true.
        let older = Utc.timestamp_opt(1_700_000_000 - 60, 0).unwrap();
        assert!(file_newer_than(&target, Some(older)).unwrap());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn hermes_dedupe_key_uses_session_id() {
        let mut event = UsageEvent::new(
            skopos_core::ProviderId::new("hermes"),
            skopos_core::ModelId::new("gpt-5.5"),
            10,
            20,
            skopos_core::UsageSource {
                app: "hermes".into(),
                kind: skopos_core::UsageSourceKind::Log,
            },
        );
        event.session_id = Some("20260520_181742_d9b5f5".into());
        assert_eq!(
            hermes_dedupe_key(&event),
            "hermes:session:20260520_181742_d9b5f5"
        );

        // Without session_id, falls back to a stable tuple. The format
        // is implementation-detail; we just check it does not panic and
        // depends on the model name.
        event.session_id = None;
        assert!(hermes_dedupe_key(&event).contains("hermes:fallback:"));
        assert!(hermes_dedupe_key(&event).contains("gpt-5.5"));
    }
}
