use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::time;

use crate::parser::ClaudeEvent;
use crate::pricing::{self, PricingTable};

/// Ring buffer cap per agent for the in-memory event log. ~500 entries at
/// ~300 bytes each ≈ 150 KB per agent; 50 active agents ≈ 7.5 MB worst case.
const EVENT_RING_CAP: usize = 500;
/// Maximum chars retained for an event summary line.
const SUMMARY_CHAR_CAP: usize = 500;
/// Maximum chars retained for an event details body.
const DETAILS_CHAR_CAP: usize = 4096;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogSource {
    Jsonl,
    Hook,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub session_id: String,
    pub timestamp: DateTime<Utc>,
    pub source: LogSource,
    /// e.g. "AssistantText", "ToolUseStart", "Hook:PreToolUse".
    pub kind: String,
    /// Single-line display string, truncated at SUMMARY_CHAR_CAP chars.
    pub summary: String,
    /// Optional longer body, truncated at DETAILS_CHAR_CAP chars.
    pub details: Option<String>,
    /// Tool-specific input arg captured from the hook payload. Currently
    /// populated only for `Hook:PreToolUse` rows where `tool_name == "Bash"`
    /// — stores the raw `command` string (truncated). Powers the Usage tab's
    /// Shell-command breakdown. NULL for everything else.
    #[serde(default)]
    pub tool_params: Option<String>,
}

fn truncate_chars(s: &str, cap: usize) -> String {
    let mut count = 0;
    let mut end = s.len();
    for (i, _) in s.char_indices() {
        if count == cap {
            end = i;
            break;
        }
        count += 1;
    }
    if end < s.len() {
        format!("{}…", &s[..end])
    } else {
        s.to_string()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AgentStatus {
    /// Session ended or has been quiet for `idle_timeout_secs` — historical.
    Idle,
    /// Recent activity, no stale tool.
    Working,
    /// Recent activity AND a tool_use has been pending past
    /// `permission_timeout_secs` without a matching tool_result. Idle takes
    /// priority — once the session goes idle, this drops back to Idle.
    Waiting,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSnapshot {
    pub session_id: String,
    pub project: String,
    pub status: AgentStatus,
    pub current_message: String,
    pub current_tool: Option<String>,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_5m_tokens: u64,
    pub cache_write_1h_tokens: u64,
    pub cache_read_tokens: u64,
    /// Legacy total = sum of the three cache columns. Kept for any consumer
    /// that still wants a single "cache" number (e.g. older UI cells).
    pub cache_tokens: u64,
    pub cost_usd: f64,
    pub last_activity: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    /// For sub-agents (Task tool spawns): the parent session UUID. None for
    /// root sessions.
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSettings {
    /// After this many seconds of no events at all, an agent flips to Idle
    /// (treated as session ended → moved to History).
    pub idle_timeout_secs: u64,
    /// After a tool_use is started but no tool_result arrives within this many
    /// seconds, treat the agent as Waiting (Claude Code is likely blocked on
    /// a permission prompt).
    pub permission_timeout_secs: u64,
    /// On a text-only turn (no tool_use), wait this many seconds after the
    /// last assistant text before flipping to Waiting. Mirrors pixel-agents'
    /// `TEXT_IDLE_DELAY_MS = 5000`.
    pub text_idle_secs: u64,
    /// Max characters of current_message to keep in memory + send to UI.
    pub message_preview_chars: usize,
    /// When a hook event has been received within this window, hooks are
    /// considered authoritative and JSONL-driven heuristics are suppressed
    /// for that agent. Outside this window, JSONL heuristics take over.
    pub hook_grace_secs: u64,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            idle_timeout_secs: 180,
            permission_timeout_secs: 7,
            text_idle_secs: 5,
            message_preview_chars: 280,
            hook_grace_secs: 30,
        }
    }
}

#[derive(Debug)]
struct PendingTool {
    #[allow(dead_code)]
    name: String,
    started_at: DateTime<Utc>,
    flagged_permission: bool,
}

#[derive(Debug)]
struct AgentInner {
    snapshot: AgentSnapshot,
    pending_tools: HashMap<String, PendingTool>,
    /// Did the current turn contain any tool_use? Reset on TurnEnd /
    /// UserMessage. Used by the text-idle timer (only arms when false).
    had_tool_in_turn: bool,
    /// When set, the agent will flip to Waiting at this instant. Only set
    /// after an assistant text block on a tool-free turn. Cleared on
    /// ToolUseStart, TurnEnd, UserMessage.
    text_idle_deadline: Option<DateTime<Utc>>,
    /// Set on TurnEnd (Claude finished a turn → waiting on user's reply).
    /// Cleared on UserMessage / AssistantText / ToolUseStart.
    awaiting_user: bool,
    /// Last time a Claude Code hook event updated this agent. When recent,
    /// hooks are authoritative.
    last_hook_at: Option<DateTime<Utc>>,
    /// Bounded ring buffer of narrative events for the live detail-pane log.
    /// Newest at the back. Cap is `EVENT_RING_CAP`.
    events: VecDeque<LogEntry>,
}

impl AgentInner {
    /// Push a log entry to the ring (evicting head when full) AND append it
    /// to `out` so the caller can fan out the same entry to subscribers
    /// (Tauri emit, SQLite persistence, etc.). Single source of truth for
    /// event capture.
    fn record(&mut self, entry: LogEntry, out: &mut Vec<LogEntry>) {
        if self.events.len() >= EVENT_RING_CAP {
            self.events.pop_front();
        }
        self.events.push_back(entry.clone());
        out.push(entry);
    }
}

pub struct AgentRegistry {
    agents: RwLock<HashMap<String, AgentInner>>,
    settings: RwLock<AgentSettings>,
    pricing: RwLock<PricingTable>,
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
            settings: RwLock::new(AgentSettings::default()),
            pricing: RwLock::new(pricing::default_pricing_table()),
        }
    }

    pub fn pricing(&self) -> PricingTable {
        self.pricing.read().clone()
    }

    pub fn update_pricing(&self, table: PricingTable) {
        *self.pricing.write() = table;
    }

    pub fn snapshot_all(&self) -> Vec<AgentSnapshot> {
        self.agents
            .read()
            .values()
            .map(|a| a.snapshot.clone())
            .collect()
    }

    pub fn snapshot_one(&self, session_id: &str) -> Option<AgentSnapshot> {
        self.agents.read().get(session_id).map(|a| a.snapshot.clone())
    }

    pub fn settings(&self) -> AgentSettings {
        self.settings.read().clone()
    }

    pub fn update_settings(&self, new_settings: AgentSettings) {
        *self.settings.write() = new_settings;
    }

    /// Push a log entry into a session's ring buffer from outside the
    /// `apply_events` / `apply_hook` paths (e.g. the snapshot subsystem
    /// recording `Snapshot:PreEdit` / `Snapshot:PostEdit` / `Snapshot:Restored`
    /// events). Routes through the same `record()` capture point so the ring
    /// cap and frontend stream behave identically. No-op if the session isn't
    /// tracked yet.
    pub fn record_external(&self, session_id: &str, entry: LogEntry) {
        let mut agents = self.agents.write();
        if let Some(agent) = agents.get_mut(session_id) {
            let mut sink = Vec::with_capacity(1);
            agent.record(entry, &mut sink);
        }
    }

    /// Most-recent N entries from a session's ring buffer (oldest first,
    /// newest last). Returns empty if the agent isn't tracked.
    pub fn events_for(&self, session_id: &str, limit: usize) -> Vec<LogEntry> {
        self.agents
            .read()
            .get(session_id)
            .map(|a| {
                let len = a.events.len();
                let start = len.saturating_sub(limit);
                a.events.iter().skip(start).cloned().collect()
            })
            .unwrap_or_default()
    }

    /// Apply parsed events from a single session, return the new snapshot
    /// plus any narrative log entries captured during processing.
    /// `at` is the timestamp to attribute the activity to — pass file mtime
    /// for an initial scan of an existing JSONL file (so old sessions don't
    /// look fresh), pass `Utc::now()` for live updates.
    pub fn apply_events(
        &self,
        session_id: &str,
        events: &[ClaudeEvent],
        at: DateTime<Utc>,
        parent_id: Option<String>,
    ) -> (Option<AgentSnapshot>, Vec<LogEntry>) {
        let mut log_out: Vec<LogEntry> = Vec::new();
        if events.is_empty() {
            return (None, log_out);
        }

        let settings = self.settings.read().clone();
        let preview_chars = settings.message_preview_chars;
        let text_idle = chrono::Duration::seconds(settings.text_idle_secs as i64);

        let pricing_table = self.pricing.read().clone();

        let mut agents = self.agents.write();
        let agent = agents.entry(session_id.to_string()).or_insert_with(|| AgentInner {
            snapshot: AgentSnapshot {
                session_id: session_id.to_string(),
                project: String::new(),
                status: AgentStatus::Working,
                current_message: String::new(),
                current_tool: None,
                model: String::new(),
                input_tokens: 0,
                output_tokens: 0,
                cache_write_5m_tokens: 0,
                cache_write_1h_tokens: 0,
                cache_read_tokens: 0,
                cache_tokens: 0,
                cost_usd: 0.0,
                last_activity: at,
                started_at: at,
                parent_id: parent_id.clone(),
            },
            pending_tools: HashMap::new(),
            had_tool_in_turn: false,
            text_idle_deadline: None,
            awaiting_user: false,
            last_hook_at: None,
            events: VecDeque::new(),
        });

        // Hook authority: if a hook event has fired for this agent within
        // the grace window, treat hooks as ground truth and skip JSONL
        // writes to status-driving fields. We still update tokens / cost /
        // preview / model / project — those are useful regardless of which
        // signal source is "owning" the status.
        let now = Utc::now();
        let hooks_authoritative = match agent.last_hook_at {
            Some(t) => {
                let elapsed = now.signed_duration_since(t).num_seconds().max(0) as u64;
                elapsed < settings.hook_grace_secs
            }
            None => false,
        };

        for ev in events {
            match ev {
                ClaudeEvent::SessionStart { project, .. } => {
                    if !project.is_empty() {
                        agent.snapshot.project = project.clone();
                    }
                    // Skipped from log — `cwd` appears on every JSONL record so
                    // SessionStart fires per-line; logging it would drown the
                    // narrative stream.
                }
                ClaudeEvent::Usage { usage, model } => {
                    agent.snapshot.input_tokens += usage.input;
                    agent.snapshot.output_tokens += usage.output;
                    agent.snapshot.cache_write_5m_tokens += usage.cache_write_5m;
                    agent.snapshot.cache_write_1h_tokens += usage.cache_write_1h;
                    agent.snapshot.cache_read_tokens += usage.cache_read;
                    agent.snapshot.cache_tokens = agent.snapshot.cache_write_5m_tokens
                        + agent.snapshot.cache_write_1h_tokens
                        + agent.snapshot.cache_read_tokens;
                    if !model.is_empty() {
                        agent.snapshot.model = model.clone();
                    }
                    let model_for_cost = if !model.is_empty() {
                        model.as_str()
                    } else {
                        agent.snapshot.model.as_str()
                    };
                    let p = pricing_table.pricing_for(model_for_cost);
                    agent.snapshot.cost_usd += pricing::estimate_cost(usage, &p);
                    // Skipped from log — Usage is an aggregate counter, not narrative.
                }
                ClaudeEvent::AssistantText { text } => {
                    let trimmed: String = text.chars().take(preview_chars).collect();
                    agent.snapshot.current_message = trimmed;
                    if !hooks_authoritative {
                        agent.awaiting_user = false;
                        // Arm the text-idle timer only on tool-free turns.
                        if !agent.had_tool_in_turn {
                            agent.text_idle_deadline = Some(at + text_idle);
                        }
                    }
                    let summary = truncate_chars(text, SUMMARY_CHAR_CAP);
                    let details = if text.chars().count() > SUMMARY_CHAR_CAP {
                        Some(truncate_chars(text, DETAILS_CHAR_CAP))
                    } else {
                        None
                    };
                    agent.record(
                        LogEntry {
                            session_id: session_id.to_string(),
                            timestamp: at,
                            source: LogSource::Jsonl,
                            kind: "AssistantText".to_string(),
                            summary,
                            details,
                            tool_params: None,
                        },
                        &mut log_out,
                    );
                }
                ClaudeEvent::ToolUseStart { tool, id } => {
                    if !hooks_authoritative {
                        agent.had_tool_in_turn = true;
                        agent.text_idle_deadline = None;
                        agent.awaiting_user = false;
                        agent.pending_tools.insert(
                            id.clone(),
                            PendingTool {
                                name: tool.clone(),
                                started_at: at,
                                flagged_permission: false,
                            },
                        );
                    }
                    agent.snapshot.current_tool = Some(tool.clone());
                    agent.record(
                        LogEntry {
                            session_id: session_id.to_string(),
                            timestamp: at,
                            source: LogSource::Jsonl,
                            kind: "ToolUseStart".to_string(),
                            summary: truncate_chars(tool, SUMMARY_CHAR_CAP),
                            details: Some(id.clone()),
                            tool_params: None,
                        },
                        &mut log_out,
                    );
                }
                ClaudeEvent::ToolUseEnd { id } => {
                    // Resolve tool name BEFORE removing from pending_tools.
                    let tool_name = agent
                        .pending_tools
                        .get(id)
                        .map(|t| t.name.clone())
                        .unwrap_or_default();
                    if !hooks_authoritative {
                        agent.pending_tools.remove(id);
                        if agent.pending_tools.is_empty() {
                            agent.snapshot.current_tool = None;
                        } else if let Some(t) = agent.pending_tools.values().next() {
                            agent.snapshot.current_tool = Some(t.name.clone());
                        }
                    }
                    agent.record(
                        LogEntry {
                            session_id: session_id.to_string(),
                            timestamp: at,
                            source: LogSource::Jsonl,
                            kind: "ToolUseEnd".to_string(),
                            summary: if tool_name.is_empty() {
                                "tool finished".to_string()
                            } else {
                                tool_name
                            },
                            details: Some(id.clone()),
                            tool_params: None,
                        },
                        &mut log_out,
                    );
                }
                ClaudeEvent::TurnEnd => {
                    if !hooks_authoritative {
                        agent.had_tool_in_turn = false;
                        agent.text_idle_deadline = None;
                        agent.awaiting_user = true;
                    }
                    agent.record(
                        LogEntry {
                            session_id: session_id.to_string(),
                            timestamp: at,
                            source: LogSource::Jsonl,
                            kind: "TurnEnd".to_string(),
                            summary: "turn ended".to_string(),
                            details: None,
                            tool_params: None,
                        },
                        &mut log_out,
                    );
                }
                ClaudeEvent::UserMessage => {
                    if !hooks_authoritative {
                        agent.had_tool_in_turn = false;
                        agent.text_idle_deadline = None;
                        agent.awaiting_user = false;
                    }
                    agent.record(
                        LogEntry {
                            session_id: session_id.to_string(),
                            timestamp: at,
                            source: LogSource::Jsonl,
                            kind: "UserMessage".to_string(),
                            summary: "user message".to_string(),
                            details: None,
                            tool_params: None,
                        },
                        &mut log_out,
                    );
                }
                ClaudeEvent::Unknown => {}
            }
        }

        agent.snapshot.last_activity = at;
        agent.snapshot.status = compute_status(agent, &settings, now);

        (Some(agent.snapshot.clone()), log_out)
    }

    /// Apply a Claude Code hook event. Returns the new snapshot plus any
    /// log entries emitted. Hooks are authoritative — they set state directly
    /// and bump `last_hook_at`, which suppresses JSONL inference for
    /// `hook_grace_secs`.
    pub fn apply_hook(&self, ev: &HookEvent) -> (Option<AgentSnapshot>, Vec<LogEntry>) {
        let mut log_out: Vec<LogEntry> = Vec::new();
        let settings = self.settings.read().clone();
        let now = Utc::now();
        let preview_chars = settings.message_preview_chars;

        // Pick the right session_id: SubagentStart events identify a child
        // agent via `agent_id`; everything else uses the parent `session_id`.
        let (target_id, parent_id) = match ev.hook_event_name.as_str() {
            "SubagentStart" | "SubagentStop" => {
                let aid = ev.agent_id.clone().unwrap_or_else(|| ev.session_id.clone());
                (aid, Some(ev.session_id.clone()))
            }
            _ => (ev.session_id.clone(), None),
        };

        let mut agents = self.agents.write();
        let agent = agents.entry(target_id.clone()).or_insert_with(|| AgentInner {
            snapshot: AgentSnapshot {
                session_id: target_id.clone(),
                project: ev.cwd.clone().unwrap_or_default(),
                status: AgentStatus::Working,
                current_message: String::new(),
                current_tool: None,
                model: String::new(),
                input_tokens: 0,
                output_tokens: 0,
                cache_write_5m_tokens: 0,
                cache_write_1h_tokens: 0,
                cache_read_tokens: 0,
                cache_tokens: 0,
                cost_usd: 0.0,
                last_activity: now,
                started_at: now,
                parent_id: parent_id.clone(),
            },
            pending_tools: HashMap::new(),
            had_tool_in_turn: false,
            text_idle_deadline: None,
            awaiting_user: false,
            last_hook_at: Some(now),
            events: VecDeque::new(),
        });

        // Update project lazily if we hadn't seen it yet.
        if agent.snapshot.project.is_empty()
            && let Some(cwd) = &ev.cwd
        {
            agent.snapshot.project = cwd.clone();
        }
        if agent.snapshot.parent_id.is_none() {
            agent.snapshot.parent_id = parent_id;
        }

        match ev.hook_event_name.as_str() {
            "SessionStart" | "UserPromptSubmit" => {
                // Either the session just started OR the user just submitted
                // a new prompt. Either way: a new turn is beginning, agent
                // is Working again. UserPromptSubmit is the critical signal
                // that bridges Stop → next-turn-Working when the response
                // doesn't include any tool calls (text-only responses).
                agent.had_tool_in_turn = false;
                agent.text_idle_deadline = None;
                agent.awaiting_user = false;
            }
            "PreToolUse" => {
                agent.had_tool_in_turn = true;
                agent.text_idle_deadline = None;
                agent.awaiting_user = false;
                let id = ev
                    .tool_use_id
                    .clone()
                    .unwrap_or_else(|| format!("hook-{}", now.timestamp_nanos_opt().unwrap_or(0)));
                agent.pending_tools.insert(
                    id,
                    PendingTool {
                        name: ev.tool_name.clone().unwrap_or_default(),
                        started_at: now,
                        flagged_permission: false,
                    },
                );
                if let Some(t) = &ev.tool_name {
                    agent.snapshot.current_tool = Some(t.clone());
                }
            }
            "PostToolUse" | "PostToolUseFailure" => {
                if let Some(id) = &ev.tool_use_id {
                    agent.pending_tools.remove(id);
                }
                if agent.pending_tools.is_empty() {
                    agent.snapshot.current_tool = None;
                }
            }
            "Stop" => {
                agent.had_tool_in_turn = false;
                agent.text_idle_deadline = None;
                agent.awaiting_user = true;
            }
            "Notification" => {
                if matches!(
                    ev.notification_type.as_deref(),
                    Some("permission_prompt") | Some("idle_prompt")
                ) {
                    agent.awaiting_user = true;
                }
            }
            "PermissionRequest" => {
                agent.awaiting_user = true;
            }
            "SubagentStart" => {
                agent.had_tool_in_turn = false;
                agent.text_idle_deadline = None;
                agent.awaiting_user = false;
            }
            "SubagentStop" => {
                // Let the idle timeout naturally pull the sub to Idle.
            }
            "SessionEnd" => {
                // Force immediate Idle by backdating last_activity.
                agent.snapshot.last_activity = now
                    - chrono::Duration::seconds(settings.idle_timeout_secs as i64 + 1);
                agent.last_hook_at = Some(now);
                agent.snapshot.status = compute_status(agent, &settings, now);
                eprintln!(
                    "[claude-monitor] hook SessionEnd session={} → {:?}",
                    target_id, agent.snapshot.status
                );
                agent.record(
                    LogEntry {
                        session_id: target_id.clone(),
                        timestamp: now,
                        source: LogSource::Hook,
                        kind: "Hook:SessionEnd".to_string(),
                        summary: "session ended".to_string(),
                        details: None,
                        tool_params: None,
                    },
                    &mut log_out,
                );
                return (Some(agent.snapshot.clone()), log_out);
            }
            _ => {}
        }

        // Capture every hook as a narrative log entry. Summary picks the
        // most useful field available — tool name, notification subtype, or
        // a hint of the last assistant text.
        let summary = ev
            .tool_name
            .clone()
            .or_else(|| ev.notification_type.clone())
            .or_else(|| {
                ev.last_assistant_text
                    .as_ref()
                    .map(|t| truncate_chars(t, 200))
            })
            .unwrap_or_else(|| ev.hook_event_name.clone());
        let details = ev
            .last_assistant_text
            .as_ref()
            .filter(|t| !t.is_empty())
            .map(|t| truncate_chars(t, DETAILS_CHAR_CAP))
            .or_else(|| ev.tool_use_id.clone());
        // For Bash PreToolUse hooks, persist the raw `command` arg so the
        // Usage tab's Shell-command breakdown can bucket invocations.
        // Truncated to 2 KB — plenty for any normal CLI invocation, bounded
        // for `agent_events` storage.
        let tool_params = if ev.hook_event_name == "PreToolUse"
            && ev.tool_name.as_deref() == Some("Bash")
        {
            ev.tool_input
                .as_ref()
                .and_then(|v| v.get("command"))
                .and_then(|v| v.as_str())
                .map(|s| truncate_chars(s, 2048))
        } else {
            None
        };
        agent.record(
            LogEntry {
                session_id: target_id.clone(),
                timestamp: now,
                source: LogSource::Hook,
                kind: format!("Hook:{}", ev.hook_event_name),
                summary: truncate_chars(&summary, SUMMARY_CHAR_CAP),
                details,
                tool_params,
            },
            &mut log_out,
        );

        // Optional: capture latest assistant text from PostToolUse / Stop
        // payloads if Claude Code includes them. Best-effort.
        if let Some(text) = &ev.last_assistant_text {
            let trimmed: String = text.chars().take(preview_chars).collect();
            if !trimmed.is_empty() {
                agent.snapshot.current_message = trimmed;
            }
        }

        agent.snapshot.last_activity = agent.snapshot.last_activity.max(now);
        agent.last_hook_at = Some(now);
        agent.snapshot.status = compute_status(agent, &settings, now);

        eprintln!(
            "[claude-monitor] hook {} session={} tool={:?} → {:?}",
            ev.hook_event_name,
            short_id(&target_id),
            ev.tool_name.as_deref().unwrap_or("-"),
            agent.snapshot.status
        );

        (Some(agent.snapshot.clone()), log_out)
    }

    /// Periodic tick: flip stale agents to Idle, flag long-pending tools as
    /// Waiting. Idle takes priority — once an agent has been quiet past the
    /// idle timeout it drops to Idle even if a tool was left pending.
    /// Returns (status_changes, newly_waiting_for_response).
    pub fn tick(&self) -> (Vec<AgentSnapshot>, Vec<AgentSnapshot>) {
        let settings = self.settings.read().clone();
        let now = Utc::now();
        let mut changes = Vec::new();
        let mut newly_waiting = Vec::new();

        let mut agents = self.agents.write();
        for agent in agents.values_mut() {
            let prev_status = agent.snapshot.status;
            let idle_secs = now
                .signed_duration_since(agent.snapshot.last_activity)
                .num_seconds()
                .max(0) as u64;
            let is_idle = idle_secs >= settings.idle_timeout_secs;

            // Flag long-pending tools as a permission stall — but only while
            // the session is still active. If we've gone idle, pending tools
            // are just stale (session ended without writing a tool_result).
            if !is_idle {
                for tool in agent.pending_tools.values_mut() {
                    if !tool.flagged_permission {
                        let elapsed = now
                            .signed_duration_since(tool.started_at)
                            .num_seconds()
                            .max(0) as u64;
                        if elapsed >= settings.permission_timeout_secs {
                            tool.flagged_permission = true;
                        }
                    }
                }
            }

            let new_status = compute_status(agent, &settings, now);
            agent.snapshot.status = new_status;

            if new_status != prev_status {
                changes.push(agent.snapshot.clone());
                // Emit a "newly waiting" alert only on the Working→Waiting
                // edge — never on Idle→Waiting (which can't happen now), and
                // never on repeated Waiting ticks.
                if new_status == AgentStatus::Waiting && prev_status == AgentStatus::Working {
                    newly_waiting.push(agent.snapshot.clone());
                }
            }
        }

        (changes, newly_waiting)
    }
}

/// Single source of truth for agent status. Used by both `apply_events`
/// (right after parsing new events) and `tick` (periodic re-evaluation).
fn compute_status(agent: &AgentInner, settings: &AgentSettings, now: DateTime<Utc>) -> AgentStatus {
    let idle_secs = now
        .signed_duration_since(agent.snapshot.last_activity)
        .num_seconds()
        .max(0) as u64;

    if idle_secs >= settings.idle_timeout_secs {
        return AgentStatus::Idle;
    }

    // Tool stuck pending past permission threshold → waiting on user approval.
    if agent.pending_tools.values().any(|t| t.flagged_permission) {
        return AgentStatus::Waiting;
    }

    // Explicit Waiting from a TurnEnd marker.
    if agent.awaiting_user {
        return AgentStatus::Waiting;
    }

    // Text-only turn that's been quiet for `text_idle_secs` → turn ended,
    // waiting on user's next message. (Backup signal in case turn_duration
    // wasn't emitted.)
    if let Some(deadline) = agent.text_idle_deadline
        && now >= deadline
        && !agent.had_tool_in_turn
    {
        return AgentStatus::Waiting;
    }

    AgentStatus::Working
}

/// Spawn a background task that ticks the registry every second and emits
/// `agent-status` events when an agent's status changes, plus
/// `agent-waiting` when a new agent transitions to Waiting (alert).
///
/// Uses `tauri::async_runtime::spawn` so this can be called from inside the
/// Tauri builder `.setup()` closure (where there's no ambient Tokio runtime
/// yet).
pub fn spawn_tick_loop(app: AppHandle, registry: Arc<AgentRegistry>) {
    tauri::async_runtime::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            let (changes, newly_waiting) = registry.tick();
            for snap in &changes {
                let _ = app.emit("agent-status", snap);
            }
            for snap in &newly_waiting {
                let _ = app.emit("agent-waiting", snap);
            }
        }
    });
}

/// Payload of a Claude Code hook event (POST body of an `"type":"http"`
/// hook). Only fields we care about are deserialized; unknown fields are
/// ignored. Field names match Claude Code's documented schema (snake_case).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HookEvent {
    pub hook_event_name: String,
    #[serde(default)]
    pub session_id: String,
    pub agent_id: Option<String>,
    pub cwd: Option<String>,
    pub tool_name: Option<String>,
    pub tool_use_id: Option<String>,
    /// Notification subtype: e.g. `permission_prompt`, `idle_prompt`.
    #[serde(rename = "type")]
    pub notification_type: Option<String>,
    /// Optional preview text some events may include.
    pub last_assistant_text: Option<String>,
    /// Raw `tool_input` payload — used by the snapshot subsystem to extract
    /// `file_path` / `notebook_path` for `Edit`/`Write`/`MultiEdit`/`NotebookEdit`.
    /// Other code paths ignore it. `Option<Value>` so unknown / missing
    /// payloads don't break parsing.
    pub tool_input: Option<serde_json::Value>,
    /// Raw `tool_response` payload (`PostToolUse`). Reserved for future use.
    pub tool_response: Option<serde_json::Value>,
}

fn short_id(id: &str) -> String {
    let n = id.len().min(8);
    id[..n].to_string()
}

