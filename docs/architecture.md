# Skopos Architecture

Skopos is a local-first AI usage observability system.

Initial shape:

1. `skopos-agent`: small Rust background process.
2. `skopos-store`: SQLite persistence.
3. `skopos-cli`: reliable control/debug interface.
4. `skopos-app`: future Tauri desktop dashboard.

The first pipeline is:

`RawEvent -> Normalizer -> UsageEvent -> SQLite -> CLI/API -> UI`

Privacy default: store token accounting metadata, not prompts or outputs.
