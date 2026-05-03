# Claude Monitor — guide for Claude Code

This file orients you when working on this codebase. Read it before making non-trivial changes.

## What this app is

A desktop dashboard (Tauri 2 native shell + Leptos 0.7 WASM frontend, **pure Rust** — no JS framework) that watches Claude Code agents in real time and shows their status (Working / Waiting / Idle / Error). It correlates two signal sources:

1. **Real-time hooks** (authoritative) — embedded `axum` HTTP server receives `PreToolUse` / `Stop` / `Notification` etc. POSTs from Claude Code itself. Opt-in by clicking "Set up hooks" in Settings.
2. **JSONL file watcher** (fallback) — tails `~/.claude/projects/**/*.jsonl` and infers status from event timing.

Both feed the same `AgentRegistry` state machine. UI is blind to which signal updated state.

## Quickstart

```powershell
# Build / run
cargo tauri dev      # dev with frontend hot reload (runs trunk serve from frontend/)
cargo tauri build    # release bundle

# Verify just one half
cd src-tauri && cargo build         # backend only
cd frontend  && trunk build         # frontend WASM only
```

After a backend change, smoke-test with `timeout 25 cargo tauri dev --no-watch` from the project root — exit 143 (SIGTERM from timeout) is expected and fine; you're looking for the line `[claude-monitor] Watching: ...` and `[claude-monitor] hook server listening on http://127.0.0.1:<port>/h` to confirm both subsystems started.

## Module map

### Backend — `src-tauri/src/`

| File | Responsibility |
|---|---|
| `main.rs` | Tauri builder, `setup()` wiring, command registration, tray |
| `agents.rs` | **The heart.** `AgentRegistry`, `AgentSnapshot`, state machine (`apply_events`, `apply_hook`, `tick`, `compute_status`), `HookEvent`, cost estimation |
| `hooks.rs` | Axum HTTP server bound to `127.0.0.1:0` (random ephemeral port). Single `POST /h` endpoint with `X-Auth` header. Translates incoming JSON → `HookEvent` → `apply_hook` |
| `settings_writer.rs` | Reads/registers/unregisters our hook entries in `~/.claude/settings.json`. Tag `_claude_monitor: true` on every entry we own; backup to `.bak` on first write; atomic via `.tmp` rename |
| `watcher.rs` | `notify`-based JSONL watcher. Tracks per-file byte offset for incremental reads. Detects sub-agent paths (`<parent>/subagents/agent-X.jsonl`) and passes `parent_id` to `apply_events` |
| `parser.rs` | Line → `Vec<ClaudeEvent>`. Handles `system/turn_duration`, content blocks (text/tool_use/tool_result), `usage`, etc. |
| `db.rs` | SQLite (rusqlite, bundled) — only used for the Usage tab's history (token totals per session). Live state lives in memory. |
| `api.rs` | Anthropic billing API client (optional, key in memory only) |

### Frontend — `frontend/src/`

| File | Responsibility |
|---|---|
| `main.rs` | App shell, tab routing, header indicators, signal wiring, polls `hooks_status` every 2s |
| `tauri_bridge.rs` | Thin `wasm-bindgen` wrappers around `window.__TAURI__.core.invoke` and `__TAURI__.event.listen`. Defensive — `is_tauri()` guards no-op when run outside the webview. |
| `types.rs` | All shared types: `AgentStatus`, `AgentSnapshot`, `AgentSettings`, `AgentGroup` (parent + children), `Filter`, `HooksStatus`, `build_groups`, `apply_filter` |
| `components/agent_grid.rs` | Section renderer — parent tile + indented sub-agent tiles inside a `.group` card whose left edge color = aggregate status |
| `components/agent_detail.rs` | Side pane shown when a tile is selected |
| `components/usage_panel.rs` | SQLite-backed local usage view |
| `components/api_usage_panel.rs` | Anthropic billing API view |
| `components/settings.rs` | "Real-time hooks" toggle + state machine threshold inputs |

## The state machine (most important to understand)

State per agent lives in `AgentInner` (in `agents.rs`). The fields that drive status:

```rust
pending_tools:        HashMap<String, PendingTool>  // outstanding tool_use ids
had_tool_in_turn:     bool                          // any tool used this turn?
text_idle_deadline:   Option<DateTime<Utc>>         // when to flip Working→Waiting on text-only turn
awaiting_user:        bool                          // set by TurnEnd / Stop hook
last_hook_at:         Option<DateTime<Utc>>         // hook authority timestamp
last_activity:        DateTime<Utc>                 // any signal
```

`compute_status()` is the **single source of truth** — both `apply_events` (JSONL) and `apply_hook` (HTTP) end with `agent.snapshot.status = compute_status(...)`. Don't bypass it.

Priority in `compute_status`:
1. `now - last_activity >= idle_timeout_secs` → **Idle** (always wins)
2. Any pending tool flagged for permission → **Waiting**
3. `awaiting_user` set → **Waiting**
4. Text-idle deadline reached on tool-free turn → **Waiting**
5. Default → **Working**

The 1Hz `tick()` only mutates `pending_tools[].flagged_permission` and `snapshot.last_activity`-derived state, then re-runs `compute_status`. Don't add ad-hoc state-flipping logic in `tick` — push it into `compute_status` so JSONL and hook paths stay consistent.

## Conventions and traps

### Don't break these

- **`app.withGlobalTauri: true`** in `src-tauri/tauri.conf.json` — required so the WASM bridge can call `window.__TAURI__.event.listen`. Without it, the UI goes black.
- **`beforeDevCommand: "cd frontend && trunk serve --port 1420"`** — Tauri runs `beforeDevCommand` from the project root. The `cd` is required.
- **`<body></body>`** in `frontend/index.html` — no `<div id="root">`. Leptos `mount_to_body` appends to body; an empty `#root` with `height: 100%` would push content offscreen.
- **Atomic writes in `settings_writer.rs`** — write to `.tmp`, rename. Never partial-write user's settings.json.
- **Hook entry tag `_claude_monitor: true`** — needed for safe unregister. Don't remove it.
- **Backup is one-shot** — `register()` only writes `settings.json.bak` if it doesn't already exist. Rationale: don't clobber a user-edited backup.

### Tauri 2 quirks

- Use `tauri::async_runtime::spawn`, **not** `tokio::spawn`, when spawning from `setup()` (no Tokio reactor yet). The hook server and tick loop both follow this pattern.
- `tauri::generate_handler![...]` must list every command. Forgetting causes silent runtime failures.
- Tauri serializes command params with `rename_all = "camelCase"` by default. If you add a command like `get_thing(session_id: String)`, the frontend must invoke with `{ sessionId: "..." }`.

### Leptos 0.7 quirks

- Use `signal()` (function), not `create_signal`.
- `mount_to_body` is at `leptos::mount::mount_to_body`.
- `spawn_local` is at `leptos::task::spawn_local`.
- `view!` macro: branches with different concrete types need `.into_any()`.
- For reactive props, prefer `Signal<T>` over closures or static values when the prop should update mid-render.

### Frontend rendering of groups

`AgentGroup::aggregate_status()` returns the **most-active** member's status (Working > Error > Waiting > Idle). This is intentional: a parent in `Waiting` with a sub-agent doing real work should be visually counted as Working — the user is not blocked.

`apply_filter()` then applies the filter pill choice: drops non-matching children inside each group, then drops groups where neither parent nor any remaining child matches.

## Working on hooks

Real-time hook events are dispatched through `agents::apply_hook`. To handle a new event type:

1. Add a match arm in `apply_hook` to update the relevant fields (`had_tool_in_turn`, `awaiting_user`, etc.)
2. Add the event name to `HOOK_EVENTS` in `settings_writer.rs` so registration includes it
3. Make sure to set `agent.last_hook_at = Some(now)` so hook authority kicks in (already done at the bottom of `apply_hook`)

The HTTP handler in `hooks.rs` is intentionally lenient — unknown payloads are logged and 200'd, never blocking Claude.

## Working on JSONL parsing

When extending `parser.rs`:
- Test with a real JSONL file from `~/.claude/projects/`. The structure is documented in `README.md`'s "How status detection works" section.
- One JSONL line can yield **multiple** `ClaudeEvent`s (e.g. an `assistant` line with text + tool_use + usage emits 3 events).
- `cwd` is on every record type — top-of-function harvest is intentional so project labels populate from any record.

## Verification checklist for substantive changes

1. `cd src-tauri && cargo build` — clean
2. `cd frontend && trunk build` — clean (warnings OK)
3. `timeout 25 cargo tauri dev --no-watch` from project root — confirm:
   - `[claude-monitor] Watching: <path>` appears
   - `[claude-monitor] hook server listening on http://127.0.0.1:<port>/h` appears
   - No panic, no `error: process didn't exit successfully` other than exit 143 (timeout)
4. If you touched the state machine, sanity-check by running real `claude` in another terminal and watching the dashboard react.

## Known limitations

- Hook port is ephemeral — registrations need a refresh after each app restart.
- `Error` status is a reserved variant; nothing emits it yet.
- We aren't currently consuming `assistant/thinking` content blocks (extended-thinking output) — only `text` and `tool_use`.
- `apply_hook`'s field names follow the Claude Code docs (`tool_use_id`, `agent_id`, etc.). If real payloads differ, parse errors log to stderr — search for `"hook payload parse error"` in the terminal output.

## Roadmap (where we're heading)

See README. Short list: pin the hook port, per-project rollup, native rate-limit alerts, sprite skins, CSV export.
