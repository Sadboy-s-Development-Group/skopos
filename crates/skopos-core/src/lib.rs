use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

pub type UsageEventId = Uuid;
pub type RawEventId = Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderId(pub String);

impl ProviderId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelId(pub String);

impl ModelId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageSourceKind {
    Log,
    Proxy,
    Api,
    Manual,
    SdkWrapper,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSource {
    pub app: String,
    pub kind: UsageSourceKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Money {
    pub amount: f64,
    pub currency: String,
}

impl Money {
    pub fn usd(amount: f64) -> Self {
        Self {
            amount,
            currency: "USD".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageEvent {
    pub id: UsageEventId,
    pub timestamp: DateTime<Utc>,
    pub provider: ProviderId,
    pub model: ModelId,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub total_tokens: u64,
    pub estimated_cost: Option<Money>,
    pub source: UsageSource,
    pub project_path: Option<PathBuf>,
    pub session_id: Option<String>,
    pub request_id: Option<String>,
    pub metadata: serde_json::Value,
}

impl UsageEvent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: ProviderId,
        model: ModelId,
        input_tokens: u64,
        output_tokens: u64,
        source: UsageSource,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            provider,
            model,
            input_tokens,
            output_tokens,
            cached_input_tokens: None,
            reasoning_tokens: None,
            total_tokens: input_tokens + output_tokens,
            estimated_cost: None,
            source,
            project_path: None,
            session_id: None,
            request_id: None,
            metadata: serde_json::Value::Object(Default::default()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawEvent {
    pub id: RawEventId,
    pub source: String,
    pub captured_at: DateTime<Utc>,
    pub payload: serde_json::Value,
    pub content_hash: String,
    pub parsed: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SkoposError {
    #[error("invalid usage event: {0}")]
    InvalidUsageEvent(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_event_sets_total_tokens() {
        let event = UsageEvent::new(
            ProviderId::new("openai"),
            ModelId::new("gpt-test"),
            10,
            15,
            UsageSource {
                app: "manual".to_string(),
                kind: UsageSourceKind::Manual,
            },
        );

        assert_eq!(event.total_tokens, 25);
    }
}
