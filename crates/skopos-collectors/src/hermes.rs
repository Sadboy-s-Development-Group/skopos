//! Hermes agent collector.
//!
//! Hermes does not write per-turn JSONL transcripts. It owns a single
//! SQLite database at `~/.hermes/state.db`, and stores one *aggregated*
//! row per session in the `sessions` table:
//!
//! ```sql
//! CREATE TABLE sessions (
//!     id TEXT PRIMARY KEY,
//!     source TEXT, model TEXT, billing_provider TEXT,
//!     started_at REAL, ended_at REAL,
//!     input_tokens INTEGER, output_tokens INTEGER,
//!     cache_read_tokens INTEGER, cache_write_tokens INTEGER,
//!     reasoning_tokens INTEGER,
//!     estimated_cost_usd REAL, actual_cost_usd REAL,
//!     ...
//! );
//! ```
//!
//! The row is rewritten in place while a session is alive — token
//! counts only grow, never shrink. We surface both open and closed
//! sessions; correctness of repeated imports comes from the store's
//! `upsert_usage_event_by_dedupe_key`, which only widens the persisted
//! totals when the new read is strictly larger. The dedupe key
//! (`hermes:session:<id>`) is stable across the lifetime of the session
//! so each subsequent import refreshes the same row.
//!
//! Every event is recorded under the synthetic provider `"hermes"` so it
//! never collides with native Anthropic / OpenAI / Google traffic. The
//! underlying model name is preserved in `model`, and the billing route
//! Hermes used (`openai-codex`, `copilot`, …) goes into `metadata` so we
//! can break the rollup down later without re-parsing the DB.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use skopos_core::{ModelId, ProviderId, UsageEvent, UsageSource, UsageSourceKind};
use std::path::{Path, PathBuf};

/// Filename Hermes stores its state under, inside the home directory.
const STATE_DB_FILENAME: &str = "state.db";

/// Provider value used in `usage_events.provider` for all Hermes rows.
pub const HERMES_PROVIDER: &str = "hermes";

/// Source app label embedded in every event's `source.app`.
const HERMES_SOURCE_APP: &str = "hermes";

/// One row read out of Hermes's `sessions` table. Lives in this module
/// only — it never escapes; we convert straight to [`UsageEvent`].
struct SessionRow {
    id: String,
    source: Option<String>,
    model: Option<String>,
    billing_provider: Option<String>,
    started_at: f64,
    /// `None` while the session is still open in Hermes; we still emit
    /// an event so live activity shows up in rollups, but we keep the
    /// metadata honest about whether the session has closed.
    ended_at: Option<f64>,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    reasoning_tokens: u64,
    estimated_cost_usd: Option<f64>,
    actual_cost_usd: Option<f64>,
}

/// Discover the Hermes state database under `hermes_home`. Returns
/// `[home/state.db]` when the file exists, an empty vec otherwise. The
/// shape mirrors the JSONL discovery helpers so [`Agent`] can dispatch
/// uniformly.
pub fn discover_hermes_state_db_paths(hermes_home: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
    let candidate = hermes_home.as_ref().join(STATE_DB_FILENAME);
    if candidate.is_file() {
        Ok(vec![candidate])
    } else {
        Ok(Vec::new())
    }
}

/// Parse every non-empty session (open or closed) out of a Hermes
/// `state.db` into `UsageEvent`s. The connection is opened read-only
/// with shared cache disabled, so a live Hermes process can keep
/// writing without contention. Empty rows (no tokens yet) are skipped.
pub fn parse_usage_events_from_hermes_db(path: impl AsRef<Path>) -> Result<Vec<UsageEvent>> {
    let path = path.as_ref();
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open Hermes state DB read-only: {}", path.display()))?;

    // The session row is the unit of usage in Hermes. We skip rows
    // that have not produced any tokens yet (newly opened sessions that
    // have not made an API call) since they would translate into empty
    // events. Live sessions with tokens are imported as well; the
    // store's monotonic upsert keeps the picture coherent on re-runs.
    // `ended_at` is left nullable — it becomes `None` for open sessions
    // so the metadata does not get a phantom epoch-zero timestamp.
    let mut stmt = conn
        .prepare(
            "SELECT id,
                    source,
                    model,
                    billing_provider,
                    started_at,
                    ended_at,
                    COALESCE(input_tokens, 0),
                    COALESCE(output_tokens, 0),
                    COALESCE(cache_read_tokens, 0),
                    COALESCE(cache_write_tokens, 0),
                    COALESCE(reasoning_tokens, 0),
                    estimated_cost_usd,
                    actual_cost_usd
               FROM sessions
              WHERE (COALESCE(input_tokens, 0)
                   + COALESCE(output_tokens, 0)
                   + COALESCE(cache_read_tokens, 0)
                   + COALESCE(cache_write_tokens, 0)
                   + COALESCE(reasoning_tokens, 0)) > 0
              ORDER BY started_at",
        )
        .context("prepare Hermes sessions query")?;

    let rows = stmt
        .query_map([], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                source: row.get(1)?,
                model: row.get(2)?,
                billing_provider: row.get(3)?,
                started_at: row.get(4)?,
                ended_at: row.get(5)?,
                input_tokens: row.get::<_, i64>(6)?.max(0) as u64,
                output_tokens: row.get::<_, i64>(7)?.max(0) as u64,
                cache_read_tokens: row.get::<_, i64>(8)?.max(0) as u64,
                cache_write_tokens: row.get::<_, i64>(9)?.max(0) as u64,
                reasoning_tokens: row.get::<_, i64>(10)?.max(0) as u64,
                estimated_cost_usd: row.get(11)?,
                actual_cost_usd: row.get(12)?,
            })
        })
        .context("execute Hermes sessions query")?;

    let mut events = Vec::new();
    for row in rows {
        let row = row.context("read Hermes session row")?;
        events.push(session_row_into_event(row));
    }
    Ok(events)
}

fn session_row_into_event(row: SessionRow) -> UsageEvent {
    let model = row.model.as_deref().unwrap_or("unknown");

    let mut event = UsageEvent::new(
        ProviderId::new(HERMES_PROVIDER),
        ModelId::new(model),
        row.input_tokens,
        row.output_tokens,
        UsageSource {
            app: HERMES_SOURCE_APP.to_string(),
            kind: UsageSourceKind::Log,
        },
    );

    event.cached_input_tokens = (row.cache_read_tokens > 0).then_some(row.cache_read_tokens);
    event.reasoning_tokens = (row.reasoning_tokens > 0).then_some(row.reasoning_tokens);
    event.total_tokens = row
        .input_tokens
        .saturating_add(row.output_tokens)
        .saturating_add(row.cache_read_tokens)
        .saturating_add(row.cache_write_tokens)
        .saturating_add(row.reasoning_tokens);
    event.timestamp = timestamp_from_unix_secs(row.started_at).unwrap_or_else(Utc::now);
    event.session_id = Some(row.id);
    event.metadata = build_metadata(
        row.source.as_deref(),
        row.billing_provider.as_deref(),
        row.cache_write_tokens,
        row.ended_at,
        row.estimated_cost_usd,
        row.actual_cost_usd,
    );
    event
}

/// Convert a Unix epoch float (Python `time.time()` style — seconds with
/// fractional nanoseconds) into UTC. Returns `None` for NaN/out-of-range
/// inputs so the caller can fall back to `Utc::now()` rather than panic.
fn timestamp_from_unix_secs(seconds: f64) -> Option<DateTime<Utc>> {
    if !seconds.is_finite() {
        return None;
    }
    let whole_secs = seconds.trunc() as i64;
    let nanos = ((seconds - seconds.trunc()) * 1_000_000_000.0).round();
    let nanos = if (0.0..1_000_000_000.0).contains(&nanos) {
        nanos as u32
    } else {
        0
    };
    DateTime::from_timestamp(whole_secs, nanos)
}

fn build_metadata(
    source: Option<&str>,
    billing_provider: Option<&str>,
    cache_write_tokens: u64,
    ended_at: Option<f64>,
    estimated_cost_usd: Option<f64>,
    actual_cost_usd: Option<f64>,
) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    if let Some(value) = source {
        map.insert("hermes_source".into(), value.into());
    }
    if let Some(value) = billing_provider {
        map.insert("hermes_billing_provider".into(), value.into());
    }
    if cache_write_tokens > 0 {
        map.insert(
            "hermes_cache_write_tokens".into(),
            cache_write_tokens.into(),
        );
    }
    // For open sessions we mark the row explicitly so downstream code
    // can tell "this number may still grow" from "this is the final tally".
    match ended_at.and_then(timestamp_from_unix_secs) {
        Some(ts) => {
            map.insert("hermes_ended_at".into(), ts.to_rfc3339().into());
        }
        None => {
            map.insert("hermes_session_open".into(), true.into());
        }
    }
    if let Some(value) = estimated_cost_usd {
        map.insert("hermes_estimated_cost_usd".into(), value.into());
    }
    if let Some(value) = actual_cost_usd {
        map.insert("hermes_actual_cost_usd".into(), value.into());
    }
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Build an isolated Hermes home with a `state.db` whose schema
    /// matches the columns the parser reads. We only seed what we need;
    /// the real DB has many more columns but we never SELECT them.
    fn fresh_hermes_home() -> PathBuf {
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "skopos-hermes-test-{}-{}",
            std::process::id(),
            unique
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join(STATE_DB_FILENAME);
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                 id TEXT PRIMARY KEY,
                 source TEXT,
                 model TEXT,
                 billing_provider TEXT,
                 started_at REAL,
                 ended_at REAL,
                 input_tokens INTEGER,
                 output_tokens INTEGER,
                 cache_read_tokens INTEGER,
                 cache_write_tokens INTEGER,
                 reasoning_tokens INTEGER,
                 estimated_cost_usd REAL,
                 actual_cost_usd REAL
             );",
        )
        .unwrap();
        dir
    }

    fn insert_session(
        home: &Path,
        id: &str,
        ended_at: Option<f64>,
        model: &str,
        billing_provider: Option<&str>,
        tokens: (u64, u64, u64, u64, u64),
    ) {
        let conn = Connection::open(home.join(STATE_DB_FILENAME)).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, source, model, billing_provider, started_at, ended_at,
                                   input_tokens, output_tokens, cache_read_tokens,
                                   cache_write_tokens, reasoning_tokens,
                                   estimated_cost_usd, actual_cost_usd)
             VALUES (?, 'cli', ?, ?, 1_700_000_000.5, ?, ?, ?, ?, ?, ?, NULL, NULL)",
            params![
                id,
                model,
                billing_provider,
                ended_at,
                tokens.0 as i64,
                tokens.1 as i64,
                tokens.2 as i64,
                tokens.3 as i64,
                tokens.4 as i64,
            ],
        )
        .unwrap();
    }

    #[test]
    fn discover_returns_state_db_when_present() {
        let home = fresh_hermes_home();
        let paths = discover_hermes_state_db_paths(&home).unwrap();
        assert_eq!(paths, vec![home.join(STATE_DB_FILENAME)]);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn discover_returns_empty_when_state_db_missing() {
        let dir = std::env::temp_dir().join(format!(
            "skopos-hermes-empty-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = discover_hermes_state_db_paths(&dir).unwrap();
        assert!(paths.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_open_and_closed_sessions_and_skips_only_empty_ones() {
        let home = fresh_hermes_home();
        // Closed session with tokens: surfaced, with `hermes_ended_at` set.
        insert_session(
            &home,
            "closed-with-tokens",
            Some(1_700_000_100.0),
            "gpt-5.5",
            Some("openai-codex"),
            (10, 20, 30, 0, 5),
        );
        // Open session (no ended_at) with tokens: also surfaced now —
        // the store's monotonic upsert keeps the row coherent across
        // re-imports as the session grows. `hermes_session_open` flag
        // distinguishes it from a final close.
        insert_session(
            &home,
            "still-open",
            None,
            "gpt-5.5",
            Some("openai-codex"),
            (1, 2, 3, 0, 0),
        );
        // Closed but no tokens recorded: skipped — there is nothing to
        // account for and emitting a zero-token event would just bloat
        // rollups with noise.
        insert_session(
            &home,
            "closed-but-empty",
            Some(1_700_000_200.0),
            "gpt-5.5",
            None,
            (0, 0, 0, 0, 0),
        );

        let events = parse_usage_events_from_hermes_db(home.join(STATE_DB_FILENAME)).unwrap();
        assert_eq!(events.len(), 2);

        let closed = events
            .iter()
            .find(|event| event.session_id.as_deref() == Some("closed-with-tokens"))
            .expect("closed session must be present");
        assert_eq!(closed.provider.0, HERMES_PROVIDER);
        assert_eq!(closed.model.0, "gpt-5.5");
        assert_eq!(closed.input_tokens, 10);
        assert_eq!(closed.output_tokens, 20);
        assert_eq!(closed.cached_input_tokens, Some(30));
        assert_eq!(closed.reasoning_tokens, Some(5));
        // input + output + cache_read + cache_write(=0) + reasoning
        assert_eq!(closed.total_tokens, 10 + 20 + 30 + 5);
        assert_eq!(closed.source.app, HERMES_SOURCE_APP);
        assert!(closed.metadata.get("hermes_ended_at").is_some());
        assert!(closed.metadata.get("hermes_session_open").is_none());

        let open = events
            .iter()
            .find(|event| event.session_id.as_deref() == Some("still-open"))
            .expect("open session must now be present");
        assert_eq!(open.total_tokens, 1 + 2 + 3);
        assert!(open.metadata.get("hermes_ended_at").is_none());
        assert_eq!(
            open.metadata
                .get("hermes_session_open")
                .and_then(|v| v.as_bool()),
            Some(true),
        );

        assert_eq!(
            closed
                .metadata
                .get("hermes_billing_provider")
                .and_then(|v| v.as_str()),
            Some("openai-codex"),
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn cache_write_tokens_surface_through_metadata_when_nonzero() {
        let home = fresh_hermes_home();
        insert_session(
            &home,
            "with-cache-writes",
            Some(1_700_000_500.0),
            "claude-haiku-4.5",
            Some("copilot"),
            (5, 7, 0, 42, 0),
        );

        let events = parse_usage_events_from_hermes_db(home.join(STATE_DB_FILENAME)).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0]
                .metadata
                .get("hermes_cache_write_tokens")
                .and_then(|v| v.as_u64()),
            Some(42),
        );
        assert_eq!(events[0].cached_input_tokens, None);
        assert_eq!(events[0].total_tokens, 5 + 7 + 42);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn timestamp_from_unix_secs_handles_fraction_and_bad_input() {
        // Whole + fractional seconds round-trip into UTC.
        let ts = timestamp_from_unix_secs(1_700_000_000.5).unwrap();
        assert_eq!(ts.timestamp(), 1_700_000_000);
        assert_eq!(ts.timestamp_subsec_nanos(), 500_000_000);

        // NaN/infinity give None so the caller can fall back.
        assert!(timestamp_from_unix_secs(f64::NAN).is_none());
        assert!(timestamp_from_unix_secs(f64::INFINITY).is_none());
    }
}
