//! Persistence and rollups for `usage_events` — per-request token records
//! collected from each agent's local logs.

use chrono::{DateTime, Utc};
use skopos_core::{UsageEvent, UsageSourceKind};
use sqlx::{sqlite::SqliteRow, Row};

use crate::{parse_rfc3339, SkoposStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsertUsageResult {
    pub inserted: bool,
}

/// Outcome of [`SkoposStore::upsert_usage_event_by_dedupe_key`].
///
/// `Inserted` — the dedupe key was new, a fresh row landed.
/// `Updated` — the dedupe key existed and the new totals were strictly
///     larger, so the token/cost/metadata columns were refreshed in
///     place. Identity columns (`id`, `dedupe_key`, `timestamp`,
///     `provider`, `model`, source, project_path, session_id,
///     request_id) are never touched on conflict.
/// `Unchanged` — the dedupe key existed but the incoming `total_tokens`
///     was not strictly greater than what is already stored, so the
///     monotonic guard kept the existing row. This is the expected
///     outcome when re-importing a session that has not produced new
///     tokens since the last import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    Inserted,
    Updated,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageModelTotal {
    pub provider: String,
    pub model: String,
    pub events: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageTotals {
    pub events: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
}

impl SkoposStore {
    pub async fn insert_usage_event_once(
        &self,
        event: &UsageEvent,
        dedupe_key: &str,
    ) -> anyhow::Result<InsertUsageResult> {
        let metadata_json = serde_json::to_string(&event.metadata)?;
        let project_path = event
            .project_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string());
        let (estimated_cost_usd, currency) = event
            .estimated_cost
            .as_ref()
            .map(|money| (Some(money.amount), money.currency.as_str()))
            .unwrap_or((None, "USD"));

        let result = sqlx::query(
            r#"
            INSERT OR IGNORE INTO usage_events (
                id,
                dedupe_key,
                timestamp,
                provider,
                model,
                input_tokens,
                output_tokens,
                cached_input_tokens,
                reasoning_tokens,
                total_tokens,
                estimated_cost_usd,
                currency,
                source_app,
                source_type,
                project_path,
                session_id,
                request_id,
                metadata_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(event.id.to_string())
        .bind(dedupe_key)
        .bind(event.timestamp.to_rfc3339())
        .bind(&event.provider.0)
        .bind(&event.model.0)
        .bind(event.input_tokens as i64)
        .bind(event.output_tokens as i64)
        .bind(event.cached_input_tokens.map(|tokens| tokens as i64))
        .bind(event.reasoning_tokens.map(|tokens| tokens as i64))
        .bind(event.total_tokens as i64)
        .bind(estimated_cost_usd)
        .bind(currency)
        .bind(&event.source.app)
        .bind(source_kind_name(&event.source.kind))
        .bind(project_path)
        .bind(&event.session_id)
        .bind(&event.request_id)
        .bind(metadata_json)
        .execute(&self.pool)
        .await?;

        Ok(InsertUsageResult {
            inserted: result.rows_affected() == 1,
        })
    }

    /// Insert `event` keyed on `dedupe_key`, or update the existing row
    /// in place if the incoming `total_tokens` is strictly greater than
    /// what is stored. The monotonic guard makes this safe to call from
    /// agents whose source data mutates between snapshots — notably
    /// Hermes, where a single `sessions` row is rewritten in place while
    /// a session is alive. Tokens never go down within one session, so
    /// "only grow" is the right semantics; an unexpected smaller read
    /// (corrupt source, partial write) is silently ignored.
    ///
    /// Identity columns (`id`, `dedupe_key`, `timestamp`, `provider`,
    /// `model`, source, project, session/request id) are NOT touched on
    /// conflict — `timestamp` is the session start and must stay stable
    /// across updates so timestamp-range rollups (`today`, `week`,
    /// `month`) keep classifying the event under the period it began in.
    pub async fn upsert_usage_event_by_dedupe_key(
        &self,
        event: &UsageEvent,
        dedupe_key: &str,
    ) -> anyhow::Result<UpsertOutcome> {
        let metadata_json = serde_json::to_string(&event.metadata)?;
        let project_path = event
            .project_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string());
        let (estimated_cost_usd, currency) = event
            .estimated_cost
            .as_ref()
            .map(|money| (Some(money.amount), money.currency.as_str()))
            .unwrap_or((None, "USD"));

        // A single transaction so the existence probe and the upsert see
        // the same snapshot — without it, a concurrent writer could turn
        // a Some/0 result into a phantom Inserted outcome.
        let mut tx = self.pool.begin().await?;

        let existed: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM usage_events WHERE dedupe_key = ? LIMIT 1")
                .bind(dedupe_key)
                .fetch_optional(&mut *tx)
                .await?;

        let result = sqlx::query(
            r#"
            INSERT INTO usage_events (
                id,
                dedupe_key,
                timestamp,
                provider,
                model,
                input_tokens,
                output_tokens,
                cached_input_tokens,
                reasoning_tokens,
                total_tokens,
                estimated_cost_usd,
                currency,
                source_app,
                source_type,
                project_path,
                session_id,
                request_id,
                metadata_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(dedupe_key) DO UPDATE SET
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                cached_input_tokens = excluded.cached_input_tokens,
                reasoning_tokens = excluded.reasoning_tokens,
                total_tokens = excluded.total_tokens,
                estimated_cost_usd = excluded.estimated_cost_usd,
                currency = excluded.currency,
                metadata_json = excluded.metadata_json
            WHERE excluded.total_tokens > usage_events.total_tokens
            "#,
        )
        .bind(event.id.to_string())
        .bind(dedupe_key)
        .bind(event.timestamp.to_rfc3339())
        .bind(&event.provider.0)
        .bind(&event.model.0)
        .bind(event.input_tokens as i64)
        .bind(event.output_tokens as i64)
        .bind(event.cached_input_tokens.map(|tokens| tokens as i64))
        .bind(event.reasoning_tokens.map(|tokens| tokens as i64))
        .bind(event.total_tokens as i64)
        .bind(estimated_cost_usd)
        .bind(currency)
        .bind(&event.source.app)
        .bind(source_kind_name(&event.source.kind))
        .bind(project_path)
        .bind(&event.session_id)
        .bind(&event.request_id)
        .bind(metadata_json)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(match (existed.is_some(), result.rows_affected()) {
            (false, 1) => UpsertOutcome::Inserted,
            (true, 1) => UpsertOutcome::Updated,
            (true, 0) => UpsertOutcome::Unchanged,
            // (false, 0) would mean SQLite accepted an INSERT without
            // touching a row, which never happens for this statement.
            (existed, n) => {
                anyhow::bail!(
                    "upsert returned an impossible (existed={existed}, rows_affected={n}) state"
                );
            }
        })
    }

    pub async fn count_usage_events(&self) -> anyhow::Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS count FROM usage_events")
            .fetch_one(&self.pool)
            .await?;

        Ok(row.get("count"))
    }

    pub async fn usage_totals_by_model(&self) -> anyhow::Result<Vec<UsageModelTotal>> {
        self.usage_totals_by_model_filtered(None).await
    }

    pub async fn usage_totals_by_model_filtered(
        &self,
        provider: Option<&str>,
    ) -> anyhow::Result<Vec<UsageModelTotal>> {
        let where_clause = match provider {
            Some(_) => "WHERE provider = ?",
            None => "",
        };
        let sql = model_totals_sql(where_clause);

        let mut query = sqlx::query(&sql);
        if let Some(provider) = provider {
            query = query.bind(provider);
        }
        let rows = query.fetch_all(&self.pool).await?;

        Ok(rows.into_iter().map(usage_model_total_from_row).collect())
    }

    pub async fn latest_usage_event_timestamp_for_provider(
        &self,
        provider: &str,
    ) -> anyhow::Result<Option<DateTime<Utc>>> {
        let row =
            sqlx::query("SELECT MAX(timestamp) AS max_ts FROM usage_events WHERE provider = ?")
                .bind(provider)
                .fetch_one(&self.pool)
                .await?;
        let raw: Option<String> = row.try_get("max_ts")?;
        raw.as_deref().map(parse_rfc3339).transpose()
    }

    pub async fn usage_totals_between(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> anyhow::Result<UsageTotals> {
        self.usage_totals_between_filtered(start, end, None).await
    }

    pub async fn usage_totals_by_model_between_filtered(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        provider: Option<&str>,
    ) -> anyhow::Result<Vec<UsageModelTotal>> {
        let sql = model_totals_sql(&format!(
            "WHERE timestamp >= ? AND timestamp < ?{}",
            provider_clause(provider)
        ));

        let mut query = sqlx::query(&sql)
            .bind(start.to_rfc3339())
            .bind(end.to_rfc3339());
        if let Some(provider) = provider {
            query = query.bind(provider);
        }
        let rows = query.fetch_all(&self.pool).await?;

        Ok(rows.into_iter().map(usage_model_total_from_row).collect())
    }

    pub async fn usage_totals_between_filtered(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        provider: Option<&str>,
    ) -> anyhow::Result<UsageTotals> {
        let sql = format!(
            r#"
            SELECT
                COUNT(*) AS events,
                COALESCE(SUM(input_tokens), 0) AS input_tokens,
                COALESCE(SUM(cached_input_tokens), 0) AS cached_input_tokens,
                COALESCE(SUM(output_tokens), 0) AS output_tokens,
                COALESCE(SUM(total_tokens), 0) AS total_tokens
            FROM usage_events
            WHERE timestamp >= ? AND timestamp < ?{}
            "#,
            provider_clause(provider)
        );

        let mut query = sqlx::query(&sql)
            .bind(start.to_rfc3339())
            .bind(end.to_rfc3339());
        if let Some(provider) = provider {
            query = query.bind(provider);
        }
        let row = query.fetch_one(&self.pool).await?;

        Ok(UsageTotals {
            events: row.get("events"),
            input_tokens: row.get("input_tokens"),
            cached_input_tokens: row.get("cached_input_tokens"),
            output_tokens: row.get("output_tokens"),
            total_tokens: row.get("total_tokens"),
        })
    }
}

/// The `SELECT … FROM usage_events` per-model rollup shared by every
/// `usage_totals_by_model*` query; `where_clause` is spliced in verbatim
/// (`""` for no filter) and decides how many binds the caller appends.
fn model_totals_sql(where_clause: &str) -> String {
    format!(
        r#"
        SELECT
            provider,
            model,
            COUNT(*) AS events,
            COALESCE(SUM(input_tokens), 0) AS input_tokens,
            COALESCE(SUM(cached_input_tokens), 0) AS cached_input_tokens,
            COALESCE(SUM(output_tokens), 0) AS output_tokens,
            COALESCE(SUM(total_tokens), 0) AS total_tokens
        FROM usage_events
        {where_clause}
        GROUP BY provider, model
        ORDER BY total_tokens DESC, provider, model
        "#
    )
}

/// `" AND provider = ?"` when filtering by provider, else empty — appended
/// after an existing `WHERE` clause.
fn provider_clause(provider: Option<&str>) -> &'static str {
    match provider {
        Some(_) => " AND provider = ?",
        None => "",
    }
}

fn usage_model_total_from_row(row: SqliteRow) -> UsageModelTotal {
    UsageModelTotal {
        provider: row.get("provider"),
        model: row.get("model"),
        events: row.get("events"),
        input_tokens: row.get("input_tokens"),
        cached_input_tokens: row.get("cached_input_tokens"),
        output_tokens: row.get("output_tokens"),
        total_tokens: row.get("total_tokens"),
    }
}

fn source_kind_name(kind: &UsageSourceKind) -> &'static str {
    match kind {
        UsageSourceKind::Log => "log",
        UsageSourceKind::Proxy => "proxy",
        UsageSourceKind::Api => "api",
        UsageSourceKind::Manual => "manual",
        UsageSourceKind::SdkWrapper => "sdk-wrapper",
        UsageSourceKind::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use skopos_core::{ModelId, ProviderId, UsageEvent, UsageSource, UsageSourceKind};

    fn usage_event(request_id: &str) -> UsageEvent {
        usage_event_at(request_id, Utc::now())
    }

    fn usage_event_at(request_id: &str, timestamp: chrono::DateTime<Utc>) -> UsageEvent {
        let mut event = UsageEvent::new(
            ProviderId::new("anthropic"),
            ModelId::new("claude-opus-4-7"),
            10,
            20,
            UsageSource {
                app: "claude-code".to_string(),
                kind: UsageSourceKind::Log,
            },
        );
        event.cached_input_tokens = Some(30);
        event.total_tokens = 60;
        event.timestamp = timestamp;
        event.session_id = Some("session-1".to_string());
        event.request_id = Some(request_id.to_string());
        event
    }

    #[tokio::test]
    async fn upsert_inserts_then_updates_then_no_ops() {
        let store = SkoposStore::connect("sqlite::memory:").await.unwrap();
        store.migrate().await.unwrap();

        // First call: fresh dedupe key → Inserted.
        let mut event = usage_event_at(
            "session-1",
            Utc.with_ymd_and_hms(2026, 5, 13, 12, 0, 0).unwrap(),
        );
        let first = store
            .upsert_usage_event_by_dedupe_key(&event, "hermes:session:s1")
            .await
            .unwrap();
        assert_eq!(first, UpsertOutcome::Inserted);
        assert_eq!(store.count_usage_events().await.unwrap(), 1);

        // Same dedupe key, total grew → Updated, still a single row.
        event.input_tokens = 100;
        event.output_tokens = 200;
        event.cached_input_tokens = Some(50);
        event.total_tokens = 350;
        let second = store
            .upsert_usage_event_by_dedupe_key(&event, "hermes:session:s1")
            .await
            .unwrap();
        assert_eq!(second, UpsertOutcome::Updated);
        assert_eq!(store.count_usage_events().await.unwrap(), 1);

        // Same totals re-imported → Unchanged (the `>` guard blocks).
        let third = store
            .upsert_usage_event_by_dedupe_key(&event, "hermes:session:s1")
            .await
            .unwrap();
        assert_eq!(third, UpsertOutcome::Unchanged);

        // Verify the stored row reflects the larger numbers.
        let totals = store.usage_totals_by_model().await.unwrap();
        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].input_tokens, 100);
        assert_eq!(totals[0].output_tokens, 200);
        assert_eq!(totals[0].cached_input_tokens, 50);
        assert_eq!(totals[0].total_tokens, 350);
    }

    #[tokio::test]
    async fn upsert_with_smaller_total_does_not_clobber() {
        let store = SkoposStore::connect("sqlite::memory:").await.unwrap();
        store.migrate().await.unwrap();

        // Seed a row with large totals.
        let mut big = usage_event("session-big");
        big.input_tokens = 1_000;
        big.output_tokens = 2_000;
        big.total_tokens = 3_000;
        let outcome = store
            .upsert_usage_event_by_dedupe_key(&big, "hermes:session:big")
            .await
            .unwrap();
        assert_eq!(outcome, UpsertOutcome::Inserted);

        // Now try to upsert the same key with smaller totals (e.g.
        // accidental partial read). The monotonic guard must protect us.
        let mut small = big.clone();
        small.input_tokens = 10;
        small.output_tokens = 20;
        small.total_tokens = 30;
        let outcome = store
            .upsert_usage_event_by_dedupe_key(&small, "hermes:session:big")
            .await
            .unwrap();
        assert_eq!(outcome, UpsertOutcome::Unchanged);

        let totals = store.usage_totals_by_model().await.unwrap();
        assert_eq!(totals[0].total_tokens, 3_000, "big row must survive");
    }

    #[tokio::test]
    async fn insert_usage_event_once_deduplicates_by_key() {
        let store = SkoposStore::connect("sqlite::memory:").await.unwrap();
        store.migrate().await.unwrap();
        let event = usage_event("msg-1");

        let first = store
            .insert_usage_event_once(&event, "claude-code:session-1:msg-1")
            .await
            .unwrap();
        let second = store
            .insert_usage_event_once(&event, "claude-code:session-1:msg-1")
            .await
            .unwrap();

        assert!(first.inserted);
        assert!(!second.inserted);
        assert_eq!(store.count_usage_events().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn usage_totals_by_model_reads_persisted_events() {
        let store = SkoposStore::connect("sqlite::memory:").await.unwrap();
        store.migrate().await.unwrap();

        store
            .insert_usage_event_once(&usage_event("msg-1"), "claude-code:session-1:msg-1")
            .await
            .unwrap();
        store
            .insert_usage_event_once(&usage_event("msg-2"), "claude-code:session-1:msg-2")
            .await
            .unwrap();

        let totals = store.usage_totals_by_model().await.unwrap();

        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].provider, "anthropic");
        assert_eq!(totals[0].model, "claude-opus-4-7");
        assert_eq!(totals[0].events, 2);
        assert_eq!(totals[0].input_tokens, 20);
        assert_eq!(totals[0].cached_input_tokens, 60);
        assert_eq!(totals[0].output_tokens, 40);
        assert_eq!(totals[0].total_tokens, 120);
    }

    #[tokio::test]
    async fn latest_usage_event_timestamp_for_provider_returns_max_per_provider() {
        let store = SkoposStore::connect("sqlite::memory:").await.unwrap();
        store.migrate().await.unwrap();

        let earlier = Utc.with_ymd_and_hms(2026, 5, 10, 12, 0, 0).unwrap();
        let later = Utc.with_ymd_and_hms(2026, 5, 12, 9, 30, 0).unwrap();
        let other_provider_later = Utc.with_ymd_and_hms(2026, 5, 15, 0, 0, 0).unwrap();

        let mut event_a = usage_event_at("a", earlier);
        event_a.provider = ProviderId::new("openai");
        let mut event_b = usage_event_at("b", later);
        event_b.provider = ProviderId::new("openai");
        let mut event_c = usage_event_at("c", other_provider_later);
        event_c.provider = ProviderId::new("anthropic");

        store
            .insert_usage_event_once(&event_a, "openai:a")
            .await
            .unwrap();
        store
            .insert_usage_event_once(&event_b, "openai:b")
            .await
            .unwrap();
        store
            .insert_usage_event_once(&event_c, "anthropic:c")
            .await
            .unwrap();

        let max = store
            .latest_usage_event_timestamp_for_provider("openai")
            .await
            .unwrap();
        assert_eq!(max, Some(later));

        let absent = store
            .latest_usage_event_timestamp_for_provider("nope")
            .await
            .unwrap();
        assert_eq!(absent, None);
    }

    #[tokio::test]
    async fn usage_totals_between_filters_by_timestamp_range() {
        let store = SkoposStore::connect("sqlite::memory:").await.unwrap();
        store.migrate().await.unwrap();
        let start = Utc.with_ymd_and_hms(2026, 5, 13, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 5, 14, 0, 0, 0).unwrap();

        store
            .insert_usage_event_once(
                &usage_event_at(
                    "before",
                    Utc.with_ymd_and_hms(2026, 5, 12, 23, 59, 59).unwrap(),
                ),
                "before",
            )
            .await
            .unwrap();
        store
            .insert_usage_event_once(&usage_event_at("inside", start), "inside")
            .await
            .unwrap();
        store
            .insert_usage_event_once(&usage_event_at("after", end), "after")
            .await
            .unwrap();

        let totals = store.usage_totals_between(start, end).await.unwrap();

        assert_eq!(totals.events, 1);
        assert_eq!(totals.input_tokens, 10);
        assert_eq!(totals.cached_input_tokens, 30);
        assert_eq!(totals.output_tokens, 20);
        assert_eq!(totals.total_tokens, 60);
    }

    #[tokio::test]
    async fn usage_totals_by_model_between_groups_per_model_in_range() {
        let store = SkoposStore::connect("sqlite::memory:").await.unwrap();
        store.migrate().await.unwrap();
        let start = Utc.with_ymd_and_hms(2026, 5, 13, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 5, 14, 0, 0, 0).unwrap();

        let mut inside_a = usage_event_at("inside-a", start);
        inside_a.model = ModelId::new("claude-opus-4-7");
        let mut inside_b = usage_event_at("inside-b", start);
        inside_b.model = ModelId::new("claude-haiku-4-5-20251001");
        let mut outside = usage_event_at("outside", end);
        outside.model = ModelId::new("claude-opus-4-7");

        store
            .insert_usage_event_once(&inside_a, "inside-a")
            .await
            .unwrap();
        store
            .insert_usage_event_once(&inside_b, "inside-b")
            .await
            .unwrap();
        store
            .insert_usage_event_once(&outside, "outside")
            .await
            .unwrap();

        let totals = store
            .usage_totals_by_model_between_filtered(start, end, None)
            .await
            .unwrap();
        assert_eq!(totals.len(), 2);
        assert!(totals
            .iter()
            .any(|t| t.model == "claude-opus-4-7" && t.events == 1));
        assert!(totals
            .iter()
            .any(|t| t.model == "claude-haiku-4-5-20251001" && t.events == 1));
    }
}
