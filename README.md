# Claude Monitor

A pixel-style desktop dashboard that watches your Claude Code agents in real time.

**Pure Rust** — Tauri 2 backend + Leptos 0.7 (CSR) WASM frontend. No JavaScript framework.

## What it shows

- **Pixel-agent grid** — one tile per Claude Code session, color-coded by status, animated sprite per state
  - 🟢 **Working** — assistant or tool currently in flight
  - 🟡 **Waiting** — turn ended, Claude is waiting on your input (or stuck on a permission prompt)
  - ⚪ **Idle** — no activity for `idle_timeout_secs` (treated as ended/historical)
  - 🔴 **Error** (reserved)
- **Sub-agent grouping** — Task-tool sub-agents are nested under their parent session with `↳ sub-agent <id>` indented rows. The group's headline status is the most-active member, so a parent "Waiting on its sub" is correctly counted as **Working**.
- **Filter pills** — All / Active / Idle on the Agents tab; filter applies to both top-level agents and their sub-agents.
- **Live status detail** — click any tile to see the agent's last assistant message, in-flight tool, tokens, cost, project path.
- **Real-time hooks** — opt-in: a one-click button registers Claude Code hook entries that POST authoritative events (PreToolUse, Stop, Notification, SubagentStart, …) to an embedded localhost HTTP server. Far more accurate than file-tailing heuristics.
- **JSONL fallback** — when hooks aren't registered, status is inferred from `~/.claude/projects/**/*.jsonl` events with a state machine that includes the `system/turn_duration` end-of-turn marker.
- **Usage panel** — today's tokens/cost + 7-day bar chart.
- **API panel** — optional Anthropic billing API view (paste a key, kept in memory only).
- **Tray icon + toast** — yellow toast pops up when an agent flips to Waiting.

## Architecture

```
claude-monitor/
├── Cargo.toml                     # workspace root
├── src-tauri/                     # native backend
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/default.json
│   └── src/
│       ├── main.rs                # Tauri commands, tray, app wiring
│       ├── watcher.rs             # tails ~/.claude/projects/**/*.jsonl
│       ├── parser.rs              # JSONL → ClaudeEvent
│       ├── agents.rs              # AgentRegistry, state machine, tick loop, HookEvent
│       ├── hooks.rs               # axum HTTP server for Claude Code hooks
│       ├── settings_writer.rs     # registers hooks in ~/.claude/settings.json
│       ├── db.rs                  # SQLite history (rusqlite, bundled)
│       └── api.rs                 # Anthropic billing API client
└── frontend/                      # Rust → WASM via Trunk
    ├── Cargo.toml
    ├── Trunk.toml
    ├── index.html
    ├── styles/main.css            # pixel-art theme (CSS-only sprites + animations)
    └── src/
        ├── main.rs                # Leptos app shell, tab routing, header indicators
        ├── tauri_bridge.rs        # invoke / listen wrappers around window.__TAURI__
        ├── types.rs               # AgentStatus, AgentSnapshot, AgentGroup, Filter, HooksStatus
        └── components/
            ├── agent_grid.rs      # group rendering with nested sub-agents
            ├── agent_detail.rs    # selected-agent inspector
            ├── usage_panel.rs     # local SQLite usage chart
            ├── api_usage_panel.rs # Anthropic billing-API view
            └── settings.rs        # hook setup + state machine thresholds
```

## Prerequisites

```bash
rustup target add wasm32-unknown-unknown      # WASM target
cargo install trunk                           # WASM bundler
cargo install tauri-cli --version "^2"        # Tauri build orchestrator
```

On Linux you'll also need standard Tauri 2 system deps (webkit2gtk, libssl, etc.) — see https://tauri.app/start/prerequisites/.

## Run

```bash
cargo tauri dev      # dev with hot reload
cargo tauri build    # release bundle
```

The dev command runs `cd frontend && trunk serve --port 1420` automatically (configured in `tauri.conf.json`) and launches the Tauri webview pointing at it.

## How status detection works

There are **two signal sources** that converge on a single state machine in `AgentRegistry`:

### 1. Real-time hooks (authoritative — auto-on)

Hooks register **automatically on every app launch** (the URL/port refreshes each time since the server binds to a random port). On first launch, the app:
1. Backs up `~/.claude/settings.json` to `settings.json.bak` (only if no backup exists yet)
2. Adds 11 hook entries (one per event) tagged with `_claude_monitor: true` so they can be removed cleanly. Each is `"type": "http"` pointing at `http://127.0.0.1:<random>/h` with an `X-Auth: <random>` header.
3. Claude Code picks the changes up live — no restart.

If you click **"Disable hooks"** in the Settings panel, the preference is persisted to `<data_local_dir>/claude-monitor/prefs.json` (`hooks_enabled: false`) and auto-register is skipped on subsequent launches. Click "Set up hooks" to re-enable.

| Hook event | New status |
|---|---|
| `SessionStart` / `UserPromptSubmit` | Working (new turn — clears Waiting from previous Stop) |
| `PreToolUse` | Working (cancel waiting timers, push pending tool) |
| `PostToolUse` / `PostToolUseFailure` | (turn continues, pop pending tool) |
| `Stop` | Waiting (turn ended) |
| `Notification(permission_prompt | idle_prompt)` | Waiting |
| `PermissionRequest` | Waiting |
| `SubagentStart` | child agent spawned with `parent_id` set |
| `SubagentStop` / `SessionEnd` | natural decay → Idle |

Hook events bump `last_hook_at`. While that's recent (< `hook_grace_secs`, default 30s), hooks are treated as ground truth.

### 2. JSONL fallback (always on)

When hooks aren't registered (or for sessions that started before they were), status is inferred from `~/.claude/projects/<hash>/<session>.jsonl`:

| JSONL `type` + content | Event | Effect |
|---|---|---|
| any record with `cwd` | `SessionStart` | seeds project path |
| `system` `subtype: turn_duration` | `TurnEnd` | sets `awaiting_user = true` → Waiting |
| `assistant` `content[].text` | `AssistantText` | updates preview; arms 5s text-idle deadline on tool-free turns |
| `assistant` `content[].tool_use` | `ToolUseStart` | sets `had_tool_in_turn`; pushes pending tool |
| `assistant` `usage` | `Usage` | increments token counters + cost |
| `user` `content[].tool_result` | `ToolUseEnd` | removes pending tool |
| `user` (no `tool_result`) | `UserMessage` | new turn → Working |

A 1Hz tick loop re-evaluates with priority:
1. Quiet for `idle_timeout_secs` → **Idle**
2. Pending tool past `permission_timeout_secs` → **Waiting**
3. `awaiting_user` flag set → **Waiting**
4. Text-idle deadline reached on tool-free turn → **Waiting**
5. Otherwise → **Working**

### Sub-agent detection

Path-based: files at `<projects>/<proj>/<parent_uuid>/subagents/agent-<id>.jsonl` are detected as sub-agents and registered with `parent_id = <parent_uuid>`. The frontend groups them under their parent.

## Settings (defaults)

| Setting | Default | Meaning |
|---|---|---|
| `idle_timeout_secs` | 180 | Quiet for this long → Idle (history) |
| `permission_timeout_secs` | 7 | Tool pending this long → Waiting |
| `text_idle_secs` | 5 | Text-only turn quiet for this long → Waiting |
| `hook_grace_secs` | 30 | When to treat hooks as authoritative |
| `message_preview_chars` | 280 | Trim length for assistant message preview |

## Tauri commands (frontend ↔ backend)

| Command | Returns |
|---|---|
| `list_agents` | `Vec<AgentSnapshot>` |
| `get_agent { session_id }` | `Option<AgentSnapshot>` |
| `get_agent_settings` / `set_agent_settings { settings }` | `AgentSettings` |
| `hooks_status` | `HooksStatus { registered, url, port }` |
| `register_hooks` / `unregister_hooks` | `HooksStatus` |
| `get_daily_summary` / `get_weekly_chart` / `get_sessions { limit }` | SQLite history |
| `set_api_key { key }` / `fetch_api_usage` | Anthropic billing API |

Events emitted to the frontend: `agent-status`, `agent-waiting`.

## Pricing assumptions (per million tokens)

| Model  | Input  | Output | Cache  |
|--------|--------|--------|--------|
| Opus   | $15.00 | $75.00 | $1.875 |
| Sonnet | $3.00  | $15.00 | $0.375 |
| Haiku  | $0.80  | $4.00  | $0.10  |

Edit `src-tauri/src/agents.rs::estimate_cost` to adjust.

## Caveats

- The hook HTTP server binds to a **random ephemeral port** on each app launch. The auto-register on launch refreshes the URL in `settings.json` so this is invisible — no manual action needed unless you've explicitly disabled hooks.
- The `tauri.conf.json` setting `app.withGlobalTauri: true` is required so the WASM bridge can use `window.__TAURI__.event.listen` — don't remove it.
- `beforeDevCommand` must run from `frontend/`, hence the `cd frontend &&` prefix — Tauri runs the command from the project root by default.
- Hook entries are tagged `_claude_monitor: true`. If you edit `~/.claude/settings.json` manually, leave that key alone or unregister via the app first.

## Sponsor

Claude Monitor is free and MIT-licensed. If it saves you time watching your
agents, consider sponsoring continued work — it directly funds new features
on the roadmap below (per-project rollup, native rate-limit alerts, CSV
export, sprite skins).

- **GitHub Sponsors** — https://github.com/sponsors/pakpoomsr
- **Buy Me a Coffee** — https://buymeacoffee.com/pakpoomsr
- **Issues / feedback** — https://github.com/pakpoomsr/claude-monitor/issues

If you're using this in a team or company context and would like priority
features (team rollup, cloud sync, integrations), open a discussion — happy
to talk.

## Roadmap

- [ ] Pin hook server to a fixed port so registrations survive restarts
- [ ] Per-project rollup view
- [ ] Native rate-limit alerts via `tauri-plugin-notification`
- [ ] Export CSV
- [ ] Sprite skin picker
- [ ] Detect Claude Code subscription plan
