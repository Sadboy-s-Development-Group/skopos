use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use skopos_core::{ModelId, ProviderId, UsageEvent, UsageSource, UsageSourceKind};
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

pub fn discover_claude_code_jsonl_paths(claude_home: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
    let projects_dir = claude_home.as_ref().join("projects");
    let mut paths = Vec::new();

    if !projects_dir.exists() {
        return Ok(paths);
    }

    collect_jsonl_paths(&projects_dir, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn collect_jsonl_paths(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("read Claude Code projects directory: {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            collect_jsonl_paths(&path, paths)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            paths.push(path);
        }
    }

    Ok(())
}

pub fn parse_usage_events_from_jsonl_path(path: impl AsRef<Path>) -> Result<Vec<UsageEvent>> {
    let path = path.as_ref();
    let file = File::open(path)
        .with_context(|| format!("open Claude Code JSONL file: {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();

    for line in reader.lines() {
        let line =
            line.with_context(|| format!("read Claude Code JSONL file: {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(event) = parse_usage_event_from_jsonl_line(&line)? {
            events.push(event);
        }
    }

    Ok(events)
}

pub fn parse_usage_event_from_jsonl_line(line: &str) -> Result<Option<UsageEvent>> {
    let value: Value = serde_json::from_str(line).context("invalid Claude Code JSONL line")?;

    let message = match value.get("message") {
        Some(message) => message,
        None => return Ok(None),
    };

    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return Ok(None);
    }

    let usage = match message.get("usage") {
        Some(usage) => usage,
        None => return Ok(None),
    };

    let model = match message.get("model").and_then(Value::as_str) {
        Some(model) => model,
        None => return Ok(None),
    };

    let input_tokens = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_creation_input_tokens = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read_input_tokens = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cached_input_tokens = cache_creation_input_tokens + cache_read_input_tokens;
    let total_tokens = input_tokens + output_tokens + cached_input_tokens;

    if total_tokens == 0 {
        return Ok(None);
    }

    let mut event = UsageEvent::new(
        ProviderId::new("anthropic"),
        ModelId::new(model),
        input_tokens,
        output_tokens,
        UsageSource {
            app: "claude-code".to_string(),
            kind: UsageSourceKind::Log,
        },
    );

    event.cached_input_tokens = (cached_input_tokens > 0).then_some(cached_input_tokens);
    event.total_tokens = total_tokens;
    event.timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    event.project_path = value.get("cwd").and_then(Value::as_str).map(PathBuf::from);
    event.session_id = value
        .get("sessionId")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    event.request_id = message
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    event.metadata = serde_json::json!({
        "claude_code_uuid": value.get("uuid").cloned().unwrap_or(Value::Null),
        "claude_code_version": value.get("version").cloned().unwrap_or(Value::Null),
        "service_tier": usage.get("service_tier").cloned().unwrap_or(Value::Null),
        "server_tool_use": usage.get("server_tool_use").cloned().unwrap_or(Value::Null),
    });

    Ok(Some(event))
}
