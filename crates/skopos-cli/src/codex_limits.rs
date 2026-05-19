//! Codex (ChatGPT) rate-limit snapshot, fetched over JSON-RPC from
//! `codex app-server`.
//!
//! Unlike Anthropic — where the official statusline hook is the only
//! sanctioned escape valve — Codex CLI ships an app-server that speaks
//! newline-delimited JSON-RPC on stdin/stdout. We drive a tiny handshake
//! (`initialize` + `account/rateLimits/read`), parse the user's 5h /
//! weekly window percentages, and persist a snapshot. The server may
//! interleave unsolicited notifications (e.g. `remoteControl/status/changed`);
//! we ignore any line without a matching `id`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::timeout;

use crate::limits::{home_dir, humanise_relative_future, progress_bar};
use crate::{dim, purple, purple_bold};

const BAR_WIDTH: usize = 24;
const FETCH_TIMEOUT: StdDuration = StdDuration::from_secs(5);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct CodexSnapshot {
    pub provider: String,
    pub plan_type: Option<String>,
    pub limit_id: Option<String>,
    pub received_at: DateTime<Utc>,
    pub primary: Option<CodexWindow>,
    pub secondary: Option<CodexWindow>,
    pub credits: Option<CodexCredits>,
    pub rate_limit_reached_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct CodexWindow {
    pub used_percent: f64,
    #[serde(default, with = "unix_epoch_seconds_opt")]
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub window_duration_mins: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct CodexCredits {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<String>,
}

mod unix_epoch_seconds_opt {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(when: &Option<DateTime<Utc>>, s: S) -> Result<S::Ok, S::Error> {
        match when {
            Some(when) => when.timestamp().serialize(s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<DateTime<Utc>>, D::Error> {
        let secs: Option<i64> = Option::deserialize(d)?;
        match secs {
            Some(s) => Ok(Some(DateTime::from_timestamp(s, 0).ok_or_else(|| {
                serde::de::Error::custom(format!("invalid unix timestamp: {s}"))
            })?)),
            None => Ok(None),
        }
    }
}

// ===========================================================================
// On-disk paths
// ===========================================================================

pub(crate) fn codex_snapshot_path() -> PathBuf {
    home_dir()
        .join(".local")
        .join("share")
        .join("skopos")
        .join("codex-session.json")
}

// ===========================================================================
// Snapshot I/O
// ===========================================================================

pub(crate) fn load_codex_snapshot(path: &Path) -> anyhow::Result<Option<CodexSnapshot>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("reading codex snapshot at {}", path.display()))?;
    let snapshot = serde_json::from_str(&raw)
        .map_err(|err| anyhow::anyhow!("failed to parse {}: {err}", path.display()))?;
    Ok(Some(snapshot))
}

pub(crate) fn save_codex_snapshot(path: &Path, snap: &CodexSnapshot) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(snap)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, body).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} → {}", tmp.display(), path.display()))?;
    Ok(())
}

// ===========================================================================
// Wire format
// ===========================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetAccountRateLimitsResponse {
    rate_limits: RateLimitSnapshotWire,
    #[serde(default)]
    #[allow(dead_code)]
    rate_limits_by_limit_id: Option<std::collections::BTreeMap<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitSnapshotWire {
    #[serde(default)]
    limit_id: Option<String>,
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    primary: Option<RateWindowWire>,
    #[serde(default)]
    secondary: Option<RateWindowWire>,
    #[serde(default)]
    credits: Option<CreditsWire>,
    #[serde(default)]
    rate_limit_reached_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateWindowWire {
    used_percent: f64,
    #[serde(default)]
    resets_at: Option<i64>,
    #[serde(default)]
    window_duration_mins: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreditsWire {
    has_credits: bool,
    unlimited: bool,
    #[serde(default)]
    balance: Option<String>,
}

impl RateWindowWire {
    fn into_window(self) -> CodexWindow {
        CodexWindow {
            used_percent: self.used_percent,
            resets_at: self.resets_at.and_then(|s| DateTime::from_timestamp(s, 0)),
            window_duration_mins: self.window_duration_mins,
        }
    }
}

impl CreditsWire {
    fn into_credits(self) -> CodexCredits {
        CodexCredits {
            has_credits: self.has_credits,
            unlimited: self.unlimited,
            balance: self.balance,
        }
    }
}

fn wire_to_snapshot(
    wire: GetAccountRateLimitsResponse,
    received_at: DateTime<Utc>,
) -> CodexSnapshot {
    let rl = wire.rate_limits;
    CodexSnapshot {
        provider: "openai".to_string(),
        plan_type: rl.plan_type,
        limit_id: rl.limit_id,
        received_at,
        primary: rl.primary.map(RateWindowWire::into_window),
        secondary: rl.secondary.map(RateWindowWire::into_window),
        credits: rl.credits.map(CreditsWire::into_credits),
        rate_limit_reached_type: rl.rate_limit_reached_type,
    }
}

// ===========================================================================
// JSON-RPC handshake
// ===========================================================================

pub(crate) async fn fetch_codex_snapshot() -> anyhow::Result<CodexSnapshot> {
    timeout(FETCH_TIMEOUT, fetch_inner())
        .await
        .map_err(|_| anyhow::anyhow!("timed out fetching Codex rate limits after 5s"))?
}

async fn fetch_inner() -> anyhow::Result<CodexSnapshot> {
    let mut child = tokio::process::Command::new("codex")
        .args(["app-server"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!("`codex` binary not found on PATH — install Codex CLI first")
            } else {
                anyhow::Error::new(err).context("spawning codex app-server")
            }
        })?;

    let mut stdin = child
        .stdin
        .take()
        .context("opening codex app-server stdin")?;
    let stdout = child
        .stdout
        .take()
        .context("opening codex app-server stdout")?;

    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"skopos","version":"0.1.0"},"capabilities":{"experimentalApi":true}}}"#;
    let read_rl = r#"{"jsonrpc":"2.0","id":2,"method":"account/rateLimits/read","params":null}"#;

    stdin
        .write_all(init.as_bytes())
        .await
        .context("writing initialize to codex app-server")?;
    stdin.write_all(b"\n").await?;
    stdin
        .write_all(read_rl.as_bytes())
        .await
        .context("writing rateLimits/read to codex app-server")?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await?;

    let mut reader = BufReader::new(stdout).lines();
    let mut snapshot: Option<CodexSnapshot> = None;

    while let Some(line) = reader
        .next_line()
        .await
        .context("reading codex app-server stdout")?
    {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        // Notifications have no `id`; responses for other requests don't
        // carry id=2. Either way, ignore.
        let id = value.get("id").and_then(|v| v.as_i64());
        if id != Some(2) {
            continue;
        }
        let result = value
            .get("result")
            .cloned()
            .ok_or_else(|| match value.get("error") {
                Some(err) => anyhow::anyhow!("codex app-server returned error: {err}"),
                None => anyhow::anyhow!("codex app-server response missing `result`"),
            })?;
        let wire: GetAccountRateLimitsResponse =
            serde_json::from_value(result).context("parsing account/rateLimits/read response")?;
        snapshot = Some(wire_to_snapshot(wire, Utc::now()));
        break;
    }

    // Closing stdin signals codex to exit cleanly.
    drop(stdin);
    let _ = child.wait().await;

    snapshot.ok_or_else(|| {
        anyhow::anyhow!("codex app-server closed before returning rate limits — is `codex` authenticated? Run `codex login`.")
    })
}

// ===========================================================================
// Plan labels
// ===========================================================================

pub(crate) fn plan_label(plan: Option<&str>) -> String {
    match plan {
        Some("plus") => "Codex Plus".to_string(),
        Some("pro") => "Codex Pro".to_string(),
        Some("free") => "Codex Free".to_string(),
        Some("go") => "Codex Go".to_string(),
        Some("team") => "Codex Team".to_string(),
        Some("business") => "Codex Business".to_string(),
        Some("enterprise") => "Codex Enterprise".to_string(),
        Some("edu") => "Codex Edu".to_string(),
        Some("prolite") => "Codex Pro Lite".to_string(),
        Some(other) if !other.is_empty() => format!("Codex {}", capitalise(other)),
        _ => "Codex".to_string(),
    }
}

fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

// ===========================================================================
// Rendering
// ===========================================================================

pub(crate) fn render_codex_limits_block(
    snapshot: Option<&CodexSnapshot>,
    now: DateTime<Utc>,
) -> String {
    let mut out = String::new();
    out.push_str(&purple_bold("  Codex limits"));
    out.push('\n');

    let Some(snap) = snapshot else {
        out.push_str(&dim(
            "    no snapshot yet — run `skopos codex refresh` to fetch one.\n",
        ));
        return out;
    };

    out.push_str(&format!(
        "    {} · {}\n",
        purple(&snap.provider),
        plan_label(snap.plan_type.as_deref()),
    ));
    out.push('\n');
    out.push_str(&render_row("5-hour", snap.primary.as_ref(), now));
    out.push('\n');
    out.push_str(&render_row("weekly", snap.secondary.as_ref(), now));
    out
}

fn render_row(label: &str, window: Option<&CodexWindow>, now: DateTime<Utc>) -> String {
    let Some(window) = window else {
        return format!(
            "    {}  {}\n",
            purple(&format!("{label:<8}")),
            dim("no data"),
        );
    };
    let pct = window.used_percent.clamp(0.0, 100.0);
    let bar = codex_green(&progress_bar(pct, BAR_WIDTH));
    let pct_text = format!("{pct:>5.1}%");
    let resets = match window.resets_at {
        Some(when) => format!("resets in {}", humanise_relative_future(when, now)),
        None => "no reset".to_string(),
    };
    format!(
        "    {}  [{bar}]  {pct_text}   {}\n",
        purple(&format!("{label:<8}")),
        dim(&resets),
    )
}

fn codex_green(text: &str) -> String {
    format!("\x1b[38;2;180;220;130m{text}\x1b[0m")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn sample_rpc_result() -> &'static str {
        r#"{
            "rateLimits": {
                "limitId": "codex",
                "limitName": null,
                "planType": "plus",
                "primary":   {"usedPercent": 1,  "windowDurationMins": 300,   "resetsAt": 1779173203},
                "secondary": {"usedPercent": 24, "windowDurationMins": 10080, "resetsAt": 1779580279},
                "credits":   {"hasCredits": false, "unlimited": false, "balance": "0"},
                "rateLimitReachedType": null
            },
            "rateLimitsByLimitId": {
                "codex": {
                    "limitId": "codex",
                    "limitName": null,
                    "planType": "plus",
                    "primary":   {"usedPercent": 1,  "windowDurationMins": 300,   "resetsAt": 1779173203},
                    "secondary": {"usedPercent": 24, "windowDurationMins": 10080, "resetsAt": 1779580279},
                    "credits":   {"hasCredits": false, "unlimited": false, "balance": "0"},
                    "rateLimitReachedType": null
                }
            }
        }"#
    }

    #[test]
    fn wire_response_parses_into_snapshot() {
        let wire: GetAccountRateLimitsResponse = serde_json::from_str(sample_rpc_result()).unwrap();
        let snap = wire_to_snapshot(wire, ts("2026-05-17T10:00:00Z"));
        assert_eq!(snap.provider, "openai");
        assert_eq!(snap.plan_type.as_deref(), Some("plus"));
        assert_eq!(snap.limit_id.as_deref(), Some("codex"));
        let p = snap.primary.as_ref().unwrap();
        assert_eq!(p.used_percent, 1.0);
        assert_eq!(p.window_duration_mins, Some(300));
        assert_eq!(p.resets_at.map(|d| d.timestamp()), Some(1779173203));
        let s = snap.secondary.as_ref().unwrap();
        assert_eq!(s.used_percent, 24.0);
        assert_eq!(s.window_duration_mins, Some(10080));
        let c = snap.credits.unwrap();
        assert!(!c.has_credits);
        assert!(!c.unlimited);
        assert_eq!(c.balance.as_deref(), Some("0"));
    }

    #[test]
    fn plan_label_covers_known_variants() {
        assert_eq!(plan_label(Some("plus")), "Codex Plus");
        assert_eq!(plan_label(Some("pro")), "Codex Pro");
        assert_eq!(plan_label(Some("free")), "Codex Free");
        assert_eq!(plan_label(Some("prolite")), "Codex Pro Lite");
        assert_eq!(plan_label(Some("mystery")), "Codex Mystery");
        assert_eq!(plan_label(None), "Codex");
        assert_eq!(plan_label(Some("unknown")), "Codex Unknown");
    }

    #[test]
    fn snapshot_save_load_roundtrip() {
        let dir =
            std::env::temp_dir().join(format!("skopos-codex-snapshot-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("codex-session.json");
        let snap = CodexSnapshot {
            provider: "openai".to_string(),
            plan_type: Some("plus".to_string()),
            limit_id: Some("codex".to_string()),
            received_at: Utc.with_ymd_and_hms(2026, 5, 17, 10, 0, 0).unwrap(),
            primary: Some(CodexWindow {
                used_percent: 1.0,
                resets_at: DateTime::from_timestamp(1779173203, 0),
                window_duration_mins: Some(300),
            }),
            secondary: Some(CodexWindow {
                used_percent: 24.0,
                resets_at: DateTime::from_timestamp(1779580279, 0),
                window_duration_mins: Some(10080),
            }),
            credits: Some(CodexCredits {
                has_credits: false,
                unlimited: false,
                balance: Some("0".to_string()),
            }),
            rate_limit_reached_type: None,
        };
        save_codex_snapshot(&path, &snap).unwrap();
        let back = load_codex_snapshot(&path).unwrap().unwrap();
        assert_eq!(back, snap);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_block_shows_hint_when_missing() {
        let body = render_codex_limits_block(None, ts("2026-05-17T10:00:00Z"));
        assert!(body.contains("no snapshot yet"));
        assert!(body.contains("skopos codex refresh"));
    }

    #[test]
    fn render_block_includes_percentages_and_labels() {
        let wire: GetAccountRateLimitsResponse = serde_json::from_str(sample_rpc_result()).unwrap();
        let snap = wire_to_snapshot(wire, ts("2026-05-17T10:00:00Z"));
        let body = render_codex_limits_block(Some(&snap), ts("2026-05-17T10:00:00Z"));
        assert!(body.contains("5-hour"));
        assert!(body.contains("weekly"));
        assert!(body.contains("1.0%"));
        assert!(body.contains("24.0%"));
    }
}
