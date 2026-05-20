//! One connectivity probe tick.
//!
//! Each tick shells out to the system `ping` binary against a set of
//! websites — Ubuntu's `ping` already carries `cap_net_raw`, so the daemon
//! needs no privileges or capability wrangling of its own. The per-target
//! results are folded together with the local interface carrier (read
//! straight from sysfs) into a single `ok` / `degraded` / `down` verdict.

use std::fs;

use tokio::process::Command;
use tokio::task::JoinSet;

use crate::config::NetworkConfig;

/// Per-tick classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TickStatus {
    Ok,
    Degraded,
    Down,
}

impl TickStatus {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            TickStatus::Ok => "ok",
            TickStatus::Degraded => "degraded",
            TickStatus::Down => "down",
        }
    }
}

/// Probe parameters distilled from `NetworkConfig`.
#[derive(Debug, Clone)]
pub(super) struct ProbeConfig {
    pub targets: Vec<String>,
    pub ping_count: u32,
    pub iface: Option<String>,
    pub degraded_rtt_ms: f64,
    pub degraded_loss_pct: f64,
}

impl ProbeConfig {
    pub(super) fn from_network(cfg: &NetworkConfig) -> Self {
        Self {
            targets: cfg.targets.clone(),
            ping_count: cfg.ping_count.max(1),
            iface: cfg.iface.clone(),
            degraded_rtt_ms: cfg.degraded_rtt_ms,
            degraded_loss_pct: cfg.degraded_loss_pct,
        }
    }
}

/// Outcome of one probe tick.
#[derive(Debug, Clone)]
pub(super) struct ProbeResult {
    pub status: TickStatus,
    pub rtt_ms: Option<f64>,
    pub loss_pct: f64,
    pub sites_ok: usize,
    pub sites_total: usize,
    pub iface: Option<String>,
    pub carrier: Option<bool>,
    /// Failure cause — set only when `status == Down`.
    pub cause: Option<String>,
}

/// Result of pinging one host.
#[derive(Debug, Clone)]
struct TargetOutcome {
    responded: bool,
    loss_pct: f64,
    rtt_ms: Option<f64>,
    dns_failure: bool,
}

/// Parsed `ping` statistics block.
#[derive(Debug, Clone, PartialEq)]
struct PingStats {
    received: u32,
    loss_pct: f64,
    avg_rtt_ms: Option<f64>,
}

/// Run one probe tick: ping every target concurrently, read the carrier,
/// and classify.
pub(super) async fn probe_once(cfg: &ProbeConfig) -> ProbeResult {
    let iface = cfg.iface.clone().or_else(detect_iface);
    let carrier = iface.as_deref().and_then(read_carrier);

    let sites_total = cfg.targets.len();
    let mut set: JoinSet<TargetOutcome> = JoinSet::new();
    for host in &cfg.targets {
        let host = host.clone();
        let count = cfg.ping_count;
        set.spawn(async move { ping_target(&host, count).await });
    }

    let mut outcomes: Vec<TargetOutcome> = Vec::with_capacity(sites_total);
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(outcome) => outcomes.push(outcome),
            // A panicked probe task is treated as an unreachable target.
            Err(_) => outcomes.push(TargetOutcome {
                responded: false,
                loss_pct: 100.0,
                rtt_ms: None,
                dns_failure: false,
            }),
        }
    }

    classify(&outcomes, sites_total, iface, carrier, cfg)
}

/// Fold per-target outcomes into a single verdict.
fn classify(
    outcomes: &[TargetOutcome],
    sites_total: usize,
    iface: Option<String>,
    carrier: Option<bool>,
    cfg: &ProbeConfig,
) -> ProbeResult {
    let sites_ok = outcomes.iter().filter(|o| o.responded).count();

    // The link is only `down` when every target failed — one site or CDN
    // hiccup must not register as an internet outage.
    if sites_ok == 0 {
        let all_dns = !outcomes.is_empty() && outcomes.iter().all(|o| o.dns_failure);
        let cause = if all_dns {
            "dns"
        } else if carrier == Some(false) {
            "no-carrier"
        } else {
            "unreachable"
        };
        return ProbeResult {
            status: TickStatus::Down,
            rtt_ms: None,
            loss_pct: 100.0,
            sites_ok: 0,
            sites_total,
            iface,
            carrier,
            cause: Some(cause.to_string()),
        };
    }

    // Best (lowest) RTT among responders is the cleanest reading of the link.
    let rtt_ms = outcomes
        .iter()
        .filter_map(|o| o.rtt_ms)
        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Mean packet loss across every target attempted.
    let loss_pct = if outcomes.is_empty() {
        0.0
    } else {
        outcomes.iter().map(|o| o.loss_pct).sum::<f64>() / outcomes.len() as f64
    };

    let degraded = loss_pct >= cfg.degraded_loss_pct
        || rtt_ms.is_some_and(|rtt| rtt >= cfg.degraded_rtt_ms)
        || sites_ok < sites_total;

    ProbeResult {
        status: if degraded {
            TickStatus::Degraded
        } else {
            TickStatus::Ok
        },
        rtt_ms,
        loss_pct,
        sites_ok,
        sites_total,
        iface,
        carrier,
        cause: None,
    }
}

/// Ping one host with the system `ping` binary.
async fn ping_target(host: &str, count: u32) -> TargetOutcome {
    // `-w` bounds the whole call so a dead host can't stall a tick.
    let deadline = (u64::from(count) + 4).to_string();
    let output = Command::new("ping")
        .arg("-c")
        .arg(count.to_string())
        .arg("-W")
        .arg("2")
        .arg("-w")
        .arg(&deadline)
        .arg("-n")
        .arg(host)
        .output()
        .await;

    let output = match output {
        Ok(output) => output,
        // `ping` missing or unspawnable — treat the target as unreachable.
        Err(_) => {
            return TargetOutcome {
                responded: false,
                loss_pct: 100.0,
                rtt_ms: None,
                dns_failure: false,
            }
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    match parse_ping_stats(&stdout) {
        Some(stats) => TargetOutcome {
            responded: stats.received > 0,
            loss_pct: stats.loss_pct,
            rtt_ms: stats.avg_rtt_ms,
            dns_failure: false,
        },
        None => TargetOutcome {
            responded: false,
            loss_pct: 100.0,
            rtt_ms: None,
            dns_failure: is_dns_failure(&stderr),
        },
    }
}

/// Whether `ping`'s stderr names a DNS resolution failure.
fn is_dns_failure(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("name resolution")
        || lower.contains("name or service not known")
        || lower.contains("unknown host")
        || lower.contains("no address associated")
}

/// Parse the `ping` statistics block. Returns `None` when no summary line
/// is present (host never resolved, or `ping` failed before pinging).
fn parse_ping_stats(stdout: &str) -> Option<PingStats> {
    let summary = stdout
        .lines()
        .find(|line| line.contains("packets transmitted"))?;

    let mut received = 0u32;
    let mut loss_pct = 100.0f64;
    for segment in summary.split(',') {
        let seg = segment.trim();
        if let Some(rest) = seg.strip_suffix("received") {
            received = rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
        } else if seg.contains("packet loss") {
            if let Some(token) = seg.split_whitespace().find(|t| t.ends_with('%')) {
                loss_pct = token.trim_end_matches('%').parse().unwrap_or(100.0);
            }
        }
    }

    // `rtt min/avg/max/mdev = 8.420/8.766/9.110/0.281 ms` — take the avg.
    let avg_rtt_ms = stdout
        .lines()
        .find(|line| line.contains("min/avg/max"))
        .and_then(|line| line.split('=').nth(1))
        .and_then(|rhs| rhs.split_whitespace().next())
        .and_then(|nums| nums.split('/').nth(1))
        .and_then(|avg| avg.parse().ok());

    Some(PingStats {
        received,
        loss_pct,
        avg_rtt_ms,
    })
}

/// Read `/sys/class/net/<iface>/carrier`, falling back to `operstate`.
fn read_carrier(iface: &str) -> Option<bool> {
    let base = format!("/sys/class/net/{iface}");
    if let Ok(raw) = fs::read_to_string(format!("{base}/carrier")) {
        match raw.trim() {
            "1" => return Some(true),
            "0" => return Some(false),
            _ => {}
        }
    }
    match fs::read_to_string(format!("{base}/operstate")) {
        Ok(state) => match state.trim() {
            "up" => Some(true),
            "down" => Some(false),
            _ => None,
        },
        Err(_) => None,
    }
}

/// Pick a likely primary interface: the first non-loopback interface,
/// preferring one whose `operstate` is `up`.
pub(super) fn detect_iface() -> Option<String> {
    let entries = fs::read_dir("/sys/class/net").ok()?;
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name != "lo")
        .collect();
    names.sort();
    names
        .iter()
        .find(|name| {
            fs::read_to_string(format!("/sys/class/net/{name}/operstate"))
                .map(|state| state.trim() == "up")
                .unwrap_or(false)
        })
        .or_else(|| names.first())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ProbeConfig {
        ProbeConfig {
            targets: vec!["example.test".to_string()],
            ping_count: 3,
            iface: None,
            degraded_rtt_ms: 250.0,
            degraded_loss_pct: 5.0,
        }
    }

    #[test]
    fn parses_a_successful_ping() {
        let out = "PING cloudflare.com (104.16.132.229) 56(84) bytes of data.\n\
                   64 bytes from 104.16.132.229: icmp_seq=1 ttl=57 time=8.42 ms\n\
                   \n--- cloudflare.com ping statistics ---\n\
                   3 packets transmitted, 3 received, 0% packet loss, time 2003ms\n\
                   rtt min/avg/max/mdev = 8.420/8.766/9.110/0.281 ms\n";
        let stats = parse_ping_stats(out).unwrap();
        assert_eq!(stats.received, 3);
        assert_eq!(stats.loss_pct, 0.0);
        assert_eq!(stats.avg_rtt_ms, Some(8.766));
    }

    #[test]
    fn parses_total_packet_loss() {
        let out = "PING down.test (10.0.0.1) 56(84) bytes of data.\n\
                   --- down.test ping statistics ---\n\
                   5 packets transmitted, 0 received, 100% packet loss, time 4090ms\n";
        let stats = parse_ping_stats(out).unwrap();
        assert_eq!(stats.received, 0);
        assert_eq!(stats.loss_pct, 100.0);
        assert_eq!(stats.avg_rtt_ms, None);
    }

    #[test]
    fn no_summary_line_yields_none() {
        assert!(parse_ping_stats("ping: bad: Temporary failure\n").is_none());
    }

    #[test]
    fn detects_dns_failure_from_stderr() {
        assert!(is_dns_failure(
            "ping: nope.invalid: Temporary failure in name resolution"
        ));
        assert!(!is_dns_failure("64 bytes from 1.1.1.1: icmp_seq=1"));
    }

    #[test]
    fn classify_down_when_every_target_fails() {
        let outcomes = vec![TargetOutcome {
            responded: false,
            loss_pct: 100.0,
            rtt_ms: None,
            dns_failure: false,
        }];
        let result = classify(&outcomes, 1, Some("eth0".into()), Some(true), &cfg());
        assert_eq!(result.status, TickStatus::Down);
        assert_eq!(result.cause.as_deref(), Some("unreachable"));
    }

    #[test]
    fn classify_down_tags_dns_when_all_targets_fail_resolution() {
        let outcomes = vec![TargetOutcome {
            responded: false,
            loss_pct: 100.0,
            rtt_ms: None,
            dns_failure: true,
        }];
        let result = classify(&outcomes, 1, None, Some(true), &cfg());
        assert_eq!(result.cause.as_deref(), Some("dns"));
    }

    #[test]
    fn classify_degraded_on_high_latency() {
        let outcomes = vec![TargetOutcome {
            responded: true,
            loss_pct: 0.0,
            rtt_ms: Some(400.0),
            dns_failure: false,
        }];
        let result = classify(&outcomes, 1, None, None, &cfg());
        assert_eq!(result.status, TickStatus::Degraded);
    }

    #[test]
    fn classify_ok_when_link_is_healthy() {
        let outcomes = vec![TargetOutcome {
            responded: true,
            loss_pct: 0.0,
            rtt_ms: Some(20.0),
            dns_failure: false,
        }];
        let result = classify(&outcomes, 1, None, None, &cfg());
        assert_eq!(result.status, TickStatus::Ok);
        assert_eq!(result.rtt_ms, Some(20.0));
    }
}
