<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/banner-dark.svg">
  <img alt="SKOPOS" src="docs/banner-light.svg" width="620">
</picture>

### The unified workspace for developers

*One terminal home for your projects, your AI spend, and your machine.*

![Rust](https://img.shields.io/badge/Rust-1.95-BD93F9?style=flat-square&logo=rust&logoColor=white&labelColor=2E1065)
![License](https://img.shields.io/badge/License-MIT-BD93F9?style=flat-square&labelColor=2E1065)
![Platform](https://img.shields.io/badge/Platform-linux%20%7C%20macOS-A78BFA?style=flat-square&labelColor=2E1065)
![Local-first](https://img.shields.io/badge/Local--first-%E2%9C%93-9D7CD8?style=flat-square&labelColor=2E1065)

</div>

---

**Skopos** pulls the scattered pieces of a developer's day into a single, fast,
local-first terminal app: jump into any project with the agentic CLI of your
choice, watch your AI rate limits and token spend across every provider, and
keep an eye on the machine you depend on — without a single byte leaving your
laptop.

> **skopós** · Greek *σκοπός* — *"the watcher"*: the one who observes, keeps
> watch, and never loses sight of the goal.

```
  $ skopos                      # open the interactive workspace
  $ skopos work                 # pick a project, launch an agentic CLI
  $ skopos usage                # 5h / weekly AI limits, at a glance
  $ skopos network              # live connectivity dashboard
```

---

## Contents

- [Why Skopos](#why-skopos)
- [Features](#features)
  - [Launchpad — `skopos work`](#-launchpad--skopos-work)
  - [AI observability — `skopos usage`](#-ai-observability--skopos-usage)
  - [Spend tracking — `skopos claude·codex·gemini·hermes`](#-spend-tracking--skopos-claude--codex--gemini--hermes)
  - [Network tracker — `skopos network`](#-network-tracker--skopos-network)
  - [The interactive shell](#-the-interactive-shell)
- [Install](#install)
- [Quick start](#quick-start)
- [Command reference](#command-reference)
- [Configuration & data](#configuration--data)
- [Architecture](#architecture)
- [Desktop app](#desktop-app)
- [Development](#development)
- [Privacy](#privacy)
- [About this project](#about-this-project)
- [Credits](#credits)
- [License](#license)

---

## Why Skopos

Modern development is fragmented. Your projects live in one folder, your
agentic CLIs each have their own dashboards, your API usage is buried behind
three web consoles, and the health of your dev box is anybody's guess.

Skopos is the **single pane of glass** for all of it — a polished TUI that is:

| | |
|---|---|
| **Fast** | A single Rust binary. No Electron, no daemon you didn't ask for. |
| **Local-first** | Everything lives in a SQLite file on your machine. Nothing is uploaded. |
| **Designed** | A consistent, purple-accented interface — splash, pickers, dashboards. |
| **Unified** | Projects, AI usage, cost and connectivity behind one command. |

---

## Features

### ▸ Launchpad — `skopos work`

Stop `cd`-ing around. `skopos work` lists the projects under your code
directory, detects each one's language, and hands the terminal straight to the
agentic CLI you pick — Claude, Codex, Gemini, Hermes or opencode.

```
  claude  ~/Coding

  ▶  ⬢  skopos
        ⬡  my-api
        ⬡  dotfiles
        ⬡  notes

  ↑/↓ project  ·  ←/→ provider  ·  enter open  ·  esc cancel
```

Skopos `exec`s into the chosen CLI inside the project directory — it gets out
of the way completely and your terminal is now that tool.

### ▸ AI observability — `skopos usage`

One screen for every rate limit you actually care about. Skopos surfaces
Anthropic's 5-hour / weekly windows, Codex's limits, your live Claude Code
session, and recent local activity — with brand-coloured progress bars.

```
  Usage

  Limits
    anthropic · Claude Max 5x

    5-hour    [████████░░░░░░░░░░░░░░░░]   33.1%   resets in 2h 14m
    weekly    [██████░░░░░░░░░░░░░░░░░░]   24.0%   resets in 4d 06h

  Current Session
    anthropic · Claude Max 5x   updated just now
    model    Opus 4.7 (1M context)
    ctx      [███░░░░░░░░░░░░░░░░░░░░░░]   14.8%   148K of 1.0M tokens
    cost     $7.35   3m 12s   +123 / -45 lines
```

> Anthropic's per-account quota isn't exposed to third-party tools, so Skopos
> reads it the supported way — a `statusLine` hook you install once with
> `skopos usage install`.

### ▸ Spend tracking — `skopos claude` · `codex` · `gemini` · `hermes`

Import the local logs every agentic CLI already writes, and Skopos accounts
for them: tokens by period, by model, and an estimated dollar cost from a
built-in (and overridable) price catalog.

```
  Claude usage this month
    2026-05-01 → 2026-06-01

  events        1,231
  input         12.4M
  cached       248.1M
  output         3.1M
  total        263.6M
  est cost     $48.20
```

Per-period (`-t` today · `-w` week · `-m` month) and per-model views for
Claude Code, Codex, Gemini and Hermes — all from logs already on disk.

### ▸ Network tracker — `skopos network`

A connectivity tracker for the machine you depend on. A background daemon
pings the wider internet on an interval and records every sample and every
outage; the dashboard classifies the link **stable · moderate · severe** so a
fading uplink shows *before* it drops.

```
  skopos network                                    srv-01 · eth0

  ┌─────────────────────────────────────────────────────────────┐
  │   ●  MODERATE          3 interruptions in the last hour       │
  │                        1m 48s total downtime · link up 12m    │
  └─────────────────────────────────────────────────────────────┘

  Current link
    latency 28 ms   loss 0%   sites 4/4   carrier up
    last probe 6s ago   ·   daemon running

  Last 60 min                              one cell ≈ 1 min, oldest left
    ████████████▇█████░██████████████▇██░████████████████░██████

  window      outages   downtime    availability      worst rtt
  1 hour            3     1m 48s    [████████████] 97%      310 ms
  24 hours         11    14m 02s    [███████████░] 99%      880 ms
  7 days           58     2h 19m    [████████████] 98%      1.2 s
```

Install it as a systemd service with `skopos network install`, or run
`skopos network watch` directly. `skopos network status` prints a one-shot
verdict with a `0`/`1`/`2` exit code — perfect for an MOTD banner or a cron
check.

### ▸ The interactive shell

Run `skopos` with no arguments to drop into the workspace itself: a branded
splash, a live-redrawn input box, command history, and short aliases for
everything below.

```
  ███ Skopos                          Commands
  ███                                   work       pick a project, launch CLI
  ███  the unified                      usage      5h / weekly limit bars
  ███  developer workspace              network    connectivity dashboard
  ███                                   claude -m  usage by period
                                        providers  tracked providers
```

---

## Install

### One-line installer (recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/Sadboy-s-Development-Group/skopos/master/install.sh | sh
```

Downloads the latest release tarball, verifies the SHA-256 checksum, and
installs `skopos` into `~/.local/bin`. Override the target directory with
`INSTALL_DIR=…` or pin a version with `SKOPOS_VERSION=v0.2.0-beta.1`.

Linux x86_64 is the first published target; more platforms land as the
release matrix expands.

### From crates.io

```bash
cargo install skopos-cli --locked
```

Compiles from source — slower than the prebuilt tarball, but always
available on any platform with a Rust toolchain.

### From source

```bash
git clone https://github.com/Sadboy-s-Development-Group/skopos.git
cd skopos
cargo install --path crates/skopos-cli --locked   # installs `skopos` into ~/.cargo/bin
```

The pinned toolchain (`rust-toolchain.toml`) is **Rust 1.95.0 stable** —
`rustup` picks it up automatically.

### Updating

```bash
skopos update           # pull the latest release and replace the binary in place
skopos update --check   # only report whether a newer release is out
skopos --version        # what you're running now
```

`skopos update` consults the GitHub Releases feed, finds the asset that
matches your platform, downloads it and atomically swaps the running
binary. The same command works regardless of how you installed it.

## Quick start

```bash
skopos                       # open the interactive workspace
skopos work                  # pick a project and launch an agentic CLI

skopos usage install         # register the Claude Code statusline hook (once)
skopos usage                 # AI rate limits + live session
skopos claude import         # pull Claude Code logs into the local store
skopos claude -m             # this month's Claude tokens + estimated cost

skopos network install       # set up the connectivity probe service
skopos network               # open the network dashboard
```

## Command reference

<details>
<summary><b>Workspace</b></summary>

| Command | Description |
|---|---|
| `skopos` | Open the interactive shell. |
| `skopos work` | Pick a project, then `exec` into an agentic CLI inside it. |
| `skopos status` / `doctor` | Local status and the paths Skopos uses. |
| `skopos --version` | Print the version Skopos is running. |
| `skopos update [--check]` | Replace this binary with the latest published release. |

</details>

<details>
<summary><b>AI usage & cost</b></summary>

| Command | Description |
|---|---|
| `skopos usage` | 5h / weekly limit bars, live session, local activity. |
| `skopos usage install` / `uninstall` | Manage the Claude Code statusline hook. |
| `skopos providers` | Providers tracked in the local store. |
| `skopos claude·codex·gemini·hermes import` | Import that tool's local usage logs. |
| `skopos claude·codex·gemini·hermes -t/-w/-m` | Usage today / this week / this month. |
| `skopos claude·codex·gemini·hermes models` | Usage grouped by model. |
| `skopos codex usage` / `refresh` | Codex 5h / weekly limits from the app-server. |

</details>

<details>
<summary><b>Network</b></summary>

| Command | Description |
|---|---|
| `skopos network` | Live connectivity dashboard (stable / moderate / severe). |
| `skopos network watch` | Run the probe daemon in the foreground. |
| `skopos network status` | One-shot verdict; exit code `0`/`1`/`2`. |
| `skopos network install [--user]` | Generate the systemd unit for the daemon. |
| `skopos network uninstall [--user]` | Remove the systemd unit. |

</details>

## Configuration & data

| Path | Purpose |
|---|---|
| `~/.config/skopos/config.toml` | Project root, default provider, `[network]` probe settings. |
| `~/.config/skopos/pricing.toml` | Optional price-catalog overrides for cost estimates. |
| `~/.local/share/skopos/skopos.db` | The SQLite store — usage events and network history. |
| `~/.local/state/skopos/skopos.log` | Logs. |

All files are created with sensible defaults on first use.

## Architecture

Skopos is a Cargo workspace. Raw events are normalized, persisted, and read
back by the CLI and (soon) the desktop UI:

```
RawEvent → Normalizer → UsageEvent → SQLite → CLI / API → UI
```

| Crate | Responsibility |
|---|---|
| `skopos-core` | Domain types and pure accounting concepts. |
| `skopos-store` | SQLite persistence and the query layer. |
| `skopos-pricing` | Model price catalog and cost estimation. |
| `skopos-collectors` | Provider/tool collectors that produce usage events. |
| `skopos-agent` | Background daemon. |
| `skopos-cli` | The CLI control plane and TUI. |
| `apps/desktop` | Tauri v2 desktop UI (React + TypeScript + Vite). |

## Desktop app

`apps/desktop` is a Tauri v2 app — a React + TypeScript + Vite frontend over a
thin Rust shell — and is in active development.

```bash
npm run desktop:install   # install frontend deps (one-time)
npm run desktop:dev       # run with hot reload
npm run desktop:build     # produce a release bundle
```

Linux needs `webkit2gtk-4.1`, `libsoup-3.0`, `gtk+-3.0` (plus `patchelf` for
AppImage bundling).

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Or run all three at once:

```bash
npm run check
```

## Privacy

Skopos is local-first by design. It stores **token-accounting metadata** —
counts, models, timestamps, cost estimates — and **never** prompts, completions
or any conversation content. There is no telemetry and no network upload; the
only outbound traffic is the connectivity probe pinging public hosts, and only
when you run it.

## About this project

Skopos is a **personal and educational project** built to explore how AI
providers (Anthropic, OpenAI, Google, Hermes) surface token usage and
rate-limit data, and how that data can be unified into a single
observability layer that lives entirely on the developer's machine. It
is not a commercial product — expect rough edges, breaking changes, and
the occasional opinion baked into a default.

## Credits

Skopos was built by **Kevin Coss** ([@D4ffi](https://github.com/D4ffi)),
member of the [Sadboys Development Group](https://github.com/Sadboy-s-Development-Group).

## License

[MIT](LICENSE) © Skopos contributors

<div align="center">
<sub>Built with Rust · designed for the terminal · keeps watch so you don't have to.</sub>
</div>
