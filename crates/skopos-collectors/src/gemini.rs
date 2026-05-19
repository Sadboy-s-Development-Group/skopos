use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use skopos_core::{ModelId, ProviderId, UsageEvent, UsageSource, UsageSourceKind};
use std::{
    collections::HashSet,
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

#[derive(Debug, Default, Clone)]
pub struct SessionState {
    pub session_id: Option<String>,
    pub project_hash: Option<String>,
    pub project_alias: Option<String>,
    pub session_kind: Option<String>,
    pub seen_message_ids: HashSet<String>,
}

pub fn discover_gemini_session_paths(gemini_home: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
    let tmp_dir = gemini_home.as_ref().join("tmp");
    let mut paths = Vec::new();

    if !tmp_dir.exists() {
        return Ok(paths);
    }

    collect_jsonl_paths(&tmp_dir, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn collect_jsonl_paths(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("read Gemini tmp directory: {}", dir.display()))?
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

pub fn parse_usage_events_from_session_path(path: impl AsRef<Path>) -> Result<Vec<UsageEvent>> {
    let path = path.as_ref();
    let file = File::open(path)
        .with_context(|| format!("open Gemini session JSONL file: {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut state = SessionState {
        project_alias: project_alias_from_path(path),
        ..SessionState::default()
    };

    let project_path = path
        .ancestors()
        .nth(4)
        .and_then(|gemini_home| read_alias_to_cwd_map(gemini_home).ok())
        .and_then(|map| {
            state
                .project_alias
                .as_ref()
                .and_then(|alias| map.get(alias).cloned())
        });

    for line in reader.lines() {
        let line =
            line.with_context(|| format!("read Gemini session JSONL file: {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(mut event) = parse_usage_event_from_jsonl_line(&line, &mut state)? {
            if event.project_path.is_none() {
                event.project_path = project_path.clone();
            }
            events.push(event);
        }
    }

    Ok(events)
}

pub fn parse_usage_event_from_jsonl_line(
    line: &str,
    state: &mut SessionState,
) -> Result<Option<UsageEvent>> {
    let value: Value = serde_json::from_str(line).context("invalid Gemini session JSONL line")?;

    if value.get("$set").is_some() {
        return Ok(None);
    }

    let record_type = value.get("type").and_then(Value::as_str);

    if record_type.is_none()
        && value.get("sessionId").is_some()
        && value.get("projectHash").is_some()
    {
        state.session_id = value
            .get("sessionId")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        state.project_hash = value
            .get("projectHash")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        state.session_kind = value
            .get("kind")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        return Ok(None);
    }

    if record_type != Some("gemini") {
        return Ok(None);
    }

    let message_id = match value.get("id").and_then(Value::as_str) {
        Some(id) => id.to_string(),
        None => return Ok(None),
    };

    if state.seen_message_ids.contains(&message_id) {
        return Ok(None);
    }
    state.seen_message_ids.insert(message_id.clone());

    let tokens = match value.get("tokens") {
        Some(tokens) => tokens,
        None => return Ok(None),
    };

    let total_tokens = tokens.get("total").and_then(Value::as_u64).unwrap_or(0);
    if total_tokens == 0 {
        return Ok(None);
    }

    let gross_input = tokens.get("input").and_then(Value::as_u64).unwrap_or(0);
    let cached = tokens.get("cached").and_then(Value::as_u64).unwrap_or(0);
    let output_tokens = tokens.get("output").and_then(Value::as_u64).unwrap_or(0);
    let thoughts = tokens.get("thoughts").and_then(Value::as_u64).unwrap_or(0);
    let tool_tokens = tokens.get("tool").and_then(Value::as_u64).unwrap_or(0);
    let input_tokens = gross_input.saturating_sub(cached);

    let model = value
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    let mut event = UsageEvent::new(
        ProviderId::new("google"),
        ModelId::new(model),
        input_tokens,
        output_tokens,
        UsageSource {
            app: "gemini-cli".to_string(),
            kind: UsageSourceKind::Log,
        },
    );

    event.cached_input_tokens = (cached > 0).then_some(cached);
    event.reasoning_tokens = (thoughts > 0).then_some(thoughts);
    event.total_tokens = total_tokens;
    event.timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    event.session_id = state.session_id.clone();
    event.request_id = Some(message_id);

    let thoughts_count = value
        .get("thoughts")
        .and_then(Value::as_array)
        .map(|arr| arr.len());

    event.metadata = serde_json::json!({
        "gemini_session_kind": state.session_kind,
        "gemini_project_hash": state.project_hash,
        "gemini_tool_tokens": tool_tokens,
        "gemini_thoughts_count": thoughts_count,
    });

    Ok(Some(event))
}

fn project_alias_from_path(path: &Path) -> Option<String> {
    path.parent()
        .and_then(|chats_dir| chats_dir.parent())
        .and_then(|alias_dir| alias_dir.file_name())
        .map(|name| name.to_string_lossy().into_owned())
}

fn read_alias_to_cwd_map(gemini_home: &Path) -> Result<std::collections::HashMap<String, PathBuf>> {
    let path = gemini_home.join("projects.json");
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("read Gemini projects.json: {}", path.display()))?;
    let value: Value = serde_json::from_str(&contents)
        .with_context(|| format!("parse Gemini projects.json: {}", path.display()))?;
    let mut map = std::collections::HashMap::new();
    if let Some(projects) = value.get("projects").and_then(Value::as_object) {
        for (cwd, alias) in projects {
            if let Some(alias) = alias.as_str() {
                map.insert(alias.to_string(), PathBuf::from(cwd));
            }
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_META_LINE: &str = r#"{"sessionId":"00000000-0000-4000-8000-000000000001","projectHash":"0000000000000000000000000000000000000000000000000000000000000001","startTime":"2026-05-19T02:28:52.118Z","lastUpdated":"2026-05-19T02:28:52.118Z","kind":"main"}"#;
    const USER_LINE: &str = r#"{"id":"00000000-0000-4000-8000-000000000002","timestamp":"2026-05-19T02:29:02.306Z","type":"user","content":[{"text":"hello"}]}"#;
    const DOLLAR_SET_LINE: &str = r#"{"$set":{"lastUpdated":"2026-05-19T02:29:07.616Z"}}"#;
    const GEMINI_LINE: &str = r#"{"id":"00000000-0000-4000-8000-000000000003","timestamp":"2026-05-19T02:29:07.616Z","type":"gemini","content":"<p>hello","thoughts":[{"subject":"a","description":"b","timestamp":"2026-05-19T02:29:05.632Z"}],"tokens":{"input":11427,"output":132,"cached":0,"thoughts":539,"tool":0,"total":12098},"model":"gemini-3-flash-preview"}"#;
    const GEMINI_LINE_CACHED: &str = r#"{"id":"00000000-0000-4000-8000-000000000004","timestamp":"2026-05-19T02:29:09.702Z","type":"gemini","tokens":{"input":12184,"output":23,"cached":3800,"thoughts":62,"tool":0,"total":12269},"model":"gemini-3-flash-preview"}"#;
    const GEMINI_LINE_DISTINCT: &str = r#"{"id":"00000000-0000-4000-8000-000000000005","timestamp":"2026-05-19T02:29:15.016Z","type":"gemini","tokens":{"input":300,"output":10,"cached":0,"thoughts":0,"tool":0,"total":310},"model":"gemini-3-flash-preview"}"#;
    const ZERO_GEMINI_LINE: &str = r#"{"id":"zero-id","timestamp":"2026-05-19T02:29:07.616Z","type":"gemini","tokens":{"input":0,"output":0,"cached":0,"thoughts":0,"tool":0,"total":0},"model":"gemini-3-flash-preview"}"#;

    #[test]
    fn gemini_response_line_emits_usage_event() {
        let mut state = SessionState::default();
        assert!(
            parse_usage_event_from_jsonl_line(SESSION_META_LINE, &mut state)
                .unwrap()
                .is_none()
        );

        let event = parse_usage_event_from_jsonl_line(GEMINI_LINE, &mut state)
            .expect("line should parse")
            .expect("gemini response should produce event");

        assert_eq!(event.provider.0, "google");
        assert_eq!(event.model.0, "gemini-3-flash-preview");
        assert_eq!(event.input_tokens, 11427);
        assert_eq!(event.cached_input_tokens, None);
        assert_eq!(event.output_tokens, 132);
        assert_eq!(event.reasoning_tokens, Some(539));
        assert_eq!(event.total_tokens, 12098);
        assert_eq!(event.source.app, "gemini-cli");
        assert_eq!(
            event.session_id.as_deref(),
            Some("00000000-0000-4000-8000-000000000001")
        );
        assert_eq!(
            event.request_id.as_deref(),
            Some("00000000-0000-4000-8000-000000000003")
        );
    }

    #[test]
    fn gemini_response_with_cached_tokens_subtracts_cached_from_input() {
        let mut state = SessionState::default();
        let _ = parse_usage_event_from_jsonl_line(SESSION_META_LINE, &mut state).unwrap();

        let event = parse_usage_event_from_jsonl_line(GEMINI_LINE_CACHED, &mut state)
            .unwrap()
            .expect("gemini response should produce event");

        assert_eq!(event.input_tokens, 12184 - 3800);
        assert_eq!(event.cached_input_tokens, Some(3800));
        assert_eq!(event.total_tokens, 12269);
        assert_eq!(event.reasoning_tokens, Some(62));
    }

    #[test]
    fn duplicate_gemini_id_emits_event_only_once() {
        let mut state = SessionState::default();
        let _ = parse_usage_event_from_jsonl_line(SESSION_META_LINE, &mut state).unwrap();

        let first = parse_usage_event_from_jsonl_line(GEMINI_LINE, &mut state).unwrap();
        assert!(first.is_some());

        let second = parse_usage_event_from_jsonl_line(GEMINI_LINE, &mut state).unwrap();
        assert!(second.is_none());
    }

    #[test]
    fn user_and_dollar_set_lines_are_skipped() {
        let mut state = SessionState::default();
        assert!(parse_usage_event_from_jsonl_line(USER_LINE, &mut state)
            .unwrap()
            .is_none());
        assert!(
            parse_usage_event_from_jsonl_line(DOLLAR_SET_LINE, &mut state)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn session_meta_populates_state() {
        let mut state = SessionState::default();
        let event = parse_usage_event_from_jsonl_line(SESSION_META_LINE, &mut state).unwrap();
        assert!(event.is_none());
        assert_eq!(
            state.session_id.as_deref(),
            Some("00000000-0000-4000-8000-000000000001")
        );
        assert_eq!(
            state.project_hash.as_deref(),
            Some("0000000000000000000000000000000000000000000000000000000000000001")
        );
        assert_eq!(state.session_kind.as_deref(), Some("main"));
    }

    #[test]
    fn zero_total_tokens_skipped() {
        let mut state = SessionState::default();
        let _ = parse_usage_event_from_jsonl_line(SESSION_META_LINE, &mut state).unwrap();
        let event = parse_usage_event_from_jsonl_line(ZERO_GEMINI_LINE, &mut state).unwrap();
        assert!(event.is_none());
    }

    #[test]
    fn discover_gemini_session_paths_walks_nested_dirs() {
        let temp_dir = std::env::temp_dir().join(format!(
            "skopos-collectors-gemini-discover-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let chats = temp_dir.join("tmp").join("proj").join("chats");
        std::fs::create_dir_all(&chats).unwrap();
        let session = chats.join("session-foo.jsonl");
        let other = temp_dir.join("state.json");
        std::fs::write(&session, "").unwrap();
        std::fs::write(&other, "ignore").unwrap();

        let paths = discover_gemini_session_paths(&temp_dir).unwrap();
        assert_eq!(paths, vec![session]);

        std::fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn parse_usage_events_from_session_path_dedupes_across_lines() {
        let temp_dir = std::env::temp_dir().join(format!(
            "skopos-collectors-gemini-stream-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let chats = temp_dir.join("tmp").join("skopos").join("chats");
        std::fs::create_dir_all(&chats).unwrap();
        let path = chats.join("session-test.jsonl");
        let body = format!(
            "{}\n{}\n{}\n{}\n",
            SESSION_META_LINE, GEMINI_LINE, GEMINI_LINE, GEMINI_LINE_DISTINCT
        );
        std::fs::write(&path, body).unwrap();

        let events = parse_usage_events_from_session_path(&path).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].total_tokens, 12098);
        assert_eq!(events[1].total_tokens, 310);

        std::fs::remove_dir_all(&temp_dir).unwrap();
    }
}
