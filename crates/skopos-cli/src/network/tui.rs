//! `skopos network` — the live connectivity dashboard, plus the one-shot
//! `skopos network status` text report.
//!
//! Both are pure readers over the SQLite history the `watch` daemon
//! records. The dashboard redraws once a second inside the alternate
//! screen (crossterm raw mode); `status` gathers the same snapshot once,
//! prints it, and hands back a process exit code.

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Local, Utc};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::Print,
    terminal::{self, Clear, ClearType},
};
use skopos_store::{NetworkOutage, NetworkSample, SkoposStore};

use crate::config::Config;
use crate::limits::{humanise_relative_past, progress_bar};
use crate::theme::{dim, purple, purple_bold};

use super::health::{self, HealthReport, HealthThresholds};
use super::{hostname, rgb, rgb_bold};

const GREEN: (u8, u8, u8) = (78, 201, 109);
const AMBER: (u8, u8, u8) = (224, 165, 48);
const RED: (u8, u8, u8) = (232, 86, 76);
/// Number of one-minute cells in the timeline sparkline.
const SPARK_CELLS: i64 = 60;

// ===========================================================================
// Entry points
// ===========================================================================

/// `skopos network` — open the live dashboard. Falls back to the one-shot
/// report when stdout/stdin is not a terminal.
pub(super) async fn run(cfg: &Config, db: PathBuf) -> anyhow::Result<()> {
    let store = open_store(&db).await?;

    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        let dash = gather(&store, cfg, headline_default(cfg)).await?;
        print!("{}", render_status_text(&dash));
        return Ok(());
    }

    enter_screen()?;
    let result = dashboard_loop(&store, cfg).await;
    let _ = leave_screen();
    result
}

/// `skopos network status` — print a one-shot verdict, return the exit code.
pub(super) async fn run_status(cfg: &Config, db: PathBuf) -> anyhow::Result<i32> {
    let store = open_store(&db).await?;
    let dash = gather(&store, cfg, headline_default(cfg)).await?;
    print!("{}", render_status_text(&dash));
    Ok(match &dash.latest {
        Some(_) => dash.report.health.exit_code(),
        None => 0,
    })
}

async fn open_store(db: &Path) -> anyhow::Result<SkoposStore> {
    if let Some(parent) = db.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let store = SkoposStore::connect_path(db).await?;
    store.migrate().await?;
    Ok(store)
}

fn headline_default(cfg: &Config) -> Duration {
    Duration::minutes(cfg.network.headline_window_mins.max(1))
}

// ===========================================================================
// Dashboard loop
// ===========================================================================

fn enter_screen() -> io::Result<()> {
    terminal::enable_raw_mode()?;
    execute!(io::stdout(), terminal::EnterAlternateScreen, cursor::Hide)
}

fn leave_screen() -> io::Result<()> {
    execute!(io::stdout(), cursor::Show, terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()
}

async fn dashboard_loop(store: &SkoposStore, cfg: &Config) -> anyhow::Result<()> {
    let mut headline = headline_default(cfg);
    loop {
        let dash = gather(store, cfg, headline).await?;
        draw(&render(&dash))?;

        // Block up to a second waiting for a key; a timeout just re-renders.
        let event = tokio::task::spawn_blocking(|| -> io::Result<Option<Event>> {
            if event::poll(StdDuration::from_millis(1000))? {
                Ok(Some(event::read()?))
            } else {
                Ok(None)
            }
        })
        .await??;

        if let Some(Event::Key(key)) = event {
            if key.kind != KeyEventKind::Release {
                match (key.code, key.modifiers) {
                    (KeyCode::Char('c'), KeyModifiers::CONTROL)
                    | (KeyCode::Esc, _)
                    | (KeyCode::Char('q'), _) => return Ok(()),
                    (KeyCode::Char('1'), _) => headline = Duration::hours(1),
                    (KeyCode::Char('2'), _) => headline = Duration::days(1),
                    (KeyCode::Char('3'), _) => headline = Duration::days(7),
                    _ => {}
                }
            }
        }
    }
}

fn draw(lines: &[String]) -> io::Result<()> {
    let mut out = io::stdout();
    queue!(out, Clear(ClearType::All), cursor::MoveTo(0, 0))?;
    for line in lines {
        queue!(out, Print(line), Print("\r\n"))?;
    }
    out.flush()
}

// ===========================================================================
// Data gathering
// ===========================================================================

/// One rolling window's row in the dashboard table.
struct WindowRow {
    label: &'static str,
    outages: i64,
    downtime_secs: i64,
    availability: f64,
    worst_rtt_ms: Option<f64>,
}

/// Everything one frame of the dashboard needs.
struct Dashboard {
    now: DateTime<Utc>,
    hostname: String,
    iface: String,
    latest: Option<NetworkSample>,
    report: HealthReport,
    minute_cells: Vec<Cell>,
    windows: Vec<WindowRow>,
    recent_outages: Vec<NetworkOutage>,
    daemon_fresh: bool,
}

async fn gather(
    store: &SkoposStore,
    cfg: &Config,
    headline: Duration,
) -> anyhow::Result<Dashboard> {
    let now = Utc::now();
    let net = &cfg.network;

    let latest = store.latest_network_sample().await?;
    let iface = net
        .iface
        .clone()
        .or_else(|| latest.as_ref().and_then(|s| s.iface.clone()))
        .or_else(super::probe::detect_iface)
        .unwrap_or_else(|| "—".to_string());

    // Headline verdict over the (toggleable) headline window.
    let headline_start = now - headline;
    let counts = store.network_status_counts_since(headline_start).await?;
    let headline_outages = store.network_outages_since(headline_start).await?;
    let thresholds = HealthThresholds {
        moderate_outages: net.moderate_outages,
        severe_outages: net.severe_outages,
        moderate_downtime_secs: net.moderate_downtime_secs,
        severe_downtime_secs: net.severe_downtime_secs,
        moderate_degraded_ratio: net.moderate_degraded_ratio,
    };
    let report = health::assess(
        now,
        headline,
        &headline_outages,
        counts.degraded,
        counts.total(),
        &thresholds,
    );

    // Sparkline: always the last 60 one-minute cells.
    let samples_1h = store
        .network_samples_since(now - Duration::hours(1))
        .await?;
    let minute_cells = bucket_minutes(&samples_1h, now);

    // The 7-day outage list feeds both the windows table and the list.
    let outages_7d = store.network_outages_since(now - Duration::days(7)).await?;
    let mut windows = Vec::new();
    for (label, window) in [
        ("1 hour", Duration::hours(1)),
        ("24 hours", Duration::days(1)),
        ("7 days", Duration::days(7)),
    ] {
        let start = now - window;
        let in_window: Vec<&NetworkOutage> = outages_7d
            .iter()
            .filter(|o| o.ended_at.map(|end| end >= start).unwrap_or(true))
            .collect();
        let downtime: i64 = in_window
            .iter()
            .map(|o| health::outage_overlap_secs(o, start, now))
            .sum();
        let window_secs = window.num_seconds().max(1);
        let availability = ((window_secs - downtime).max(0) as f64 / window_secs as f64) * 100.0;
        windows.push(WindowRow {
            label,
            outages: in_window.len() as i64,
            downtime_secs: downtime,
            availability,
            worst_rtt_ms: store.network_worst_rtt_since(start).await?,
        });
    }

    let recent_outages: Vec<NetworkOutage> = outages_7d.iter().take(5).cloned().collect();

    // The daemon is "fresh" if a sample landed within a few probe intervals.
    let daemon_fresh = latest
        .as_ref()
        .map(|s| (now - s.ts).num_seconds() < net.interval_secs as i64 * 3 + 10)
        .unwrap_or(false);

    Ok(Dashboard {
        now,
        hostname: hostname(),
        iface,
        latest,
        report,
        minute_cells,
        windows,
        recent_outages,
        daemon_fresh,
    })
}

// ===========================================================================
// Timeline sparkline
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cell {
    Empty,
    Ok,
    Degraded,
    Down,
}

impl Cell {
    fn from_status(status: &str) -> Cell {
        match status {
            "down" => Cell::Down,
            "degraded" => Cell::Degraded,
            "ok" => Cell::Ok,
            _ => Cell::Empty,
        }
    }

    fn rank(self) -> u8 {
        match self {
            Cell::Empty => 0,
            Cell::Ok => 1,
            Cell::Degraded => 2,
            Cell::Down => 3,
        }
    }

    /// Keep whichever cell is the worse of the two.
    fn worst(self, other: Cell) -> Cell {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }
}

/// Fold samples into 60 one-minute cells, oldest on the left. Each cell
/// shows the worst status seen in that minute.
fn bucket_minutes(samples: &[NetworkSample], now: DateTime<Utc>) -> Vec<Cell> {
    let mut cells = vec![Cell::Empty; SPARK_CELLS as usize];
    for sample in samples {
        let age_min = (now - sample.ts).num_minutes();
        if !(0..SPARK_CELLS).contains(&age_min) {
            continue;
        }
        let idx = (SPARK_CELLS - 1 - age_min) as usize;
        cells[idx] = cells[idx].worst(Cell::from_status(&sample.status));
    }
    cells
}

fn sparkline(cells: &[Cell]) -> String {
    let mut out = String::new();
    for cell in cells {
        match cell {
            Cell::Empty => out.push_str(&dim("·")),
            Cell::Ok => out.push_str(&rgb("█", GREEN)),
            Cell::Degraded => out.push_str(&rgb("█", AMBER)),
            Cell::Down => out.push_str(&rgb("█", RED)),
        }
    }
    out
}

// ===========================================================================
// Rendering
// ===========================================================================

fn render(dash: &Dashboard) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    lines.push(format!(
        "  {}    {}",
        purple_bold("skopos network"),
        dim(&format!("{} · {}", dash.hostname, dash.iface)),
    ));
    lines.push(String::new());

    if dash.latest.is_none() {
        lines.push(dim("  No connectivity data recorded yet."));
        lines.push(String::new());
        lines.push(dim("  Install the probe daemon (per-user, no root):"));
        lines.push("    skopos network install".to_string());
        lines.push(String::new());
        lines.push(dim("  or run it directly in another terminal:"));
        lines.push("    skopos network watch".to_string());
        lines.push(String::new());
        lines.push(dim("  q quit"));
        return lines;
    }

    let color = dash.report.health.color();
    lines.push(format!(
        "  {}  {}    {}",
        rgb("●", color),
        rgb_bold(dash.report.health.label(), color),
        dim(&dash.report.reason),
    ));
    lines.push(String::new());

    // Current link.
    lines.push(purple("  Current link"));
    if let Some(sample) = &dash.latest {
        let rtt = sample
            .rtt_ms
            .map(fmt_rtt)
            .unwrap_or_else(|| "—".to_string());
        let carrier = match sample.carrier {
            Some(true) => "up",
            Some(false) => "down",
            None => "—",
        };
        lines.push(format!(
            "    latency {:<9}  loss {:<5}  sites {}/{}  carrier {}",
            rtt,
            format!("{:.0}%", sample.loss_pct),
            sample.sites_ok,
            sample.sites_total,
            carrier,
        ));
        let daemon = if dash.daemon_fresh {
            "running".to_string()
        } else {
            "not detected — history may have gaps".to_string()
        };
        lines.push(dim(&format!(
            "    last probe {}   ·   daemon {}",
            humanise_relative_past(sample.ts, dash.now),
            daemon,
        )));
    }
    lines.push(String::new());

    // Timeline sparkline.
    lines.push(format!(
        "  {}   {}",
        purple("Last 60 min"),
        dim("one cell ≈ 1 min, oldest left"),
    ));
    lines.push(format!("    {}", sparkline(&dash.minute_cells)));
    lines.push(String::new());

    // Windows table.
    lines.push(purple(&format!(
        "  {:<10} {:>7}  {:>9}   {:<17} {:>9}",
        "window", "outages", "downtime", "availability", "worst rtt",
    )));
    for window in &dash.windows {
        let bar = rgb(
            &progress_bar(window.availability, 12),
            availability_color(window.availability),
        );
        lines.push(format!(
            "  {:<10} {:>7}  {:>9}   {} {:>3.0}%  {:>9}",
            window.label,
            window.outages,
            health::human_secs(window.downtime_secs),
            bar,
            window.availability,
            window
                .worst_rtt_ms
                .map(fmt_rtt)
                .unwrap_or_else(|| "—".to_string()),
        ));
    }
    lines.push(String::new());

    // Recent outages.
    lines.push(purple("  Recent outages"));
    if dash.recent_outages.is_empty() {
        lines.push(dim("    none in the last 7 days"));
    } else {
        for outage in &dash.recent_outages {
            let when = outage
                .started_at
                .with_timezone(&Local)
                .format("%m-%d %H:%M:%S");
            let duration = match outage.duration_secs {
                Some(secs) => health::human_secs(secs),
                None => "ongoing".to_string(),
            };
            let cause = outage
                .cause
                .clone()
                .unwrap_or_else(|| "unreachable".to_string());
            let row = format!("    {when}   {duration:>8}   {cause}");
            if outage.ended_at.is_none() {
                lines.push(rgb(&row, color));
            } else {
                lines.push(row);
            }
        }
    }
    lines.push(String::new());
    lines.push(dim(
        "  r refresh  ·  1 hour  ·  2 day  ·  3 week  ·  q quit",
    ));

    lines
}

/// One-shot text report for `skopos network status` (and non-TTY runs).
fn render_status_text(dash: &Dashboard) -> String {
    let mut out = String::new();
    out.push_str(&purple_bold("Network status"));
    out.push('\n');

    let Some(sample) = &dash.latest else {
        out.push_str(&dim(
            "  no connectivity data yet — run `skopos network watch`\n",
        ));
        return out;
    };

    let color = dash.report.health.color();
    out.push_str(&format!(
        "  {}  {}\n",
        rgb_bold(dash.report.health.label(), color),
        dim(&dash.report.reason),
    ));
    let rtt = sample
        .rtt_ms
        .map(fmt_rtt)
        .unwrap_or_else(|| "—".to_string());
    out.push_str(&dim(&format!(
        "  latency {} · loss {:.0}% · {}/{} sites · updated {}\n",
        rtt,
        sample.loss_pct,
        sample.sites_ok,
        sample.sites_total,
        humanise_relative_past(sample.ts, dash.now),
    )));
    out
}

fn fmt_rtt(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.1} s", ms / 1000.0)
    } else {
        format!("{ms:.0} ms")
    }
}

fn availability_color(pct: f64) -> (u8, u8, u8) {
    if pct >= 99.5 {
        GREEN
    } else if pct >= 98.0 {
        AMBER
    } else {
        RED
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_at(minutes_ago: i64, status: &str) -> NetworkSample {
        NetworkSample {
            ts: Utc.with_ymd_and_hms(2026, 5, 20, 12, 0, 0).unwrap()
                - Duration::minutes(minutes_ago),
            status: status.to_string(),
            rtt_ms: Some(20.0),
            loss_pct: 0.0,
            sites_ok: 3,
            sites_total: 3,
            iface: Some("eth0".to_string()),
            carrier: Some(true),
        }
    }

    #[test]
    fn bucket_minutes_places_cells_oldest_left() {
        let now = Utc.with_ymd_and_hms(2026, 5, 20, 12, 0, 0).unwrap();
        let samples = vec![sample_at(59, "ok"), sample_at(0, "down")];
        let cells = bucket_minutes(&samples, now);
        assert_eq!(cells.len(), 60);
        assert_eq!(cells[0], Cell::Ok);
        assert_eq!(cells[59], Cell::Down);
    }

    #[test]
    fn bucket_minutes_keeps_the_worst_status_per_minute() {
        let now = Utc.with_ymd_and_hms(2026, 5, 20, 12, 0, 0).unwrap();
        let samples = vec![sample_at(5, "ok"), sample_at(5, "degraded")];
        let cells = bucket_minutes(&samples, now);
        assert_eq!(cells[54], Cell::Degraded);
    }

    #[test]
    fn fmt_rtt_switches_to_seconds_above_a_second() {
        assert_eq!(fmt_rtt(28.0), "28 ms");
        assert_eq!(fmt_rtt(1200.0), "1.2 s");
    }
}
