use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::time;

use crate::parser::ClaudeEvent;

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
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            idle_timeout_secs: 180,
            permission_timeout_secs: 7,
            text_idle_secs: 5,
            message_preview_chars: 280,
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
}

pub struct AgentRegistry {
    agents: RwLock<HashMap<String, AgentInner>>,
    settings: RwLock<AgentSettings>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
            settings: RwLock::new(AgentSettings::default()),
        }
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

    /// Apply parsed events from a single session, return the new snapshot.
    /// `at` is the timestamp to attribute the activity to — pass file mtime
    /// for an initial scan of an existing JSONL file (so old sessions don't
    /// look fresh), pass `Utc::now()` for live updates.
    pub fn apply_events(
        &self,
        session_id: &str,
        events: &[ClaudeEvent],
        at: DateTime<Utc>,
        parent_id: Option<String>,
    ) -> Option<AgentSnapshot> {
        if events.is_empty() {
            return None;
        }

        let settings = self.settings.read().clone();
        let preview_chars = settings.message_preview_chars;
        let text_idle = chrono::Duration::seconds(settings.text_idle_secs as i64);

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
        });

        for ev in events {
            match ev {
                ClaudeEvent::SessionStart { project, .. } => {
                    if !project.is_empty() {
                        agent.snapshot.project = project.clone();
                    }
                }
                ClaudeEvent::Usage { input, output, cache, model } => {
                    agent.snapshot.input_tokens += input;
                    agent.snapshot.output_tokens += output;
                    agent.snapshot.cache_tokens += cache;
                    if !model.is_empty() {
                        agent.snapshot.model = model.clone();
                    }
                    agent.snapshot.cost_usd += estimate_cost(model, *input, *output, *cache);
                }
                ClaudeEvent::AssistantText { text } => {
                    let trimmed: String = text.chars().take(preview_chars).collect();
                    agent.snapshot.current_message = trimmed;
                    agent.awaiting_user = false;
                    // Arm the text-idle timer only on tool-free turns.
                    // If a tool was used this turn, the turn isn't over yet —
                    // we wait for tool_results / TurnEnd instead.
                    if !agent.had_tool_in_turn {
                        agent.text_idle_deadline = Some(at + text_idle);
                    }
                }
                ClaudeEvent::ToolUseStart { tool, id } => {
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
                    agent.snapshot.current_tool = Some(tool.clone());
                }
                ClaudeEvent::ToolUseEnd { id } => {
                    agent.pending_tools.remove(id);
                    if agent.pending_tools.is_empty() {
                        agent.snapshot.current_tool = None;
                    } else if let Some(t) = agent.pending_tools.values().next() {
                        agent.snapshot.current_tool = Some(t.name.clone());
                    }
                    // Tool finished but turn isn't necessarily over —
                    // assistant may emit more text or call more tools.
                    // Don't flip state here; wait for TurnEnd or text-idle.
                }
                ClaudeEvent::TurnEnd => {
                    // Definitive end-of-turn marker from Claude Code.
                    agent.had_tool_in_turn = false;
                    agent.text_idle_deadline = None;
                    agent.awaiting_user = true;
                }
                ClaudeEvent::UserMessage => {
                    // User just typed — new turn starting, agent is working.
                    agent.had_tool_in_turn = false;
                    agent.text_idle_deadline = None;
                    agent.awaiting_user = false;
                }
                ClaudeEvent::Unknown => {}
            }
        }

        agent.snapshot.last_activity = at;
        agent.snapshot.status = compute_status(agent, &settings, Utc::now());

        Some(agent.snapshot.clone())
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
    if let Some(deadline) = agent.text_idle_deadline {
        if now >= deadline && !agent.had_tool_in_turn {
            return AgentStatus::Waiting;
        }
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

/// Estimate cost based on Anthropic pricing (per million tokens).
pub fn estimate_cost(model: &str, input: u64, output: u64, cache: u64) -> f64 {
    let model_lc = model.to_lowercase();
    let (input_price, output_price, cache_price) = if model_lc.contains("opus") {
        (15.0, 75.0, 1.875)
    } else if model_lc.contains("sonnet") {
        (3.0, 15.0, 0.375)
    } else {
        (0.80, 4.0, 0.10)
    };
    let m = 1_000_000.0;
    (input as f64 / m) * input_price
        + (output as f64 / m) * output_price
        + (cache as f64 / m) * cache_price
}
