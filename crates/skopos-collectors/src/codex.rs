use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use skopos_core::{ModelId, ProviderId, UsageEvent, UsageSource, UsageSourceKind};
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

#[derive(Debug, Default, Clone)]
pub struct RolloutState {
    pub session_id: Option<String>,
    pub session_cwd: Option<String>,
    pub cli_version: Option<String>,
    pub originator: Option<String>,
    pub current_turn_id: Option<String>,
    pub current_model: Option<String>,
    pub current_cwd: Option<String>,
}

pub fn discover_codex_rollout_paths(codex_home: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
    let sessions_dir = codex_home.as_ref().join("sessions");
    let mut paths = Vec::new();

    if !sessions_dir.exists() {
        return Ok(paths);
    }

    collect_jsonl_paths(&sessions_dir, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn collect_jsonl_paths(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("read Codex sessions directory: {}", dir.display()))?
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

pub fn parse_usage_events_from_rollout_path(path: impl AsRef<Path>) -> Result<Vec<UsageEvent>> {
    let path = path.as_ref();
    let file = File::open(path)
        .with_context(|| format!("open Codex rollout JSONL file: {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut state = RolloutState::default();

    for line in reader.lines() {
        let line =
            line.with_context(|| format!("read Codex rollout JSONL file: {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(event) = parse_usage_event_from_jsonl_line(&line, &mut state)? {
            events.push(event);
        }
    }

    Ok(events)
}

pub fn parse_usage_event_from_jsonl_line(
    line: &str,
    state: &mut RolloutState,
) -> Result<Option<UsageEvent>> {
    let value: Value = serde_json::from_str(line).context("invalid Codex rollout JSONL line")?;

    let record_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    let payload = match value.get("payload") {
        Some(payload) => payload,
        None => return Ok(None),
    };

    match record_type {
        "session_meta" => {
            state.session_id = payload
                .get("id")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            state.session_cwd = payload
                .get("cwd")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            state.cli_version = payload
                .get("cli_version")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            state.originator = payload
                .get("originator")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            Ok(None)
        }
        "turn_context" => {
            state.current_turn_id = payload
                .get("turn_id")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let nested_model = payload
                .get("collaboration_mode")
                .and_then(|cm| cm.get("settings"))
                .and_then(|s| s.get("model"))
                .and_then(Value::as_str);
            state.current_model = payload
                .get("model")
                .and_then(Value::as_str)
                .or(nested_model)
                .map(ToString::to_string);
            state.current_cwd = payload
                .get("cwd")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            Ok(None)
        }
        "event_msg" => {
            if payload.get("type").and_then(Value::as_str) != Some("token_count") {
                return Ok(None);
            }
            emit_token_count_event(&value, payload, state)
        }
        _ => Ok(None),
    }
}

fn emit_token_count_event(
    outer: &Value,
    payload: &Value,
    state: &RolloutState,
) -> Result<Option<UsageEvent>> {
    let info = match payload.get("info") {
        Some(info) => info,
        None => return Ok(None),
    };
    let last = match info.get("last_token_usage") {
        Some(last) => last,
        None => return Ok(None),
    };

    let gross_input = last
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cached_input = last
        .get("cached_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = last
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning_output_tokens = last
        .get("reasoning_output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = last
        .get("total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    if total_tokens == 0 {
        return Ok(None);
    }

    let input_tokens = gross_input.saturating_sub(cached_input);
    let model = state.current_model.as_deref().unwrap_or("unknown");

    let mut event = UsageEvent::new(
        ProviderId::new("openai"),
        ModelId::new(model),
        input_tokens,
        output_tokens,
        UsageSource {
            app: "codex".to_string(),
            kind: UsageSourceKind::Log,
        },
    );

    event.cached_input_tokens = (cached_input > 0).then_some(cached_input);
    event.reasoning_tokens = (reasoning_output_tokens > 0).then_some(reasoning_output_tokens);
    event.total_tokens = total_tokens;
    event.timestamp = outer
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    event.project_path = state
        .current_cwd
        .as_ref()
        .or(state.session_cwd.as_ref())
        .map(PathBuf::from);
    event.session_id = state.session_id.clone();
    event.request_id = payload
        .get("turn_id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| state.current_turn_id.clone());
    event.metadata = serde_json::json!({
        "codex_originator": state.originator,
        "codex_cli_version": state.cli_version,
        "codex_model_context_window": info.get("model_context_window").cloned().unwrap_or(Value::Null),
        "reasoning_output_tokens": reasoning_output_tokens,
    });

    Ok(Some(event))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_META_LINE: &str = r#"{"timestamp":"2026-05-19T01:57:38.272Z","type":"session_meta","payload":{"id":"00000000-0000-4000-8000-000000000001","timestamp":"2026-05-19T01:42:36.698Z","cwd":"/home/example/project","originator":"codex-tui","cli_version":"0.131.0","source":"cli","model_provider":"openai"}}"#;

    const TURN_CONTEXT_LINE: &str = r#"{"timestamp":"2026-05-19T01:57:38.275Z","type":"turn_context","payload":{"turn_id":"00000000-0000-4000-8000-000000000002","cwd":"/home/example/project","current_date":"2026-05-18","model":"gpt-5.5","personality":"pragmatic","collaboration_mode":{"mode":"default","settings":{"model":"gpt-5.5"}}}}"#;

    const TOKEN_COUNT_LINE: &str = r#"{"timestamp":"2026-05-19T01:57:39.734Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":13367,"cached_input_tokens":11648,"output_tokens":18,"reasoning_output_tokens":0,"total_tokens":13385},"last_token_usage":{"input_tokens":13367,"cached_input_tokens":11648,"output_tokens":18,"reasoning_output_tokens":0,"total_tokens":13385},"model_context_window":258400},"rate_limits":{},"turn_id":"00000000-0000-4000-8000-000000000002"}}"#;

    const TASK_STARTED_LINE: &str = r#"{"timestamp":"2026-05-19T01:57:38.300Z","type":"event_msg","payload":{"type":"task_started","turn_id":"00000000-0000-4000-8000-000000000002"}}"#;

    const ZERO_TOKEN_COUNT_LINE: &str = r#"{"timestamp":"2026-05-19T01:57:39.734Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":0}}}}"#;

    #[test]
    fn parse_usage_event_from_jsonl_line_yields_an_event_for_token_count() {
        let mut state = RolloutState::default();
        assert!(
            parse_usage_event_from_jsonl_line(SESSION_META_LINE, &mut state)
                .unwrap()
                .is_none()
        );
        assert!(
            parse_usage_event_from_jsonl_line(TURN_CONTEXT_LINE, &mut state)
                .unwrap()
                .is_none()
        );

        let event = parse_usage_event_from_jsonl_line(TOKEN_COUNT_LINE, &mut state)
            .expect("line should parse")
            .expect("token_count line should produce event");

        assert_eq!(event.provider.0, "openai");
        assert_eq!(event.model.0, "gpt-5.5");
        assert_eq!(event.input_tokens, 13367 - 11648);
        assert_eq!(event.cached_input_tokens, Some(11648));
        assert_eq!(event.output_tokens, 18);
        assert_eq!(event.total_tokens, 13385);
        assert_eq!(event.source.app, "codex");
        assert_eq!(
            event.session_id.as_deref(),
            Some("00000000-0000-4000-8000-000000000001")
        );
        assert_eq!(
            event.request_id.as_deref(),
            Some("00000000-0000-4000-8000-000000000002")
        );
        assert_eq!(
            event.project_path.unwrap().to_string_lossy(),
            "/home/example/project"
        );
    }

    #[test]
    fn non_token_count_event_msgs_are_skipped() {
        let mut state = RolloutState::default();
        let event = parse_usage_event_from_jsonl_line(TASK_STARTED_LINE, &mut state).unwrap();
        assert!(event.is_none());
    }

    #[test]
    fn session_meta_updates_state_without_emitting() {
        let mut state = RolloutState::default();
        let event = parse_usage_event_from_jsonl_line(SESSION_META_LINE, &mut state).unwrap();
        assert!(event.is_none());
        assert_eq!(
            state.session_id.as_deref(),
            Some("00000000-0000-4000-8000-000000000001")
        );
        assert_eq!(state.session_cwd.as_deref(), Some("/home/example/project"));
        assert_eq!(state.cli_version.as_deref(), Some("0.131.0"));
        assert_eq!(state.originator.as_deref(), Some("codex-tui"));
    }

    #[test]
    fn turn_context_updates_state_without_emitting() {
        let mut state = RolloutState::default();
        let event = parse_usage_event_from_jsonl_line(TURN_CONTEXT_LINE, &mut state).unwrap();
        assert!(event.is_none());
        assert_eq!(state.current_model.as_deref(), Some("gpt-5.5"));
        assert_eq!(
            state.current_turn_id.as_deref(),
            Some("00000000-0000-4000-8000-000000000002")
        );
        assert_eq!(state.current_cwd.as_deref(), Some("/home/example/project"));
    }

    #[test]
    fn zero_total_token_count_events_are_skipped() {
        let mut state = RolloutState::default();
        let event = parse_usage_event_from_jsonl_line(ZERO_TOKEN_COUNT_LINE, &mut state).unwrap();
        assert!(event.is_none());
    }

    #[test]
    fn discover_codex_rollout_paths_walks_nested_dirs() {
        let temp_dir = std::env::temp_dir().join(format!(
            "skopos-collectors-codex-discover-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let nested = temp_dir.join("sessions").join("2026").join("05").join("18");
        std::fs::create_dir_all(&nested).unwrap();
        let rollout = nested.join("rollout-foo.jsonl");
        let other = nested.join("notes.md");
        std::fs::write(&rollout, "").unwrap();
        std::fs::write(&other, "ignore").unwrap();

        let paths = discover_codex_rollout_paths(&temp_dir).unwrap();
        assert_eq!(paths, vec![rollout]);

        std::fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn parse_usage_events_from_rollout_path_streams_state_through_lines() {
        let temp_dir = std::env::temp_dir().join(format!(
            "skopos-collectors-codex-stream-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let path = temp_dir.join("rollout.jsonl");
        let body = format!(
            "{}\n{}\n{}\n{}\n",
            SESSION_META_LINE, TASK_STARTED_LINE, TURN_CONTEXT_LINE, TOKEN_COUNT_LINE
        );
        std::fs::write(&path, body).unwrap();

        let events = parse_usage_events_from_rollout_path(&path).unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.model.0, "gpt-5.5");
        assert_eq!(event.input_tokens, 13367 - 11648);
        assert_eq!(event.cached_input_tokens, Some(11648));
        assert_eq!(event.output_tokens, 18);
        assert_eq!(event.total_tokens, 13385);

        std::fs::remove_dir_all(&temp_dir).unwrap();
    }
}
