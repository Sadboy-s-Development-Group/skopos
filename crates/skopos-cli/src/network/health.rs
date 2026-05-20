//! Pure classification of recorded connectivity into a stable / moderate /
//! severe verdict over a rolling window.
//!
//! Kept free of I/O so the decision logic is exhaustively unit-testable —
//! the caller hands in the outages overlapping the window plus the sample
//! status counts inside it.

use chrono::{DateTime, Duration, Utc};
use skopos_store::NetworkOutage;

/// The headline verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Health {
    Stable,
    Moderate,
    Severe,
}

impl Health {
    pub(super) fn label(self) -> &'static str {
        match self {
            Health::Stable => "STABLE",
            Health::Moderate => "MODERATE",
            Health::Severe => "SEVERE",
        }
    }

    /// Verdict colour: green / amber / red.
    pub(super) fn color(self) -> (u8, u8, u8) {
        match self {
            Health::Stable => (78, 201, 109),
            Health::Moderate => (224, 165, 48),
            Health::Severe => (232, 86, 76),
        }
    }

    /// Process exit code for `skopos network status`.
    pub(super) fn exit_code(self) -> i32 {
        match self {
            Health::Stable => 0,
            Health::Moderate => 1,
            Health::Severe => 2,
        }
    }
}

/// Tunables for the verdict, sourced from `NetworkConfig`.
#[derive(Debug, Clone)]
pub(super) struct HealthThresholds {
    pub moderate_outages: i64,
    pub severe_outages: i64,
    pub moderate_downtime_secs: i64,
    pub severe_downtime_secs: i64,
    pub moderate_degraded_ratio: f64,
}

/// The verdict plus a human sentence explaining which rule fired.
#[derive(Debug, Clone)]
pub(super) struct HealthReport {
    pub health: Health,
    pub reason: String,
}

/// Classify the window `[now - window, now]`.
///
/// `outages` must be the outages overlapping that window; `degraded_ticks`
/// and `total_ticks` the sample-status counts inside it.
pub(super) fn assess(
    now: DateTime<Utc>,
    window: Duration,
    outages: &[NetworkOutage],
    degraded_ticks: i64,
    total_ticks: i64,
    th: &HealthThresholds,
) -> HealthReport {
    let window_start = now - window;
    let down_secs: i64 = outages
        .iter()
        .map(|outage| outage_overlap_secs(outage, window_start, now))
        .sum();
    let outage_count = outages.len() as i64;
    let currently_down = outages.iter().any(|outage| outage.ended_at.is_none());
    let degraded_ratio = if total_ticks > 0 {
        degraded_ticks as f64 / total_ticks as f64
    } else {
        0.0
    };

    let win = window_label(window);

    let (health, reason) = if currently_down {
        (Health::Severe, "internet is down right now".to_string())
    } else if outage_count >= th.severe_outages {
        (
            Health::Severe,
            format!("{outage_count} interruptions in the last {win}"),
        )
    } else if down_secs >= th.severe_downtime_secs {
        (
            Health::Severe,
            format!("{} of downtime in the last {win}", human_secs(down_secs)),
        )
    } else if outage_count >= th.moderate_outages {
        (
            Health::Moderate,
            format!("{outage_count} {} in the last {win}", plural(outage_count),),
        )
    } else if degraded_ratio >= th.moderate_degraded_ratio {
        (
            Health::Moderate,
            format!(
                "link degraded {:.0}% of the last {win}",
                degraded_ratio * 100.0,
            ),
        )
    } else if down_secs >= th.moderate_downtime_secs {
        (
            Health::Moderate,
            format!("{} of downtime in the last {win}", human_secs(down_secs)),
        )
    } else {
        (
            Health::Stable,
            format!("no interruptions in the last {win}"),
        )
    };

    HealthReport { health, reason }
}

/// Seconds of `outage` that fall inside `[window_start, now]`.
pub(super) fn outage_overlap_secs(
    outage: &NetworkOutage,
    window_start: DateTime<Utc>,
    now: DateTime<Utc>,
) -> i64 {
    let start = outage.started_at.max(window_start);
    let end = outage.ended_at.unwrap_or(now).min(now);
    (end - start).num_seconds().max(0)
}

fn window_label(window: Duration) -> String {
    let mins = window.num_minutes();
    match mins {
        60 => "hour".to_string(),
        1440 => "24 hours".to_string(),
        m if m > 0 && m % 1440 == 0 => format!("{} days", m / 1440),
        m if m > 0 && m % 60 == 0 => format!("{} hours", m / 60),
        m => format!("{m} min"),
    }
}

/// Compact duration, e.g. `9s`, `1m 48s`, `2h 19m`.
pub(super) fn human_secs(secs: i64) -> String {
    if secs < 60 {
        return format!("{secs}s");
    }
    let minutes = secs / 60;
    let seconds = secs % 60;
    if minutes < 60 {
        return format!("{minutes}m {seconds:02}s");
    }
    let hours = minutes / 60;
    let minutes = minutes % 60;
    format!("{hours}h {minutes:02}m")
}

fn plural(n: i64) -> &'static str {
    if n == 1 {
        "interruption"
    } else {
        "interruptions"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn outage(start: &str, end: Option<&str>) -> NetworkOutage {
        NetworkOutage {
            id: 1,
            started_at: DateTime::parse_from_rfc3339(start)
                .unwrap()
                .with_timezone(&Utc),
            ended_at: end.map(|e| DateTime::parse_from_rfc3339(e).unwrap().with_timezone(&Utc)),
            duration_secs: None,
            down_samples: 1,
            cause: None,
        }
    }

    fn thresholds() -> HealthThresholds {
        HealthThresholds {
            moderate_outages: 1,
            severe_outages: 4,
            moderate_downtime_secs: 30,
            severe_downtime_secs: 300,
            moderate_degraded_ratio: 0.20,
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 20, 12, 0, 0).unwrap()
    }

    #[test]
    fn stable_when_quiet() {
        let report = assess(now(), Duration::hours(1), &[], 0, 360, &thresholds());
        assert_eq!(report.health, Health::Stable);
        assert!(report.reason.contains("no interruptions"));
    }

    #[test]
    fn moderate_on_a_single_outage() {
        let outages = [outage("2026-05-20T11:30:00Z", Some("2026-05-20T11:30:20Z"))];
        let report = assess(now(), Duration::hours(1), &outages, 0, 360, &thresholds());
        assert_eq!(report.health, Health::Moderate);
        assert!(report.reason.contains("1 interruption "));
    }

    #[test]
    fn moderate_on_sustained_degradation() {
        let report = assess(now(), Duration::hours(1), &[], 120, 360, &thresholds());
        assert_eq!(report.health, Health::Moderate);
        assert!(report.reason.contains("degraded"));
    }

    #[test]
    fn severe_when_currently_down() {
        let outages = [outage("2026-05-20T11:59:00Z", None)];
        let report = assess(now(), Duration::hours(1), &outages, 0, 360, &thresholds());
        assert_eq!(report.health, Health::Severe);
        assert!(report.reason.contains("down right now"));
    }

    #[test]
    fn severe_on_many_outages() {
        let outages: Vec<NetworkOutage> = (0..4)
            .map(|i| {
                let m = 10 + i * 5;
                outage(
                    &format!("2026-05-20T11:{m:02}:00Z"),
                    Some(&format!("2026-05-20T11:{m:02}:05Z")),
                )
            })
            .collect();
        let report = assess(now(), Duration::hours(1), &outages, 0, 360, &thresholds());
        assert_eq!(report.health, Health::Severe);
    }

    #[test]
    fn overlap_clips_to_the_window() {
        // Outage started before the window; only the in-window part counts.
        let o = outage("2026-05-20T10:50:00Z", Some("2026-05-20T11:10:00Z"));
        let secs = outage_overlap_secs(&o, now() - Duration::hours(1), now());
        assert_eq!(secs, 600); // 11:00 → 11:10
    }
}
