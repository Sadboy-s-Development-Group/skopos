//! Connectivity tracking for `skopos network watch` — probe samples and the
//! outage runs derived from them.

use chrono::{DateTime, Utc};
use sqlx::{sqlite::SqliteRow, Row};

use crate::{parse_rfc3339, SkoposStore};

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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

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
