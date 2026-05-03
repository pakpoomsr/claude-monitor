# Claude Monitor

A pixel-style desktop dashboard that watches your Claude Code agents in real time.

**Pure Rust** — Tauri 2 backend + Leptos (CSR) WASM frontend. No JavaScript framework.

## What it shows

- **Pixel-agent grid** — one tile per active Claude Code session, color-coded by status
  - 🟢 **Working** — assistant message or tool currently in flight
  - ⚪ **Idle** — no events for N seconds (configurable)
  - 🟡 **Needs permission** — tool started but no `tool_result` after N seconds (heuristic)
  - 🔴 **Error**
- **Live status message** — last assistant text + name of in-flight tool
- **Usage panel** — today's tokens/cost + 7-day bar chart
- **API panel** — optional Anthropic billing-API view (paste a key, it stays in memory)
- **Tray icon + notifications** — toast when an agent needs permission

## Architecture

```
claude-monitor/
├── Cargo.toml                # workspace root
├── src-tauri/                # native backend
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/default.json
│   └── src/
│       ├── main.rs           # Tauri commands, tray, app wiring
│       ├── watcher.rs        # tails ~/.claude/projects/**/*.jsonl
│       ├── parser.rs         # JSONL → ClaudeEvent
│       ├── agents.rs         # in-memory AgentRegistry + idle/permission tick loop
│       ├── db.rs             # SQLite history (rusqlite, bundled)
│       └── api.rs            # Anthropic billing API client
└── frontend/                 # Rust → WASM via Trunk
    ├── Cargo.toml
    ├── Trunk.toml
    ├── index.html
    ├── styles/main.css       # pixel-art theme (CSS-only sprites)
    └── src/
        ├── main.rs           # Leptos app shell
        ├── tauri_bridge.rs   # invoke / listen wrappers
        ├── types.rs
        └── components/
            ├── agent_grid.rs
            ├── agent_detail.rs
            ├── usage_panel.rs
            ├── api_usage_panel.rs
            └── settings.rs
```

## Prerequisites

```bash
# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# WASM target (for the Leptos frontend)
rustup target add wasm32-unknown-unknown

# Trunk — bundler for WASM apps
cargo install trunk

# Tauri CLI
cargo install tauri-cli --version "^2"
```

On Linux you'll also need the standard Tauri 2 system deps (webkit2gtk, libssl, etc.) — see https://tauri.app/start/prerequisites/.

## Run

```bash
# Dev (hot-reloads frontend, restarts backend on Rust change)
cargo tauri dev

# Production build
cargo tauri build
```

The dev command runs `trunk serve` automatically (configured in `tauri.conf.json`) and launches the Tauri shell pointing at it.

## How status detection works

Claude Code transcripts live at `~/.claude/projects/<hash>/<session-id>.jsonl`.

The watcher tails these files. Each new line is parsed into events:

| JSONL `type` + content | Event | Effect |
|---|---|---|
| `system` | `SessionStart` | First-seen project path |
| `assistant` `content[]` `text` | `AssistantText` | Updates current_message preview |
| `assistant` `content[]` `tool_use` | `ToolUseStart` | Adds pending tool, marks Working |
| `assistant` `usage` | `Usage` | Increments token counters + cost |
| `user` `content[]` `tool_result` | `ToolUseEnd` | Removes pending tool |

A 1Hz tick loop in `agents.rs` then promotes agents to:
- **Idle** if `last_activity` is older than `idle_timeout_secs`
- **NeedsPermission** if any pending tool has been open longer than `permission_timeout_secs`

Both thresholds are configurable in the Settings tab.

## Tauri commands (frontend ↔ backend)

| Command | Returns |
|---|---|
| `list_agents` | `Vec<AgentSnapshot>` |
| `get_agent { session_id }` | `Option<AgentSnapshot>` |
| `get_agent_settings` / `set_agent_settings` | `AgentSettings` |
| `get_daily_summary` / `get_weekly_chart` / `get_sessions` | local SQLite stats |
| `set_api_key { key }` / `fetch_api_usage` | Anthropic billing-API view |

Events emitted to the frontend: `agent-status`, `permission-needed`.

## Pricing assumptions (per million tokens)

| Model  | Input  | Output | Cache  |
|--------|--------|--------|--------|
| Opus   | $15.00 | $75.00 | $1.875 |
| Sonnet | $3.00  | $15.00 | $0.375 |
| Haiku  | $0.80  | $4.00  | $0.10  |

Edit `src-tauri/src/agents.rs::estimate_cost` to adjust.

## Roadmap

- [ ] Per-project rollup view (group sessions by project)
- [ ] Native rate-limit alerts (cross-platform via `tauri-plugin-notification`)
- [ ] Export CSV
- [ ] Sprite skin picker (different pixel art per status)
- [ ] Detect permission events directly when Claude Code starts logging them
