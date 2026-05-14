# Skopos

Local-first AI usage observability.

Skopos starts by tracking token usage per AI provider/model on this machine, then grows toward budgets, alerts, local dashboards, collectors, and optional proxy-based accounting.

## Architecture

- `crates/skopos-core`: domain types and pure accounting concepts.
- `crates/skopos-store`: SQLite persistence and query layer.
- `crates/skopos-pricing`: model price catalog and cost estimation.
- `crates/skopos-collectors`: provider/tool collectors that produce raw or normalized usage events.
- `crates/skopos-agent`: background daemon.
- `crates/skopos-cli`: CLI control plane.
- `apps/desktop`: Tauri v2 desktop UI (React + TypeScript + Vite frontend, Rust shell in `apps/desktop/src-tauri`).

## Rust toolchain

Rust does not have an LTS channel. Skopos pins the current stable toolchain in `rust-toolchain.toml` for reproducible local development.

Current pinned toolchain: Rust 1.95.0 stable.

## First checks

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Desktop app (Tauri v2)

`apps/desktop` is a Tauri v2 app: a React + TypeScript + Vite frontend driving
a thin Rust shell in `apps/desktop/src-tauri` (workspace member
`skopos-desktop`). The shell exposes `#[tauri::command]`s that will bridge the
UI to the Skopos workspace crates.

Prerequisites:

- Node.js + npm.
- Linux system libraries: `webkit2gtk-4.1`, `libsoup-3.0`, `gtk+-3.0`
  (plus `patchelf` for AppImage bundling).
- `tauri-cli` is provided per-project via `@tauri-apps/cli`; a global
  `cargo install tauri-cli --version "^2.0"` is optional.

```bash
npm run desktop:install   # install frontend deps (one-time)
npm run desktop:dev       # run the app with hot reload
npm run desktop:build     # produce a release bundle
```

Vite serves the frontend on port 1420; Tauri owns the native window.
