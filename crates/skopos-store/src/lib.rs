use chrono::{DateTime, Utc};
use skopos_core::{UsageEvent, UsageSourceKind};
use sqlx::{
    sqlite::{SqlitePoolOptions, SqliteRow},
    Row, SqlitePool,
};
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

/// One connectivity probe tick recorded by `skopos network watch`.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkSample {
    pub ts: DateTime<Utc>,
    /// `ok` | `degraded` | `down`.
    pub status: String,
    /// Best (lowest) average RTT among responding targets, `None` when down.
    pub rtt_ms: Option<f64>,
    pub loss_pct: f64,
    pub sites_ok: i64,
    pub sites_total: i64,
    pub iface: Option<String>,
    /// Local link carrier — `Some(true)` up, `Some(false)` down.
    pub carrier: Option<bool>,
}

/// A contiguous run of unreachable ticks.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkOutage {
    pub id: i64,
    pub started_at: DateTime<Utc>,
    /// `None` while the outage is still ongoing.
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_secs: Option<i64>,
    pub down_samples: i64,
    pub cause: Option<String>,
}

/// Per-status sample counts over a window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetworkStatusCounts {
    pub ok: i64,
    pub degraded: i64,
    pub down: i64,
}

impl NetworkStatusCounts {
    pub fn total(&self) -> i64 {
        self.ok + self.degraded + self.down
    }
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
        const MIGRATIONS: &[&str] = &[
            include_str!("../../../migrations/001_initial.sql"),
            include_str!("../../../migrations/002_network.sql"),
        ];
        for migration in MIGRATIONS {
            for statement in migration.split(';') {
                let statement = statement.trim();
                if !statement.is_empty() {
                    sqlx::query(statement).execute(&self.pool).await?;
                }
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
        self.usage_totals_by_model_filtered(None).await
    }

    pub async fn usage_totals_by_model_filtered(
        &self,
        provider: Option<&str>,
    ) -> anyhow::Result<Vec<UsageModelTotal>> {
        let sql = match provider {
            Some(_) => {
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
                WHERE provider = ?
                GROUP BY provider, model
                ORDER BY total_tokens DESC, provider, model
                "#
            }
            None => {
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
                "#
            }
        };

        let mut query = sqlx::query(sql);
        if let Some(provider) = provider {
            query = query.bind(provider);
        }
        let rows = query.fetch_all(&self.pool).await?;

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
        match raw {
            Some(ts) => Ok(Some(DateTime::parse_from_rfc3339(&ts)?.with_timezone(&Utc))),
            None => Ok(None),
        }
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
        let sql = match provider {
            Some(_) => {
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
                WHERE timestamp >= ? AND timestamp < ? AND provider = ?
                GROUP BY provider, model
                ORDER BY total_tokens DESC, provider, model
                "#
            }
            None => {
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
                WHERE timestamp >= ? AND timestamp < ?
                GROUP BY provider, model
                ORDER BY total_tokens DESC, provider, model
                "#
            }
        };

        let mut query = sqlx::query(sql)
            .bind(start.to_rfc3339())
            .bind(end.to_rfc3339());
        if let Some(provider) = provider {
            query = query.bind(provider);
        }
        let rows = query.fetch_all(&self.pool).await?;

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

    pub async fn usage_totals_between_filtered(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        provider: Option<&str>,
    ) -> anyhow::Result<UsageTotals> {
        let sql = match provider {
            Some(_) => {
                r#"
                SELECT
                    COUNT(*) AS events,
                    COALESCE(SUM(input_tokens), 0) AS input_tokens,
                    COALESCE(SUM(cached_input_tokens), 0) AS cached_input_tokens,
                    COALESCE(SUM(output_tokens), 0) AS output_tokens,
                    COALESCE(SUM(total_tokens), 0) AS total_tokens
                FROM usage_events
                WHERE timestamp >= ? AND timestamp < ? AND provider = ?
                "#
            }
            None => {
                r#"
                SELECT
                    COUNT(*) AS events,
                    COALESCE(SUM(input_tokens), 0) AS input_tokens,
                    COALESCE(SUM(cached_input_tokens), 0) AS cached_input_tokens,
                    COALESCE(SUM(output_tokens), 0) AS output_tokens,
                    COALESCE(SUM(total_tokens), 0) AS total_tokens
                FROM usage_events
                WHERE timestamp >= ? AND timestamp < ?
                "#
            }
        };

        let mut query = sqlx::query(sql)
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

    // =======================================================================
    // Network connectivity tracking
    // =======================================================================

    /// Record one probe tick.
    pub async fn insert_network_sample(&self, sample: &NetworkSample) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO network_samples
                (ts, status, rtt_ms, loss_pct, sites_ok, sites_total, iface, carrier)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(sample.ts.to_rfc3339())
        .bind(&sample.status)
        .bind(sample.rtt_ms)
        .bind(sample.loss_pct)
        .bind(sample.sites_ok)
        .bind(sample.sites_total)
        .bind(&sample.iface)
        .bind(sample.carrier.map(i64::from))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Open a new outage and return its row id.
    pub async fn open_network_outage(
        &self,
        started_at: DateTime<Utc>,
        cause: Option<&str>,
    ) -> anyhow::Result<i64> {
        let result = sqlx::query(
            "INSERT INTO network_outages (started_at, down_samples, cause) VALUES (?, 1, ?)",
        )
        .bind(started_at.to_rfc3339())
        .bind(cause)
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    /// Bump the down-sample count of an ongoing outage.
    pub async fn touch_network_outage(&self, id: i64, down_samples: i64) -> anyhow::Result<()> {
        sqlx::query("UPDATE network_outages SET down_samples = ? WHERE id = ?")
            .bind(down_samples)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Close an outage with its final end time and duration.
    pub async fn close_network_outage(
        &self,
        id: i64,
        ended_at: DateTime<Utc>,
        duration_secs: i64,
        down_samples: i64,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE network_outages SET ended_at = ?, duration_secs = ?, down_samples = ? WHERE id = ?",
        )
        .bind(ended_at.to_rfc3339())
        .bind(duration_secs)
        .bind(down_samples)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The most recent outage that has not been closed, if any. Used on
    /// daemon startup to resume an outage interrupted by a restart.
    pub async fn latest_open_network_outage(&self) -> anyhow::Result<Option<NetworkOutage>> {
        let row = sqlx::query(
            "SELECT id, started_at, ended_at, duration_secs, down_samples, cause
             FROM network_outages WHERE ended_at IS NULL ORDER BY started_at DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(network_outage_from_row).transpose()
    }

    /// Probe samples at or after `since`, oldest first.
    pub async fn network_samples_since(
        &self,
        since: DateTime<Utc>,
    ) -> anyhow::Result<Vec<NetworkSample>> {
        let rows = sqlx::query(
            "SELECT ts, status, rtt_ms, loss_pct, sites_ok, sites_total, iface, carrier
             FROM network_samples WHERE ts >= ? ORDER BY ts ASC",
        )
        .bind(since.to_rfc3339())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(network_sample_from_row).collect()
    }

    /// Outages overlapping `[since, now]` — ongoing, or ended at/after `since`.
    /// Newest first.
    pub async fn network_outages_since(
        &self,
        since: DateTime<Utc>,
    ) -> anyhow::Result<Vec<NetworkOutage>> {
        let rows = sqlx::query(
            "SELECT id, started_at, ended_at, duration_secs, down_samples, cause
             FROM network_outages WHERE ended_at IS NULL OR ended_at >= ?
             ORDER BY started_at DESC",
        )
        .bind(since.to_rfc3339())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(network_outage_from_row).collect()
    }

    /// The most recent probe sample, if any.
    pub async fn latest_network_sample(&self) -> anyhow::Result<Option<NetworkSample>> {
        let row = sqlx::query(
            "SELECT ts, status, rtt_ms, loss_pct, sites_ok, sites_total, iface, carrier
             FROM network_samples ORDER BY ts DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(network_sample_from_row).transpose()
    }

    /// Count probe samples by status at or after `since`.
    pub async fn network_status_counts_since(
        &self,
        since: DateTime<Utc>,
    ) -> anyhow::Result<NetworkStatusCounts> {
        let rows = sqlx::query(
            "SELECT status, COUNT(*) AS n FROM network_samples WHERE ts >= ? GROUP BY status",
        )
        .bind(since.to_rfc3339())
        .fetch_all(&self.pool)
        .await?;
        let mut counts = NetworkStatusCounts::default();
        for row in rows {
            let status: String = row.get("status");
            let n: i64 = row.get("n");
            match status.as_str() {
                "ok" => counts.ok = n,
                "degraded" => counts.degraded = n,
                "down" => counts.down = n,
                _ => {}
            }
        }
        Ok(counts)
    }

    /// Highest recorded RTT at or after `since`, if any sample carried one.
    pub async fn network_worst_rtt_since(
        &self,
        since: DateTime<Utc>,
    ) -> anyhow::Result<Option<f64>> {
        let row = sqlx::query("SELECT MAX(rtt_ms) AS r FROM network_samples WHERE ts >= ?")
            .bind(since.to_rfc3339())
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("r")?)
    }

    /// Delete probe samples older than `cutoff`. Returns rows removed.
    pub async fn prune_network_samples_before(&self, cutoff: DateTime<Utc>) -> anyhow::Result<u64> {
        let result = sqlx::query("DELETE FROM network_samples WHERE ts < ?")
            .bind(cutoff.to_rfc3339())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}

fn parse_rfc3339(raw: &str) -> anyhow::Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(raw)?.with_timezone(&Utc))
}

fn network_sample_from_row(row: SqliteRow) -> anyhow::Result<NetworkSample> {
    let ts: String = row.get("ts");
    let carrier: Option<i64> = row.try_get("carrier")?;
    Ok(NetworkSample {
        ts: parse_rfc3339(&ts)?,
        status: row.get("status"),
        rtt_ms: row.try_get("rtt_ms")?,
        loss_pct: row.get("loss_pct"),
        sites_ok: row.get("sites_ok"),
        sites_total: row.get("sites_total"),
        iface: row.try_get("iface")?,
        carrier: carrier.map(|value| value != 0),
    })
}

fn network_outage_from_row(row: SqliteRow) -> anyhow::Result<NetworkOutage> {
    let started_at: String = row.get("started_at");
    let ended_at: Option<String> = row.try_get("ended_at")?;
    Ok(NetworkOutage {
        id: row.get("id"),
        started_at: parse_rfc3339(&started_at)?,
        ended_at: ended_at.as_deref().map(parse_rfc3339).transpose()?,
        duration_secs: row.try_get("duration_secs")?,
        down_samples: row.get("down_samples"),
        cause: row.try_get("cause")?,
    })
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

    #[tokio::test]
    async fn network_sample_and_outage_roundtrip() {
        let store = SkoposStore::connect("sqlite::memory:").await.unwrap();
        store.migrate().await.unwrap();
        let now = Utc.with_ymd_and_hms(2026, 5, 20, 12, 0, 0).unwrap();

        store
            .insert_network_sample(&NetworkSample {
                ts: now,
                status: "ok".to_string(),
                rtt_ms: Some(12.5),
                loss_pct: 0.0,
                sites_ok: 3,
                sites_total: 3,
                iface: Some("eth0".to_string()),
                carrier: Some(true),
            })
            .await
            .unwrap();

        let window_start = now - chrono::Duration::hours(1);
        let samples = store.network_samples_since(window_start).await.unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].status, "ok");
        assert_eq!(samples[0].carrier, Some(true));
        assert_eq!(samples[0].rtt_ms, Some(12.5));

        let counts = store
            .network_status_counts_since(window_start)
            .await
            .unwrap();
        assert_eq!(counts.ok, 1);
        assert_eq!(counts.total(), 1);

        let id = store
            .open_network_outage(now, Some("unreachable"))
            .await
            .unwrap();
        let open = store.latest_open_network_outage().await.unwrap().unwrap();
        assert_eq!(open.id, id);
        assert!(open.ended_at.is_none());

        store
            .close_network_outage(id, now + chrono::Duration::seconds(30), 30, 3)
            .await
            .unwrap();
        assert!(store.latest_open_network_outage().await.unwrap().is_none());

        let outages = store.network_outages_since(window_start).await.unwrap();
        assert_eq!(outages.len(), 1);
        assert_eq!(outages[0].duration_secs, Some(30));
        assert_eq!(outages[0].cause.as_deref(), Some("unreachable"));
    }
}
