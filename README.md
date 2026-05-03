# Claude Monitor

> A modern desktop dashboard that watches your Claude Code agents in real time —
> like a task manager for your AI assistants.

**Pure Rust** — Tauri 2 backend + Leptos 0.8 (CSR) WASM frontend, Rust 2024 edition.
No JavaScript framework. Light + dark themes. ~10 MB binary.

---

## ⚡ Quick start

```bash
# 1. one-time toolchain setup
rustup target add wasm32-unknown-unknown
cargo install trunk
cargo install tauri-cli --version "^2"

# 2. clone + run
git clone https://github.com/pakpoomsr/claude-monitor
cd claude-monitor
cargo tauri dev
```

That's it. The first launch auto-registers Claude Code hooks (with a backup of
your `settings.json`) so the dashboard updates live the moment you start a
`claude` session in any terminal.

---

## 📦 Install

### Prerequisites

| Tool | Version | Why |
|---|---|---|
| **Rust** | 1.85+ (2024 edition) | toolchain — `rustup install stable` |
| **WASM target** | `wasm32-unknown-unknown` | frontend compiles to WASM |
| **Trunk** | latest | Leptos/WASM bundler |
| **Tauri CLI** | 2.x | desktop build orchestrator |

```bash
rustup install stable
rustup target add wasm32-unknown-unknown
cargo install trunk
cargo install tauri-cli --version "^2"
```

### Platform-specific system deps

<details>
<summary><b>Windows</b> — usually nothing extra</summary>

WebView2 is required (preinstalled on Windows 10/11). If your machine doesn't
have it: https://developer.microsoft.com/microsoft-edge/webview2/
</details>

<details>
<summary><b>macOS</b> — Xcode CLT</summary>

```bash
xcode-select --install
```
</details>

<details>
<summary><b>Linux</b> — webkit2gtk + friends</summary>

Debian/Ubuntu:
```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

Other distros: see https://tauri.app/start/prerequisites/
</details>

### Build a release bundle

```bash
cargo tauri build
```

Produces a native installer in `src-tauri/target/release/bundle/`:
- Windows → `.msi` / `.exe`
- macOS → `.dmg` / `.app`
- Linux → `.AppImage` / `.deb` / `.rpm`

---

## ▶️ Using Claude Monitor

### Run it

```bash
cargo tauri dev      # dev mode with hot-reload (frontend on :1420)
cargo tauri build    # release bundle for distribution
```

### First launch — what happens

1. The app opens to the **Agents** tab. Empty until you run `claude` somewhere.
2. An embedded HTTP server starts on `127.0.0.1:<random_port>` for hook events.
3. Your `~/.claude/settings.json` is **backed up to `settings.json.bak`** (one-time)
   and **11 hook entries are auto-registered**. Each is tagged `_claude_monitor: true`
   so they can be cleanly removed later.
4. Claude Code picks up the new hooks live — **no restart needed**.

> ✅ Dashboard updates within a second of every PreToolUse / Stop / Notification
> from any active `claude` session, anywhere on your machine.

### The four tabs

| Tab | What you see |
|---|---|
| 🟢 **Agents** | Live grid — one tile per Claude session, color-coded by status, with parent / sub-agent grouping. Click a tile for the detail pane (last message, in-flight tool, **token cost breakdown table**, **I/O + cache hit meters**). |
| 📊 **Usage** | Local SQLite history — today's tokens & cost, plus 7-day bar chart. |
| 🌐 **API** | Optional Anthropic billing-API view (paste a key — kept in memory only, never written to disk). |
| ⚙️ **Settings** | Toggle the real-time hooks on/off, tune state-machine thresholds. |

### Status legend

| | Status | What it means |
|---|---|---|
| 🟢 | **Working** | Assistant or tool currently in flight |
| 🟡 | **Waiting** | Turn ended — Claude wants your input (or stuck on a permission prompt) |
| ⚪ | **Idle** | No activity for `idle_timeout_secs` (treated as ended/historical) |
| 🔴 | **Error** | (reserved) |

### Themes

Click the **sun/moon icon** in the top-right to toggle between dark and light.
Choice is remembered via `localStorage`. If you've never picked one, it follows
your OS `prefers-color-scheme`.

### Disabling hooks

Settings → **Disable hooks**. The app removes its tagged entries from
`~/.claude/settings.json` and writes `hooks_enabled: false` to
`<data_local_dir>/claude-monitor/prefs.json`. Subsequent launches won't
re-register until you click **Set up hooks** again.

When hooks are off, the JSONL fallback (file-tailing `~/.claude/projects/**/*.jsonl`)
still keeps the dashboard working — just less precisely.

---

## 🌟 Features

- **Live agent grid** — one tile per Claude Code session, color-coded by status
- **Parent / sub-agent grouping** — Task-tool sub-agents nest under their parent
  with `↳ sub-agent <id>` rows; the group's headline status is the most-active
  member, so a parent "Waiting on its sub" is correctly counted as **Working**
- **Filter pills** — All / Active / Idle on the Agents tab
- **Detail pane** — last assistant message, in-flight tool, **I/O bar**,
  **cache-hit gauge**, per-token-type **cost table** (Input / Output / Cache
  with rate × tokens = cost rows that sum to the displayed total), project path
- **Real-time hooks** (auto-on) — PreToolUse / Stop / Notification / SubagentStart
  → embedded localhost HTTP server. Far more accurate than file-tailing
- **JSONL fallback** — when hooks aren't registered (or for sessions that started
  before they were), status is inferred from `~/.claude/projects/**/*.jsonl` with
  a state machine that includes the `system/turn_duration` end-of-turn marker
- **Local usage history** — SQLite-backed token and cost rollups, 7-day chart
- **Optional Anthropic billing API** — paste a key, kept in memory only
- **Tray icon + toast** — toast pops up when an agent flips to Waiting
- **Light + dark themes** — modern glassy design language, reduced-motion friendly

---

## ⚙️ Settings (defaults)

| Setting | Default | Meaning |
|---|---|---|
| `idle_timeout_secs` | 180 | Quiet for this long → Idle (history) |
| `permission_timeout_secs` | 7 | Tool pending this long → Waiting |
| `text_idle_secs` | 5 | Text-only turn quiet for this long → Waiting |
| `hook_grace_secs` | 30 | When to treat hooks as authoritative |
| `message_preview_chars` | 280 | Trim length for assistant message preview |

---

## 🧠 How status detection works

There are **two signal sources** that converge on a single state machine in `AgentRegistry`:

### 1. Real-time hooks (authoritative — auto-on)

Hooks register **automatically on every app launch** (the URL/port refreshes each
time since the server binds to a random port). On first launch, the app:

1. Backs up `~/.claude/settings.json` to `settings.json.bak` (only if no backup exists yet)
2. Adds 11 hook entries (one per event) tagged with `_claude_monitor: true` so
   they can be removed cleanly. Each is `"type": "http"` pointing at
   `http://127.0.0.1:<random>/h` with an `X-Auth: <random>` header.
3. Claude Code picks the changes up live — no restart.

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

Hook events bump `last_hook_at`. While that's recent (< `hook_grace_secs`,
default 30s), hooks are treated as ground truth.

### 2. JSONL fallback (always on)

When hooks aren't registered (or for sessions that started before they were),
status is inferred from `~/.claude/projects/<hash>/<session>.jsonl`:

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

Path-based: files at `<projects>/<proj>/<parent_uuid>/subagents/agent-<id>.jsonl`
are detected as sub-agents and registered with `parent_id = <parent_uuid>`. The
frontend groups them under their parent.

---

## 💰 Pricing assumptions (per million tokens)

| Model  | Input  | Output | Cache  |
|--------|--------|--------|--------|
| Opus   | $15.00 | $75.00 | $1.875 |
| Sonnet | $3.00  | $15.00 | $0.375 |
| Haiku  | $0.80  | $4.00  | $0.10  |

Edit `src-tauri/src/agents.rs::estimate_cost` to adjust.

---

## 🏗 Architecture

```
claude-monitor/
├── Cargo.toml                     # workspace root (edition 2024)
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
│       ├── prefs.rs               # persistent app prefs (hooks_enabled, etc.)
│       ├── db.rs                  # SQLite history (rusqlite, bundled)
│       └── api.rs                 # Anthropic billing API client
└── frontend/                      # Rust → WASM via Trunk
    ├── Cargo.toml
    ├── Trunk.toml
    ├── index.html
    ├── styles/main.css            # modern glassy theme (light + dark tokens)
    └── src/
        ├── main.rs                # Leptos app shell, tab routing, theme toggle
        ├── tauri_bridge.rs        # invoke / listen wrappers around window.__TAURI__
        ├── types.rs               # AgentStatus, AgentSnapshot, AgentGroup, Filter, HooksStatus
        └── components/
            ├── agent_grid.rs      # group rendering with nested sub-agents
            ├── agent_detail.rs    # selected-agent inspector + cost table + meters
            ├── usage_panel.rs     # local SQLite usage chart
            ├── api_usage_panel.rs # Anthropic billing-API view
            └── settings.rs        # hook setup + state machine thresholds
```

---

## 🔌 Tauri commands (frontend ↔ backend)

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

---

## ⚠️ Caveats

- The hook HTTP server binds to a **random ephemeral port** on each app launch.
  The auto-register on launch refreshes the URL in `settings.json` so this is
  invisible — no manual action needed unless you've explicitly disabled hooks.
- The `tauri.conf.json` setting `app.withGlobalTauri: true` is required so the
  WASM bridge can use `window.__TAURI__.event.listen` — don't remove it.
- `beforeDevCommand` must run from `frontend/`, hence the `cd frontend &&`
  prefix — Tauri runs the command from the project root by default.
- Hook entries are tagged `_claude_monitor: true`. If you edit
  `~/.claude/settings.json` manually, leave that key alone or unregister via
  the app first.

---

## 📄 License

MIT — see [LICENSE](LICENSE). Free for personal and commercial use, including
modification and redistribution; just keep the copyright notice.

---

## ❤️ Sponsor

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

---

## 🛣 Roadmap

- [ ] Pin hook server to a fixed port so registrations survive restarts
- [ ] Per-project rollup view
- [ ] Native rate-limit alerts via `tauri-plugin-notification`
- [ ] Export CSV
- [ ] Sprite skin picker
- [ ] Detect Claude Code subscription plan
