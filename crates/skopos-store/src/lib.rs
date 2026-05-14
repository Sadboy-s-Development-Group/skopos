use chrono::{DateTime, Utc};
use skopos_core::{UsageEvent, UsageSourceKind};
use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};
use std::{path::Path, str::FromStr};

#[derive(Clone)]
pub struct SkoposStore {
    pool: SqlitePool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsertUsageResult {
    pub inserted: bool,
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
    pub async fn connect(database_url: &str) -> anyhow::Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(database_url)
            .await?;

        Ok(Self { pool })
    }

    pub async fn connect_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let options =
            sqlx::sqlite::SqliteConnectOptions::from_str(&path.as_ref().to_string_lossy())?
                .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn migrate(&self) -> anyhow::Result<()> {
        for statement in include_str!("../../../migrations/001_initial.sql").split(';') {
            let statement = statement.trim();
            if !statement.is_empty() {
                sqlx::query(statement).execute(&self.pool).await?;
            }
        }

        Ok(())
    }

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

    pub async fn count_usage_events(&self) -> anyhow::Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS count FROM usage_events")
            .fetch_one(&self.pool)
            .await?;

        Ok(row.get("count"))
    }

    pub async fn usage_totals_by_model(&self) -> anyhow::Result<Vec<UsageModelTotal>> {
        let rows = sqlx::query(
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
            GROUP BY provider, model
            ORDER BY total_tokens DESC, provider, model
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| UsageModelTotal {
                provider: row.get("provider"),
                model: row.get("model"),
                events: row.get("events"),
                input_tokens: row.get("input_tokens"),
                cached_input_tokens: row.get("cached_input_tokens"),
                output_tokens: row.get("output_tokens"),
                total_tokens: row.get("total_tokens"),
            })
            .collect())
    }

    pub async fn usage_totals_between(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> anyhow::Result<UsageTotals> {
        let row = sqlx::query(
            r#"
            SELECT
                COUNT(*) AS events,
                COALESCE(SUM(input_tokens), 0) AS input_tokens,
                COALESCE(SUM(cached_input_tokens), 0) AS cached_input_tokens,
                COALESCE(SUM(output_tokens), 0) AS output_tokens,
                COALESCE(SUM(total_tokens), 0) AS total_tokens
            FROM usage_events
            WHERE timestamp >= ? AND timestamp < ?
            "#,
        )
        .bind(start.to_rfc3339())
        .bind(end.to_rfc3339())
        .fetch_one(&self.pool)
        .await?;

        Ok(UsageTotals {
            events: row.get("events"),
            input_tokens: row.get("input_tokens"),
            cached_input_tokens: row.get("cached_input_tokens"),
            output_tokens: row.get("output_tokens"),
            total_tokens: row.get("total_tokens"),
        })
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
}
