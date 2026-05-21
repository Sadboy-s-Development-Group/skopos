//! Snapshot of the last statusline payload Claude Code piped to Skopos.
//!
//! Reading Claude's `/usage` rate-limit % via the OAuth endpoint would
//! violate Anthropic's Consumer Terms — those tokens are reserved for
//! native Anthropic applications. The official `statusLine` hook is the
//! supported escape valve: Claude Code pipes a JSON document on stdin to
//! whatever command the user registered (here: `skopos statusline`), and
//! we persist what arrives. The payload includes both the per-session
//! context/cost numbers and Anthropic's own 5h / 7d quota percentages —
//! the latter only once a session has produced any traffic.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::theme::{anthropic_orange, dim, purple, purple_bold};

/// Persisted snapshot of the last statusline payload. One file per host;
/// only `anthropic` today.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct SessionSnapshot {
    pub provider: String,
    /// `subscriptionType` from `.credentials.json`, e.g. `pro`, `max`.
    pub plan: Option<String>,
    /// `rateLimitTier`, e.g. `default_claude_max_5x`. Used as a finer label.
    pub rate_limit_tier: Option<String>,
    /// When `skopos statusline` received this snapshot.
    pub received_at: DateTime<Utc>,
    /// Claude Code's `session_id` for the session that emitted this payload.
    pub session_id: Option<String>,
    /// `cwd` of the session.
    pub cwd: Option<String>,
    /// Display label for the model in use (e.g. "Opus 4.7 (1M context)").
    pub model_display_name: Option<String>,
    /// Internal model id (e.g. "claude-opus-4-7[1m]").
    pub model_id: Option<String>,
    pub context_window: Option<ContextWindow>,
    pub cost: Option<SessionCost>,
    /// Anthropic's 5h and 7d rate-limit windows. Only populated once the
    /// session has produced traffic — null on a fresh session.
    pub rate_limits: Option<RateLimits>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct ContextWindow {
    pub total_input_tokens: Option<u64>,
    pub total_output_tokens: Option<u64>,
    pub context_window_size: Option<u64>,
    /// Sub-object as of Claude Code 2.1.143. Sum the fields for the
    /// effective current usage; the field names mirror the API usage
    /// schema.
    pub current_usage: Option<CurrentUsage>,
    pub used_percentage: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub(crate) struct CurrentUsage {
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u64>,
}

impl CurrentUsage {
    /// Total tokens currently occupying the context window. The split is
    /// preserved on disk for callers that care.
    pub(crate) fn total(&self) -> u64 {
        self.input_tokens.unwrap_or(0)
            + self.output_tokens.unwrap_or(0)
            + self.cache_creation_input_tokens.unwrap_or(0)
            + self.cache_read_input_tokens.unwrap_or(0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct SessionCost {
    pub total_cost_usd: Option<f64>,
    pub total_duration_ms: Option<u64>,
    pub total_lines_added: Option<u64>,
    pub total_lines_removed: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct RateLimits {
    #[serde(default)]
    pub five_hour: Option<RateWindow>,
    #[serde(default)]
    pub seven_day: Option<RateWindow>,
}

/// One rate-limit window. Anthropic encodes `resets_at` as a Unix epoch
/// in seconds (not an ISO string) — handled by `unix_epoch_seconds`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct RateWindow {
    pub used_percentage: f64,
    #[serde(with = "unix_epoch_seconds")]
    pub resets_at: DateTime<Utc>,
}

mod unix_epoch_seconds {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(when: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
        when.timestamp().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime<Utc>, D::Error> {
        let secs: i64 = i64::deserialize(d)?;
        DateTime::from_timestamp(secs, 0)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid unix timestamp: {secs}")))
    }
}

impl SessionSnapshot {
    /// Human plan label for the splash/usage header, e.g. `Claude Max 5x`.
    pub(crate) fn plan_label(&self) -> String {
        plan_label(self.plan.as_deref(), self.rate_limit_tier.as_deref())
    }
}

// ===========================================================================
// On-disk paths
// ===========================================================================

/// Snapshot path: `~/.local/share/skopos/session.json`.
pub(crate) fn snapshot_path() -> PathBuf {
    skopos_data_dir().join("session.json")
}

/// Debug copy of the last raw statusline payload — handy when the schema
/// drifts between Claude Code versions. Overwritten on every call.
pub(crate) fn last_payload_path() -> PathBuf {
    skopos_data_dir().join("statusline-last-payload.json")
}

pub(crate) fn save_last_payload(payload: &str) -> anyhow::Result<()> {
    let path = last_payload_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, payload)?;
    Ok(())
}

fn skopos_data_dir() -> PathBuf {
    home_dir().join(".local").join("share").join("skopos")
}

pub(crate) fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn claude_settings_path() -> PathBuf {
    home_dir().join(".claude").join("settings.json")
}

pub(crate) fn claude_credentials_path() -> PathBuf {
    home_dir().join(".claude").join(".credentials.json")
}

pub(crate) fn claude_home() -> PathBuf {
    home_dir().join(".claude")
}

// ===========================================================================
// Snapshot I/O
// ===========================================================================

pub(crate) fn load_snapshot(path: &Path) -> anyhow::Result<Option<SessionSnapshot>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)?;
    let snapshot = serde_json::from_str(&raw)
        .map_err(|err| anyhow::anyhow!("failed to parse {}: {err}", path.display()))?;
    Ok(Some(snapshot))
}

pub(crate) fn save_snapshot(path: &Path, snapshot: &SessionSnapshot) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(snapshot)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, body)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

// ===========================================================================
// Statusline input parsing
// ===========================================================================

/// Tolerant view over the JSON Claude Code pipes to a `statusLine` command.
/// Unknown fields are ignored to stay forward-compatible.
#[derive(Debug, Deserialize)]
struct StatuslineInput {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    model: Option<StatuslineModel>,
    #[serde(default)]
    context_window: Option<ContextWindow>,
    #[serde(default)]
    cost: Option<SessionCost>,
    #[serde(default)]
    rate_limits: Option<RateLimits>,
}

#[derive(Debug, Deserialize)]
struct StatuslineModel {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
}

/// Build a snapshot from one statusline JSON payload plus the user's plan.
pub(crate) fn snapshot_from_statusline_json(
    payload: &str,
    plan: Option<String>,
    rate_limit_tier: Option<String>,
    received_at: DateTime<Utc>,
) -> anyhow::Result<SessionSnapshot> {
    let parsed: StatuslineInput = serde_json::from_str(payload)
        .map_err(|err| anyhow::anyhow!("failed to parse statusline JSON: {err}"))?;
    let model = parsed.model.unwrap_or(StatuslineModel {
        id: None,
        display_name: None,
    });
    Ok(SessionSnapshot {
        provider: "anthropic".to_string(),
        plan,
        rate_limit_tier,
        received_at,
        session_id: parsed.session_id,
        cwd: parsed.cwd,
        model_display_name: model.display_name,
        model_id: model.id,
        context_window: parsed.context_window,
        cost: parsed.cost,
        rate_limits: parsed.rate_limits,
    })
}

pub(crate) fn read_stdin_to_string<R: Read>(mut reader: R) -> anyhow::Result<String> {
    let mut buf = String::new();
    reader.read_to_string(&mut buf)?;
    Ok(buf)
}

// ===========================================================================
// Plan label from .credentials.json
// ===========================================================================

#[derive(Debug, Deserialize)]
struct CredentialsFile {
    #[serde(default, rename = "claudeAiOauth")]
    claude_ai_oauth: Option<CredentialsOauth>,
}

#[derive(Debug, Deserialize)]
struct CredentialsOauth {
    #[serde(default, rename = "subscriptionType")]
    subscription_type: Option<String>,
    #[serde(default, rename = "rateLimitTier")]
    rate_limit_tier: Option<String>,
}

pub(crate) fn read_plan_labels(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(raw) = fs::read_to_string(path) else {
        return (None, None);
    };
    let Ok(parsed) = serde_json::from_str::<CredentialsFile>(&raw) else {
        return (None, None);
    };
    let oauth = parsed.claude_ai_oauth.unwrap_or(CredentialsOauth {
        subscription_type: None,
        rate_limit_tier: None,
    });
    (oauth.subscription_type, oauth.rate_limit_tier)
}

pub(crate) fn plan_label(plan: Option<&str>, tier: Option<&str>) -> String {
    let base = match plan {
        Some("pro") => "Claude Pro",
        Some("max") => "Claude Max",
        Some(other) if !other.is_empty() => return capitalise(other),
        _ => "Claude",
    };
    let suffix = match tier {
        Some(t) if t.contains("max_20x") => " 20x",
        Some(t) if t.contains("max_5x") => " 5x",
        _ => "",
    };
    format!("{base}{suffix}")
}

fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

// ===========================================================================
// Rendering — Current Session block (statusline data)
// ===========================================================================

const BAR_WIDTH: usize = 24;

/// Render the "Current Session" block. `now` is injected so the relative
/// time is testable.
pub(crate) fn render_session_block(
    snapshot: Option<&SessionSnapshot>,
    now: DateTime<Utc>,
) -> String {
    let mut out = String::new();
    out.push_str(&purple_bold("  Current Session"));
    out.push('\n');

    let Some(snap) = snapshot else {
        out.push_str(&dim(
            "    no snapshot yet — run `skopos usage install` and open Claude Code once.\n",
        ));
        return out;
    };

    let header = format!("    {} · {}", snap.provider, snap.plan_label());
    out.push_str(&purple(&header));
    out.push_str(&dim(&format!(
        "   updated {}\n",
        humanise_relative_past(snap.received_at, now),
    )));

    if let Some(name) = &snap.model_display_name {
        out.push_str(&format!("    {}{}\n", purple("model    "), name));
    }
    if let Some(cwd) = &snap.cwd {
        out.push_str(&format!("    {}{}\n", purple("cwd      "), cwd));
    }
    if let Some(ctx) = &snap.context_window {
        out.push_str(&format!(
            "    {}{}\n",
            purple("ctx      "),
            render_ctx_line(ctx)
        ));
    }
    if let Some(cost) = &snap.cost {
        out.push_str(&format!(
            "    {}{}\n",
            purple("cost     "),
            render_cost_line(cost)
        ));
    }
    out
}

fn render_ctx_line(ctx: &ContextWindow) -> String {
    let pct = ctx.used_percentage;
    let pct_text = match pct {
        Some(p) => format!("{:>5.1}%", p),
        None => "  ——  ".to_string(),
    };
    let bar_pct = pct.unwrap_or(0.0);
    let bar = anthropic_orange(&progress_bar(bar_pct, BAR_WIDTH));
    let totals = match (ctx.current_usage.as_ref(), ctx.context_window_size) {
        (Some(u), Some(size)) => format!(
            "  {} of {} tokens",
            short_tokens(u.total()),
            short_tokens(size),
        ),
        _ => String::new(),
    };
    format!("[{bar}]  {pct_text}{}", dim(&totals))
}

/// Render the "Limits" block — the official 5h / 7d quota windows from
/// Anthropic, as forwarded by the statusline hook.
pub(crate) fn render_limits_block(
    snapshot: Option<&SessionSnapshot>,
    now: DateTime<Utc>,
) -> String {
    let mut out = String::new();
    out.push_str(&purple_bold("  Limits"));
    out.push('\n');

    let Some(snap) = snapshot else {
        out.push_str(&dim(
            "    no snapshot yet — run `skopos usage install` and open Claude Code once.\n",
        ));
        return out;
    };

    let Some(rl) = &snap.rate_limits else {
        out.push_str(&dim(
            "    no rate-limit data in the last snapshot — open Claude Code and send one message.\n",
        ));
        return out;
    };

    out.push_str(&format!(
        "    {} · {}\n",
        purple(&snap.provider),
        snap.plan_label(),
    ));
    out.push('\n');
    out.push_str(&render_limits_row("5-hour", rl.five_hour.as_ref(), now));
    out.push('\n');
    out.push_str(&render_limits_row("weekly", rl.seven_day.as_ref(), now));
    out
}

fn render_limits_row(label: &str, window: Option<&RateWindow>, now: DateTime<Utc>) -> String {
    let Some(window) = window else {
        return format!(
            "    {}  {}\n",
            purple(&format!("{label:<8}")),
            dim("no data"),
        );
    };
    let pct = window.used_percentage.clamp(0.0, 100.0);
    let bar = anthropic_orange(&progress_bar(pct, BAR_WIDTH));
    let pct_text = format!("{pct:>5.1}%");
    let resets = humanise_relative_future(window.resets_at, now);
    format!(
        "    {}  [{bar}]  {pct_text}   {}\n",
        purple(&format!("{label:<8}")),
        dim(&format!("resets in {resets}")),
    )
}

pub(crate) fn humanise_relative_future(when: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let delta = when - now;
    if delta <= Duration::zero() {
        return "now".to_string();
    }
    humanise_duration(delta)
}

fn render_cost_line(cost: &SessionCost) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(usd) = cost.total_cost_usd {
        parts.push(format!("${:.2}", usd));
    }
    if let Some(ms) = cost.total_duration_ms {
        parts.push(humanise_ms(ms));
    }
    match (cost.total_lines_added, cost.total_lines_removed) {
        (Some(add), Some(rem)) if add + rem > 0 => parts.push(format!("+{add} / -{rem} lines")),
        _ => {}
    }
    if parts.is_empty() {
        return dim("——").to_string();
    }
    parts.join(&dim("   "))
}

fn humanise_ms(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        return format!("{secs}s");
    }
    let minutes = secs / 60;
    let s = secs % 60;
    if minutes < 60 {
        return format!("{minutes}m {s:02}s");
    }
    let hours = minutes / 60;
    let m = minutes % 60;
    format!("{hours}h {m:02}m")
}

/// Compact token count for "240K of 1.0M" formatting. Two sig figs.
fn short_tokens(n: u64) -> String {
    let x = n as f64;
    if x < 1_000.0 {
        return format!("{n}");
    }
    if x < 1_000_000.0 {
        return format!("{:.0}K", x / 1_000.0);
    }
    if x < 1_000_000_000.0 {
        return format!("{:.1}M", x / 1_000_000.0);
    }
    format!("{:.1}B", x / 1_000_000_000.0)
}

// ===========================================================================
// Bar primitives
// ===========================================================================

/// Whole-cell progress bar — no partial-block glyphs, because the thin
/// fractional characters (▏▎▍…) read as gaps next to a solid `█`. We
/// round to the nearest full cell, accepting ~4% quantisation on a
/// `BAR_WIDTH=24` bar in exchange for a continuous filled segment.
pub(crate) fn progress_bar(pct: f64, width: usize) -> String {
    let ratio = (pct / 100.0).clamp(0.0, 1.0);
    let full = (ratio * width as f64).round() as usize;
    let full = full.min(width);
    let mut bar = String::with_capacity(width * 3);
    for _ in 0..full {
        bar.push('█');
    }
    for _ in full..width {
        bar.push('░');
    }
    bar
}

// ===========================================================================
// Relative-time helpers
// ===========================================================================

pub(crate) fn humanise_relative_past(when: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let delta = now - when;
    if delta < Duration::minutes(1) {
        return "just now".to_string();
    }
    format!("{} ago", humanise_duration(delta))
}

fn humanise_duration(d: Duration) -> String {
    let total_minutes = d.num_minutes();
    if total_minutes < 1 {
        return "<1m".to_string();
    }
    let days = total_minutes / (60 * 24);
    let hours = (total_minutes % (60 * 24)) / 60;
    let minutes = total_minutes % 60;
    if days > 0 {
        return format!("{days}d {hours}h");
    }
    if hours > 0 {
        return format!("{hours}h {minutes}m");
    }
    format!("{minutes}m")
}

// ===========================================================================
// Statusline compact output (what Claude Code shows above the prompt)
// ===========================================================================

/// One-line summary used as the visible statusline above Claude Code's
/// prompt. Brand-coloured: labels in purple, values in Anthropic orange,
/// separators dimmed. Example output (uncoloured):
/// `skopos · 5h 2% · 7d 4% · ctx 15% · $7.35`.
pub(crate) fn render_statusline_line(snap: &SessionSnapshot) -> String {
    let chip = |label: &str, value: &str| -> String {
        format!("{} {}", purple(label), anthropic_orange(value))
    };

    let mut parts: Vec<String> = Vec::new();
    parts.push(purple_bold("skopos"));
    if let Some(rl) = &snap.rate_limits {
        if let Some(w) = &rl.five_hour {
            parts.push(chip("5h", &format!("{:.0}%", w.used_percentage)));
        }
        if let Some(w) = &rl.seven_day {
            parts.push(chip("7d", &format!("{:.0}%", w.used_percentage)));
        }
    }
    if let Some(ctx) = &snap.context_window {
        if let Some(pct) = ctx.used_percentage {
            parts.push(chip("ctx", &format!("{pct:.0}%")));
        }
    }
    if let Some(cost) = &snap.cost {
        if let Some(usd) = cost.total_cost_usd {
            if usd > 0.0 {
                parts.push(anthropic_orange(&format!("${usd:.2}")));
            }
        }
    }
    parts.join(&dim("  ·  "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn fake_payload() -> &'static str {
        r#"{
            "session_id": "s1",
            "cwd": "/home/me/project",
            "model": {"id": "claude-opus-4-7[1m]", "display_name": "Opus 4.7 (1M context)"},
            "version": "2.1.143",
            "context_window": {
                "total_input_tokens": 240,
                "total_output_tokens": 60,
                "context_window_size": 1000000,
                "current_usage": {
                    "input_tokens": 1,
                    "output_tokens": 47,
                    "cache_creation_input_tokens": 609,
                    "cache_read_input_tokens": 147728
                },
                "used_percentage": 15,
                "remaining_percentage": 85
            },
            "cost": {
                "total_cost_usd": 7.35,
                "total_duration_ms": 192000,
                "total_lines_added": 123,
                "total_lines_removed": 45
            },
            "rate_limits": {
                "five_hour": {"used_percentage": 2, "resets_at": 1779091800},
                "seven_day": {"used_percentage": 4, "resets_at": 1779487200}
            }
        }"#
    }

    #[test]
    fn snapshot_roundtrips_through_json() {
        let snap = snapshot_from_statusline_json(
            fake_payload(),
            Some("max".into()),
            Some("default_claude_max_5x".into()),
            ts("2026-05-17T10:00:00Z"),
        )
        .unwrap();
        let body = serde_json::to_string(&snap).unwrap();
        let back: SessionSnapshot = serde_json::from_str(&body).unwrap();
        assert_eq!(back, snap);
    }

    #[test]
    fn statusline_extracts_session_and_context() {
        let snap =
            snapshot_from_statusline_json(fake_payload(), None, None, ts("2026-05-17T10:00:00Z"))
                .unwrap();
        assert_eq!(snap.session_id.as_deref(), Some("s1"));
        assert_eq!(snap.cwd.as_deref(), Some("/home/me/project"));
        assert_eq!(
            snap.model_display_name.as_deref(),
            Some("Opus 4.7 (1M context)")
        );
        let ctx = snap.context_window.unwrap();
        assert_eq!(ctx.used_percentage, Some(15.0));
        let usage = ctx.current_usage.unwrap();
        // 1 + 47 + 609 + 147728 = 148385
        assert_eq!(usage.total(), 148_385);
        let cost = snap.cost.unwrap();
        assert_eq!(cost.total_cost_usd, Some(7.35));
        assert_eq!(cost.total_lines_added, Some(123));
    }

    #[test]
    fn statusline_extracts_rate_limits_with_unix_epoch_reset() {
        let snap =
            snapshot_from_statusline_json(fake_payload(), None, None, ts("2026-05-17T10:00:00Z"))
                .unwrap();
        let rl = snap.rate_limits.expect("rate_limits present");
        let fh = rl.five_hour.unwrap();
        assert_eq!(fh.used_percentage, 2.0);
        assert_eq!(fh.resets_at.timestamp(), 1779091800);
        let sd = rl.seven_day.unwrap();
        assert_eq!(sd.used_percentage, 4.0);
    }

    #[test]
    fn statusline_handles_null_context_fields() {
        let payload = r#"{
            "session_id": "s1",
            "context_window": {
                "total_input_tokens": 0,
                "total_output_tokens": 0,
                "context_window_size": 1000000,
                "current_usage": null,
                "used_percentage": null,
                "remaining_percentage": null
            },
            "cost": {"total_cost_usd": 0, "total_duration_ms": 200}
        }"#;
        let snap =
            snapshot_from_statusline_json(payload, None, None, ts("2026-05-17T10:00:00Z")).unwrap();
        let ctx = snap.context_window.unwrap();
        assert!(ctx.used_percentage.is_none());
        assert!(ctx.current_usage.is_none());
    }

    #[test]
    fn render_session_block_shows_no_snapshot_message_when_missing() {
        let now = ts("2026-05-17T10:00:00Z");
        let body = render_session_block(None, now);
        assert!(body.contains("no snapshot yet"));
    }

    #[test]
    fn progress_bar_endpoints_and_midpoint() {
        assert_eq!(progress_bar(0.0, 8), "░░░░░░░░");
        assert_eq!(progress_bar(100.0, 8), "████████");
        let mid = progress_bar(50.0, 8);
        assert!(mid.starts_with("████"));
        assert!(mid.ends_with("░░░░"));
    }

    #[test]
    fn plan_label_handles_pro_and_max_variants() {
        assert_eq!(
            plan_label(Some("max"), Some("default_claude_max_5x")),
            "Claude Max 5x"
        );
        assert_eq!(
            plan_label(Some("max"), Some("default_claude_max_20x")),
            "Claude Max 20x"
        );
        assert_eq!(plan_label(Some("pro"), None), "Claude Pro");
        assert_eq!(plan_label(None, None), "Claude");
    }

    #[test]
    fn short_tokens_formats_with_units() {
        assert_eq!(short_tokens(512), "512");
        assert_eq!(short_tokens(240_000), "240K");
        assert_eq!(short_tokens(1_000_000), "1.0M");
        assert_eq!(short_tokens(8_400_000), "8.4M");
    }

    #[test]
    fn humanise_ms_formats_durations() {
        assert_eq!(humanise_ms(249), "0s");
        assert_eq!(humanise_ms(15_000), "15s");
        assert_eq!(humanise_ms(192_000), "3m 12s");
        assert_eq!(humanise_ms(3_900_000), "1h 05m");
    }

    #[test]
    fn snapshot_save_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "skopos-session-snapshot-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("session.json");
        let snap = SessionSnapshot {
            provider: "anthropic".to_string(),
            plan: Some("pro".to_string()),
            rate_limit_tier: None,
            received_at: Utc.with_ymd_and_hms(2026, 5, 17, 10, 0, 0).unwrap(),
            session_id: None,
            cwd: None,
            model_display_name: None,
            model_id: None,
            context_window: None,
            cost: None,
            rate_limits: None,
        };
        save_snapshot(&path, &snap).unwrap();
        let back = load_snapshot(&path).unwrap().unwrap();
        assert_eq!(back, snap);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
