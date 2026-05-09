# Claude Monitor — guide for Claude Code

This file orients you when working on this codebase. Read it before making non-trivial changes.

## What this app is

A desktop dashboard (Tauri 2 native shell + Leptos 0.8 WASM frontend, **pure Rust** — no JS framework) that watches Claude Code agents in real time and shows their status (Working / Waiting / Idle / Error). It correlates two signal sources:

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

After a backend change, smoke-test with `timeout 25 cargo tauri dev --no-watch` from the project root — exit 143 (SIGTERM from timeout) is expected and fine; you're looking for these three lines to confirm subsystems started:

```
[claude-monitor] Watching: C:\Users\<you>\.claude\projects
[claude-monitor] hook server listening on http://127.0.0.1:<port>/h
[claude-monitor] auto-registered hooks at http://127.0.0.1:<port>/h
```

(The third line only fires when `prefs.json` has `hooks_enabled: true` — the default. If the user previously clicked "Disable hooks", you'll see `hooks disabled by user prefs, skipping auto-register` instead.)

## Module map

### Backend — `src-tauri/src/`

| File | Responsibility |
|---|---|
| `main.rs` | Tauri builder, `setup()` wiring, command registration, tray. Loads pricing overrides + refreshes currency cache on startup. |
| `agents.rs` | **The heart.** `AgentRegistry`, `AgentSnapshot`, state machine (`apply_events`, `apply_hook`, `tick`, `compute_status`), `HookEvent`, `LogEntry` / `LogSource`. Holds a live `PricingTable` used by the Usage event handler to compute cost. Each `AgentInner` carries a 500-entry `VecDeque<LogEntry>` ring buffer fed through the single `AgentInner::record()` helper — narrative filter and ring eviction live there, never replicated at call sites. `apply_events` and `apply_hook` return `(Option<AgentSnapshot>, Vec<LogEntry>)`; callers fan the entries out to `app.emit("agent-event", ...)` and `db.insert_events(...)`. |
| `hooks.rs` | Axum HTTP server bound to `127.0.0.1:0` (random ephemeral port). Single `POST /h` endpoint with `X-Auth` header. Constant-time token compare via `subtle`; 64 KB body limit; payload contents never logged. `ServerState` carries the `Arc<Mutex<Database>>` so hook-driven `LogEntry`s persist alongside the live emit. |
| `settings_writer.rs` | Reads/registers/unregisters our hook entries in `~/.claude/settings.json`. Tag `_claude_monitor: true` on every entry we own; backup to `.bak` on first write; symlink-safe atomic writes via `OpenOptions::create_new(true)` + rename; 0600 perms on Unix. |
| `prefs.rs` | Persistent app prefs at `<data_local_dir>/claude-monitor/prefs.json`. Fields: `hooks_enabled`, `pricing_overrides` (HashMap keyed by `PricingEntry.id`), `pricing_currency` (ISO 4217), `currency_cache` (Frankfurter rates + fetched_at), `snapshots_enabled`, `snapshot_retention_days`. |
| `watcher.rs` | `notify`-based JSONL watcher. Per-file byte offset for incremental reads. **8 MB per-line cap** (resyncs on next newline). Detects sub-agent paths (`<parent_uuid>/subagents/agent-X.jsonl`) and passes `parent_id` to `apply_events` only when the parent component is UUID-shaped. |
| `parser.rs` | Line → `Vec<ClaudeEvent>`. Handles `system/turn_duration`, content blocks (text/tool_use/tool_result), `usage` block (5 token fields including the nested `cache_creation.ephemeral_5m_input_tokens` / `_1h_input_tokens` buckets). |
| `pricing.rs` | `ModelPricing` (5 fields), `TokenUsage` (5 fields), `PricingTable` (Vec of `PricingEntry`), `default_pricing_table()` with all 13 SKUs, `merge_overrides`, `estimate_cost`. **Single source of truth** for cost math — never duplicate this logic in agents.rs. |
| `currency.rs` | Frankfurter HTTP client + curated 10-currency list (USD/EUR/GBP/JPY/CNY/THB/SGD/INR/KRW/AUD). `is_stale` returns true after 24h. |
| `db.rs` | SQLite (rusqlite, bundled). Three tables: `sessions` (Usage-tab history), `agent_events` (per-agent event log history), and `file_snapshots` (History-tab pre/post blobs metadata). Schema migration adds `cache_write_5m_tokens`, `cache_write_1h_tokens`, `cache_read_tokens` to `sessions` if missing; legacy `cache_tokens` column kept as the sum. `agent_events` is indexed `(session_id, ts DESC)` and written transactionally via `insert_events`. `file_snapshots` is indexed `(session_id, ts DESC)` and `(tool_use_id)`. |
| `snapshots.rs` | File snapshot capture/diff/restore subsystem powering the History tab. Single writer to `<data_local>/claude-monitor/snapshots/<session>/`. 1 MB cap per file; restore is reversible via a `pre-restore` snapshot row. Hook-driven — `apply_hook` does not touch this; `hooks::hook_handler` dispatches `capture_pre_edit` / `capture_post_edit` after `apply_hook` returns. |
| `api.rs` | Anthropic billing API client (optional, key in memory only) |

### Frontend — `frontend/src/`

| File | Responsibility |
|---|---|
| `main.rs` | App shell, 6-tab routing (Agents / Usage / History / API / Settings / Sponsor), header indicators, signal wiring, polls `hooks_status` every 2s. **Provides global `RwSignal<PricingTable>`, `RwSignal<CurrencyState>`, and `RwSignal<EventLogMap>` (alias for `HashMap<session_id, VecDeque<LogEntry>>`, capped at 500 per session to mirror the backend ring) via `provide_context`** — leaf components consume via `use_context`. Listens for `agent-status`, `agent-waiting`, and `agent-event` Tauri events. |
| `tauri_bridge.rs` | Thin `wasm-bindgen` wrappers around `window.__TAURI__.core.invoke` and `__TAURI__.event.listen`. Defensive — `is_tauri()` guards no-op when run outside the webview. |
| `types.rs` | All shared types: `AgentStatus`, `AgentSnapshot` (5 cache fields), `AgentSettings`, `AgentGroup`, `Filter`, `HooksStatus`, `ModelPricing`, `PricingEntry`, `PricingTable`, `CurrencyInfo`, `CurrencyState`, `LogEntry`, `LogSource`. Helpers: `build_groups`, `apply_filter`, `format_money` (thousand-separator commas, currency-aware), `format_date_short` ("DD MMM YY"), `format_datetime` ("DD MMM YYYY HH:MM:SS"), `format_log_time` ("HH:MM:SS" only — used inside the per-agent event log where the date is implied). |
| `components/agent_grid.rs` | Section renderer — parent tile + indented sub-agent tiles inside a `.group` card whose left edge color = aggregate status. Tile cost respects active currency. |
| `components/agent_detail.rs` | Side pane shown when a tile is selected. **5-row cost table** (Base Input / 5m Cache Write / 1h Cache Write / Cache Hit & Refresh / Output) reading live from the pricing context. **Recent events** section: `Effect` backfills the last 200 entries via `get_agent_events` on first selection (untracked read of the log map so the effect doesn't re-fire on every streamed event); a `For` over the per-session `VecDeque<LogEntry>` from the context renders newest-first. |
| `components/usage_panel.rs` | SQLite-backed local usage view. Range pills (Last 7d / 30d / Custom) + two `<input type="date">` for custom range; bars show date and cost without hover. |
| `components/history_panel.rs` | History tab (issue #3). Sessions grouped by project, expand to a list of edits with timestamp + tool name + file basename. Click an edit → `DiffView` renders the unified diff; **Revert** restores the `pre` snapshot. Listens for `snapshot-restored` Tauri events to refetch. |
| `components/diff_view.rs` | Pure-CSS unified-diff renderer (`+` green / `-` red / context). Backend produced the unified text via the `similar` crate; this component just splits and styles. |
| `components/api_usage_panel.rs` | Anthropic billing API view |
| `components/settings.rs` | Real-time hooks toggle, state-machine thresholds, **editable pricing table** (13 rows × 5 cells, save-on-blur), **display currency** dropdown + manual refresh, **snapshots** toggle + retention + disk-usage figure. |
| `components/sponsor.rs` | Sponsor tab — pitch + 3 buttons opening URLs via `plugin:opener|open_url`. |

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

- **`app.withGlobalTauri: true`** in `src-tauri/tauri.conf.json` — required so the WASM bridge can call `window.__TAURI__.event.listen`. Without it, the UI goes black. Defense-in-depth tradeoff: the strict CSP (below) blocks foreign scripts, so the global isn't reachable from injected content.
- **Strict CSP** in `tauri.conf.json` (`default-src 'self'`, `script-src 'self' 'wasm-unsafe-eval'`, `connect-src` whitelisted to Anthropic + Frankfurter). When you add a new outbound destination, update `connect-src` or fetches will silently 0-byte.
- **`beforeDevCommand: "cd frontend && trunk serve --port 1420"`** — Tauri runs `beforeDevCommand` from the project root. The `cd` is required.
- **`<body></body>`** in `frontend/index.html` — no `<div id="root">`. Leptos `mount_to_body` appends to body; an empty `#root` with `height: 100%` would push content offscreen.
- **Symlink-safe atomic writes in `settings_writer.rs`** — `OpenOptions::create_new(true)` to the `.tmp`, then rename. Don't switch back to `fs::write` (it follows symlinks).
- **Hook entry tag `_claude_monitor: true`** — needed for safe unregister. Don't remove it.
- **Backup is one-shot** — `register()` only writes `settings.json.bak` if it doesn't already exist. Rationale: don't clobber a user-edited backup.
- **Bundle identifier** is `com.claudemonitor.desktop` — not `.app`. The `.app` suffix conflicts with the macOS application bundle extension. Don't change it back.
- **`compute_status` is the single source of truth** — see "The state machine" section. Same rule applies to **`pricing::estimate_cost`**: there's exactly one cost-math function and the frontend mirrors only the rates, not the math.

### Tauri 2 quirks

- Use `tauri::async_runtime::spawn`, **not** `tokio::spawn`, when spawning from `setup()` (no Tokio reactor yet). The hook server and tick loop both follow this pattern.
- `tauri::generate_handler![...]` must list every command. Forgetting causes silent runtime failures.
- Tauri serializes command params with `rename_all = "camelCase"` by default. If you add a command like `get_thing(session_id: String)`, the frontend must invoke with `{ sessionId: "..." }`.

### Leptos 0.8 quirks

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

## Working on pricing

When Anthropic publishes new prices or a new model SKU:

1. Update `pricing::default_pricing_table()` in `src-tauri/src/pricing.rs`. Order matters — more-specific ids must precede less-specific ones (e.g. `claude-opus-4-7` before `claude-opus-4`) because matching is substring-based and first-match-wins.
2. The `id` field doubles as the override key in `prefs.json::pricing_overrides`. If you rename an id, existing user overrides for the old key become orphaned (silently ignored) — usually fine, but flag it in the commit message.
3. `ModelPricing` has exactly five fields. If Anthropic adds a sixth column (cache TTL bucket, batch discount, etc.), updating the struct cascades to: `parser.rs` (extract from usage block), `agents.rs` (Usage event handler accumulates the new counter), `db.rs` (new column + migration), `frontend/types.rs` mirror, `agent_detail.rs` cost table row, `settings.rs` editable cell.
4. Don't rewrite the cost math anywhere else — `pricing::estimate_cost(usage, &pricing)` is the only function that knows. The frontend's per-row cost in `agent_detail.rs` does its own breakdown for display, but the `cost_usd` written to SQLite and shown in summaries comes from the backend.
5. The five token counters in `AgentSnapshot` — `input_tokens`, `output_tokens`, `cache_write_5m_tokens`, `cache_write_1h_tokens`, `cache_read_tokens` — are independent (no overlap). The legacy `cache_tokens = sum(three cache fields)` is kept on the snapshot and DB row for any consumer that wants a single combined cache figure.

## Working on the event log

The "Recent events" detail-pane stream is fed by a per-agent log pipeline:

```
ClaudeEvent / HookEvent
        │
        ▼
  AgentInner::record()              ← single capture point + narrative filter
        │
        ├──► VecDeque<LogEntry> (cap 500, ring evicts head)
        └──► returned Vec<LogEntry>
                  │
                  ├──► app.emit("agent-event", entry)   ← live UI
                  └──► db.insert_events(&entries)        ← SQLite history
```

Capture rules:

- **Narrative filter** lives only inside the match arms of `apply_events` / `apply_hook` that opt into `record(...)`. JSONL `Usage` and `Unknown` are intentionally skipped (aggregate counters / parse misses, not narrative). `SessionStart` is also skipped because the parser emits one per JSONL line — logging it would drown the stream. Every `HookEvent` is captured (kind = `format!("Hook:{}", ev.hook_event_name)`).
- **Don't add an event-emit path inside the 1Hz `tick()` loop** — tick mutates flags only, not history. Any capture there would duplicate entries.
- **Don't bypass the ring cap** — `record()` is the only writer; `events: VecDeque<LogEntry>` is private to `AgentInner`.
- **Char caps**: `summary` truncated to 500 chars (with ellipsis), `details` to 4096. Matters when an assistant message or hook payload is unusually large — the upstream 8 MB JSONL line cap and 64 KB hook body cap would otherwise let a single entry blow the ring's memory budget.

Read API:

- **`AgentRegistry::events_for(session_id, limit)`** returns the most-recent N from the in-memory ring, oldest first.
- **`Database::events_for(session_id, limit)`** returns the same shape from SQLite (canonical superset; everything in the ring is also persisted).
- **`get_agent_events` Tauri command** combines both: live ring saturating the window short-circuits; otherwise it returns the SQLite history (which already contains the ring contents). Pass `includeHistory: false` to skip SQLite.

Frontend:

- The `agent-event` listener in `frontend/src/main.rs` appends each entry to `RwSignal<EventLogMap>` (per-session deque, frontend ring cap = 500 mirrors the backend).
- `agent_detail.rs` reads the log map via `use_context::<RwSignal<EventLogMap>>()`. Backfill is gated on the bucket being empty (`with_untracked` so the backfill effect doesn't re-fire on every streamed event).

Adding a new captured event type (e.g. you start consuming `assistant/thinking` blocks):

1. Add the enum variant in `parser.rs` (or a new `HookEvent` field).
2. Add a match arm in `apply_events` / `apply_hook` that calls `agent.record(LogEntry { ... }, &mut log_out)` — pick a stable `kind` string (e.g. `"AssistantThinking"`).
3. No frontend changes needed: the existing `<For>` renders any kind. If you want kind-specific styling, add a CSS variant `.event-row--<kind>` keyed off `e.source.css_class()` or extend with a new class on the row.

## Working on the snapshot store

The History tab (issue #3) is fed by per-edit file snapshots captured via real-time hooks:

```
PreToolUse hook   ─►  snapshots::capture_pre_edit   ─►  blob + DB row (phase=pre)
PostToolUse hook  ─►  snapshots::capture_post_edit  ─►  blob + DB row (phase=post, paired_id=<pre id>)
```

- **Single writer to the snapshots dir**: `src-tauri/src/snapshots.rs`. Same single-writer rule that already applies to `compute_status` (agents.rs), `record()` (agents.rs), and `estimate_cost` (pricing.rs).
- **On-disk layout**: `<data_local>/claude-monitor/snapshots/<sanitized-session-id>/<row_id>.bin`. One blob per row; metadata in `file_snapshots` SQLite table indexed by `(session_id, ts DESC)` and `(tool_use_id)`.
- **Tracked tools**: only `Edit` / `Write` / `MultiEdit` / `NotebookEdit`. Bash file writes (`sed -i`, `>`, `tee`) are intentionally out of scope for v1 — extend `snapshots::TRACKED_TOOLS` if you change this.
- **Hooks-required**: JSONL fires *after* tool execution and can't capture the pre-state, so the History tab shows a banner telling the user to enable hooks if `prefs.hooks_enabled = false`. JSONL alone is not a fallback path here.
- **1 MB cap per file**: oversized files store an `oversized=true` row with a zero-byte blob; UI renders "snapshot skipped (file > 1 MB)". Don't try to bypass this — disk growth on large generated files is the v1 concern.
- **Restore is reversible**: `snapshots::restore` first captures a `pre-restore` snapshot of the file's current bytes (paired_id pointing at the snapshot being restored), then atomic-writes the blob via the same symlink-safe pattern as `settings_writer.rs`. Refuses symlink targets and verifies the blob's SHA-256 before writing.
- **Special case**: a `Write` against a path that didn't exist yet stores `tool_name='Write:create'` with a zero-byte pre blob. Restore of that pre row *deletes* the file rather than writing empty content.
- **Event log integration**: `snapshots::emit_snapshot_event` calls `AgentRegistry::record_external(...)` so `Snapshot:PreEdit` / `Snapshot:PostEdit` / `Snapshot:Restored` entries flow through the same `record()` capture point as everything else and show up in the per-agent detail pane stream.
- **Pruning**: `snapshots::prune_older_than(db, retention_days)` runs once on app startup. Defaults to 14 days (configurable in Settings). No timer-based pruning.
- **Hook handler latency**: capture reads the file synchronously inside the axum handler so we observe the pre-write bytes. Sub-ms for local files; on a network mount this delays Claude's tool execution by the read time. Documented limitation — see issue #3 plan risks.

Adding a new file-mutating tool (e.g. you want to capture a future `RewriteFile` tool):

1. Append the tool name to `snapshots::TRACKED_TOOLS`.
2. Teach `snapshots::resolve_target_path` to extract the file path from that tool's `tool_input` shape.
3. (Optional) Add a special-case in `snapshots::capture` if the tool's create/delete semantics differ from `Edit`/`Write`.

## Working on currency

- **Adding a currency**: extend `currency::SUPPORTED` (the curated 10-currency list) with `(code, symbol)`. Frankfurter exposes ~30 currencies; we filter to this list to keep the dropdown manageable. The frontend `format_money` uses the symbol; if the symbol is ambiguous (e.g. multiple `$` currencies) consider a code-only display instead.
- **Replacing the FX provider**: edit `currency::fetch_rates`. Keep the return type (`CurrencyCache { rates, fetched_at, source }`) so prefs.json stays compatible. Update CSP `connect-src` for the new host.
- **Cache TTL** is 24h via `currency::is_stale`. Background refresh fires once on app startup if stale; the user can also force-refresh from Settings. Don't poll on a timer — Frankfurter is rate-limited and the rates only change daily.
- All cost displays must route through `format_money(usd, &state)`. USD stays the source of truth in storage and `AgentSnapshot.cost_usd` — conversion happens at display time only, so SQLite history stays consistent across rate changes.

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
- `apply_hook`'s field names follow the Claude Code docs (`tool_use_id`, `agent_id`, etc.). If real payloads differ, parse errors log to size only (never contents) — search for `"hook payload parse error"` in the terminal output.
- Historical SQLite rows written before the cache-split migration only have a single `cache_tokens` figure; new rows populate all four cache columns. The Usage tab is OK with this because it sums for totals, but per-bucket charts on old data will show zero.
- The `agent_events` table grows unboundedly — there's no automatic pruning. For active users this is fine (a few KB per session), but if you ship a long-running install consider adding a `DELETE WHERE ts < datetime('now', '-30 days')` sweep on startup.
- File permissions hardening (0600 on db/prefs/.bak) is Unix-only. Windows relies on the per-user `%LocalAppData%` ACLs that already restrict cross-user reads.
- Currency rates fall back to 1.0 if the Frankfurter cache is empty AND the startup fetch failed — costs will appear in USD even if a different currency is selected. Force refresh from Settings once you're online.

## Roadmap (where we're heading)

See README. Short list: pin the hook port, per-project rollup, native rate-limit alerts, sprite skins, CSV export.
