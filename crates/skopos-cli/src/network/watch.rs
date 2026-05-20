//! `skopos network watch` — the foreground connectivity probe daemon.
//!
//! This is what the systemd unit runs. It ticks on a fixed interval,
//! records every probe sample, maintains the outage table with a small
//! state machine, and prunes stale samples hourly. Per-tick failures are
//! logged but never fatal — the daemon must keep probing.

use std::path::PathBuf;
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Duration, Utc};
use skopos_store::{NetworkSample, SkoposStore};

use crate::config::Config;

use super::probe::{self, ProbeConfig, ProbeResult, TickStatus};

/// In-memory handle to the outage currently being recorded.
struct OpenOutage {
    id: i64,
    down_samples: i64,
    started_at: DateTime<Utc>,
}

pub(super) async fn run(cfg: &Config, db: PathBuf) -> anyhow::Result<()> {
    let probe_cfg = ProbeConfig::from_network(&cfg.network);
    if probe_cfg.targets.is_empty() {
        anyhow::bail!("no probe targets configured — set [network].targets in config.toml");
    }

    if let Some(parent) = db.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let store = SkoposStore::connect_path(&db).await?;
    store.migrate().await?;

    let interval = StdDuration::from_secs(cfg.network.interval_secs.max(1));
    let retention_days = cfg.network.retention_days.max(1);

    // Resume any outage that was still open when a previous run stopped, so
    // a daemon restart mid-outage does not split it into two.
    let mut current: Option<OpenOutage> =
        store
            .latest_open_network_outage()
            .await?
            .map(|outage| OpenOutage {
                id: outage.id,
                down_samples: outage.down_samples,
                started_at: outage.started_at,
            });

    println!(
        "skopos network watch — every {}s, pinging {}",
        interval.as_secs(),
        probe_cfg.targets.join(", "),
    );

    let mut last_prune = Instant::now();
    loop {
        let result = probe::probe_once(&probe_cfg).await;
        let now = Utc::now();

        let sample = NetworkSample {
            ts: now,
            status: result.status.as_str().to_string(),
            rtt_ms: result.rtt_ms,
            loss_pct: result.loss_pct,
            sites_ok: result.sites_ok as i64,
            sites_total: result.sites_total as i64,
            iface: result.iface.clone(),
            carrier: result.carrier,
        };
        if let Err(err) = store.insert_network_sample(&sample).await {
            eprintln!("skopos network watch: failed to record sample: {err}");
        }

        match result.status {
            TickStatus::Down => match &mut current {
                Some(open) => {
                    open.down_samples += 1;
                    if let Err(err) = store.touch_network_outage(open.id, open.down_samples).await {
                        eprintln!("skopos network watch: failed to update outage: {err}");
                    }
                }
                None => match store
                    .open_network_outage(now, result.cause.as_deref())
                    .await
                {
                    Ok(id) => {
                        current = Some(OpenOutage {
                            id,
                            down_samples: 1,
                            started_at: now,
                        });
                    }
                    Err(err) => {
                        eprintln!("skopos network watch: failed to open outage: {err}");
                    }
                },
            },
            TickStatus::Ok | TickStatus::Degraded => {
                if let Some(open) = current.take() {
                    let duration = (now - open.started_at).num_seconds().max(0);
                    if let Err(err) = store
                        .close_network_outage(open.id, now, duration, open.down_samples)
                        .await
                    {
                        eprintln!("skopos network watch: failed to close outage: {err}");
                    }
                }
            }
        }

        println!(
            "{}  {}",
            now.format("%Y-%m-%dT%H:%M:%SZ"),
            tick_line(&result)
        );

        if last_prune.elapsed() >= StdDuration::from_secs(3600) {
            let cutoff = now - Duration::days(retention_days);
            match store.prune_network_samples_before(cutoff).await {
                Ok(removed) if removed > 0 => {
                    println!(
                        "skopos network watch: pruned {removed} samples older than {retention_days}d"
                    );
                }
                Ok(_) => {}
                Err(err) => eprintln!("skopos network watch: prune failed: {err}"),
            }
            last_prune = Instant::now();
        }

        tokio::time::sleep(interval).await;
    }
}

/// One-line journal-friendly summary of a tick.
fn tick_line(result: &ProbeResult) -> String {
    let rtt = result
        .rtt_ms
        .map(|rtt| format!("{rtt:.0}ms"))
        .unwrap_or_else(|| "—".to_string());
    let cause = result
        .cause
        .as_deref()
        .map(|cause| format!(" ({cause})"))
        .unwrap_or_default();
    format!(
        "{:<8} rtt {:<7} loss {:>3.0}%  {}/{} sites{}",
        result.status.as_str(),
        rtt,
        result.loss_pct,
        result.sites_ok,
        result.sites_total,
        cause,
    )
}
