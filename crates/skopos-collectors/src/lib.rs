use anyhow::Result;
use skopos_core::{RawEvent, UsageEvent};

pub mod claude_code;
pub mod codex;
pub mod gemini;
pub mod hermes;

#[async_trait::async_trait]
pub trait Collector: Send + Sync {
    fn name(&self) -> &'static str;

    async fn collect(&self) -> Result<Vec<RawEvent>>;

    async fn normalize(&self, raw_event: RawEvent) -> Result<Vec<UsageEvent>>;
}

#[derive(Debug, Default)]
pub struct ManualCollector;

#[async_trait::async_trait]
impl Collector for ManualCollector {
    fn name(&self) -> &'static str {
        "manual"
    }

    async fn collect(&self) -> Result<Vec<RawEvent>> {
        Ok(Vec::new())
    }

    async fn normalize(&self, _raw_event: RawEvent) -> Result<Vec<UsageEvent>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::claude_code::parse_usage_event_from_jsonl_line;

    #[test]
    fn parses_claude_code_assistant_usage_line() {
        let line = r#"{"parentUuid":"abc","isSidechain":false,"message":{"model":"claude-opus-4-7","id":"msg_123","type":"message","role":"assistant","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":12,"cache_creation_input_tokens":34,"cache_read_input_tokens":56,"output_tokens":78,"server_tool_use":{"web_search_requests":1},"service_tier":"standard"}},"uuid":"event-uuid","timestamp":"2026-05-13T19:58:08.012Z","cwd":"/home/example/project","sessionId":"session-123","version":"2.1.140"}"#;

        let event = parse_usage_event_from_jsonl_line(line)
            .expect("line should parse")
            .expect("assistant usage line should produce event");

        assert_eq!(event.provider.0, "anthropic");
        assert_eq!(event.model.0, "claude-opus-4-7");
        assert_eq!(event.input_tokens, 12);
        assert_eq!(event.cached_input_tokens, Some(90));
        assert_eq!(event.output_tokens, 78);
        assert_eq!(event.total_tokens, 180);
        assert_eq!(event.source.app, "claude-code");
        assert_eq!(
            event.project_path.unwrap().to_string_lossy(),
            "/home/example/project"
        );
        assert_eq!(event.session_id.as_deref(), Some("session-123"));
        assert_eq!(event.request_id.as_deref(), Some("msg_123"));
    }

    #[test]
    fn ignores_claude_code_user_lines_without_usage() {
        let line = r#"{"type":"user","message":{"role":"user","content":"hello"},"timestamp":"2026-05-13T19:58:08.012Z"}"#;

        let event = parse_usage_event_from_jsonl_line(line).expect("valid json should parse");

        assert!(event.is_none());
    }

    #[test]
    fn ignores_zero_token_synthetic_assistant_usage_lines() {
        let line = r#"{"message":{"model":"<synthetic>","id":"msg_synthetic","role":"assistant","usage":{"input_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":0}},"timestamp":"2026-05-13T19:58:08.012Z"}"#;

        let event = parse_usage_event_from_jsonl_line(line).expect("valid json should parse");

        assert!(event.is_none());
    }

    #[test]
    fn parses_multiple_usage_events_from_jsonl_file() {
        let temp_dir =
            std::env::temp_dir().join(format!("skopos-collectors-test-{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let path = temp_dir.join("session.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"user","message":{"role":"user","content":"hello"},"timestamp":"2026-05-13T19:58:08.012Z"}"#,
                "\n",
                r#"{"message":{"model":"claude-sonnet-4-5","id":"msg_a","role":"assistant","usage":{"input_tokens":2,"output_tokens":3}},"timestamp":"2026-05-13T19:58:08.012Z","cwd":"/tmp/project","sessionId":"s1"}"#,
                "\n",
                r#"{"message":{"model":"claude-opus-4-7","id":"msg_b","role":"assistant","usage":{"input_tokens":5,"cache_read_input_tokens":7,"output_tokens":11}},"timestamp":"2026-05-13T19:58:09.012Z","cwd":"/tmp/project","sessionId":"s1"}"#,
                "\n"
            ),
        )
        .unwrap();

        let events = super::claude_code::parse_usage_events_from_jsonl_path(&path).unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(
            events.iter().map(|event| event.total_tokens).sum::<u64>(),
            28
        );
    }

    #[test]
    fn discovers_claude_code_project_jsonl_files() {
        let temp_dir = std::env::temp_dir().join(format!(
            "skopos-collectors-discover-test-{}",
            std::process::id()
        ));
        let projects_dir = temp_dir.join("projects").join("-tmp-project");
        std::fs::create_dir_all(&projects_dir).unwrap();
        let jsonl_path = projects_dir.join("session.jsonl");
        let ignored_path = projects_dir.join("memory.md");
        std::fs::write(&jsonl_path, "").unwrap();
        std::fs::write(ignored_path, "ignore me").unwrap();

        let paths = super::claude_code::discover_claude_code_jsonl_paths(&temp_dir).unwrap();

        assert_eq!(paths, vec![jsonl_path]);
    }
}
