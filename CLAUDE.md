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
| `agents.rs` | **The heart.** `AgentRegistry`, `AgentSnapshot`, state machine (`apply_events`, `apply_hook`, `tick`, `compute_status`), `HookEvent`. Holds a live `PricingTable` used by the Usage event handler to compute cost. |
| `hooks.rs` | Axum HTTP server bound to `127.0.0.1:0` (random ephemeral port). Single `POST /h` endpoint with `X-Auth` header. Constant-time token compare via `subtle`; 64 KB body limit; payload contents never logged. |
| `settings_writer.rs` | Reads/registers/unregisters our hook entries in `~/.claude/settings.json`. Tag `_claude_monitor: true` on every entry we own; backup to `.bak` on first write; symlink-safe atomic writes via `OpenOptions::create_new(true)` + rename; 0600 perms on Unix. |
| `prefs.rs` | Persistent app prefs at `<data_local_dir>/claude-monitor/prefs.json`. Fields: `hooks_enabled`, `pricing_overrides` (HashMap keyed by `PricingEntry.id`), `pricing_currency` (ISO 4217), `currency_cache` (Frankfurter rates + fetched_at). |
| `watcher.rs` | `notify`-based JSONL watcher. Per-file byte offset for incremental reads. **8 MB per-line cap** (resyncs on next newline). Detects sub-agent paths (`<parent_uuid>/subagents/agent-X.jsonl`) and passes `parent_id` to `apply_events` only when the parent component is UUID-shaped. |
| `parser.rs` | Line → `Vec<ClaudeEvent>`. Handles `system/turn_duration`, content blocks (text/tool_use/tool_result), `usage` block (5 token fields including the nested `cache_creation.ephemeral_5m_input_tokens` / `_1h_input_tokens` buckets). |
| `pricing.rs` | `ModelPricing` (5 fields), `TokenUsage` (5 fields), `PricingTable` (Vec of `PricingEntry`), `default_pricing_table()` with all 13 SKUs, `merge_overrides`, `estimate_cost`. **Single source of truth** for cost math — never duplicate this logic in agents.rs. |
| `currency.rs` | Frankfurter HTTP client + curated 10-currency list (USD/EUR/GBP/JPY/CNY/THB/SGD/INR/KRW/AUD). `is_stale` returns true after 24h. |
| `db.rs` | SQLite (rusqlite, bundled) — Usage-tab history only. Schema migration adds `cache_write_5m_tokens`, `cache_write_1h_tokens`, `cache_read_tokens` if missing; legacy `cache_tokens` column kept as the sum. |
| `api.rs` | Anthropic billing API client (optional, key in memory only) |

### Frontend — `frontend/src/`

| File | Responsibility |
|---|---|
| `main.rs` | App shell, 5-tab routing, header indicators, signal wiring, polls `hooks_status` every 2s. **Provides global `RwSignal<PricingTable>` and `RwSignal<CurrencyState>` via `provide_context`** — leaf components consume via `use_context`. |
| `tauri_bridge.rs` | Thin `wasm-bindgen` wrappers around `window.__TAURI__.core.invoke` and `__TAURI__.event.listen`. Defensive — `is_tauri()` guards no-op when run outside the webview. |
| `types.rs` | All shared types: `AgentStatus`, `AgentSnapshot` (5 cache fields), `AgentSettings`, `AgentGroup`, `Filter`, `HooksStatus`, `ModelPricing`, `PricingEntry`, `PricingTable`, `CurrencyInfo`, `CurrencyState`. Helpers: `build_groups`, `apply_filter`, `format_money` (thousand-separator commas, currency-aware), `format_date_short` ("DD MMM YY"), `format_datetime` ("DD MMM YYYY HH:MM:SS"). |
| `components/agent_grid.rs` | Section renderer — parent tile + indented sub-agent tiles inside a `.group` card whose left edge color = aggregate status. Tile cost respects active currency. |
| `components/agent_detail.rs` | Side pane shown when a tile is selected. **5-row cost table** (Base Input / 5m Cache Write / 1h Cache Write / Cache Hit & Refresh / Output) reading live from the pricing context. |
| `components/usage_panel.rs` | SQLite-backed local usage view. Range pills (Last 7d / 30d / Custom) + two `<input type="date">` for custom range; bars show date and cost without hover. |
| `components/api_usage_panel.rs` | Anthropic billing API view |
| `components/settings.rs` | Real-time hooks toggle, state-machine thresholds, **editable pricing table** (13 rows × 5 cells, save-on-blur), **display currency** dropdown + manual refresh. |
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
- File permissions hardening (0600 on db/prefs/.bak) is Unix-only. Windows relies on the per-user `%LocalAppData%` ACLs that already restrict cross-user reads.
- Currency rates fall back to 1.0 if the Frankfurter cache is empty AND the startup fetch failed — costs will appear in USD even if a different currency is selected. Force refresh from Settings once you're online.

## Roadmap (where we're heading)

See README. Short list: pin the hook port, per-project rollup, native rate-limit alerts, sprite skins, CSV export.
