CREATE TABLE IF NOT EXISTS raw_events (
  id TEXT PRIMARY KEY NOT NULL,
  source TEXT NOT NULL,
  captured_at TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  content_hash TEXT NOT NULL UNIQUE,
  parsed INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS usage_events (
  id TEXT PRIMARY KEY NOT NULL,
  dedupe_key TEXT NOT NULL UNIQUE,
  timestamp TEXT NOT NULL,
  provider TEXT NOT NULL,
  model TEXT NOT NULL,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cached_input_tokens INTEGER NULL,
  reasoning_tokens INTEGER NULL,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  estimated_cost_usd REAL NULL,
  currency TEXT NOT NULL DEFAULT 'USD',
  source_app TEXT NOT NULL,
  source_type TEXT NOT NULL,
  project_path TEXT NULL,
  session_id TEXT NULL,
  request_id TEXT NULL,
  metadata_json TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_usage_events_timestamp ON usage_events(timestamp);
CREATE INDEX IF NOT EXISTS idx_usage_events_provider_model ON usage_events(provider, model);
CREATE INDEX IF NOT EXISTS idx_usage_events_project_path ON usage_events(project_path);
