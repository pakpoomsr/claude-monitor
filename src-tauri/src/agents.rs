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
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Idle,
    Working,
    NeedsPermission,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSettings {
    /// After this many seconds of no events, an agent flips to Idle.
    pub idle_timeout_secs: u64,
    /// After a tool_use is started but no tool_result arrives within this many
    /// seconds, the agent flips to NeedsPermission (Claude Code is blocked
    /// waiting for the user to approve).
    pub permission_timeout_secs: u64,
    /// Max characters of current_message to keep in memory + send to UI.
    pub message_preview_chars: usize,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            idle_timeout_secs: 30,
            permission_timeout_secs: 4,
            message_preview_chars: 280,
        }
    }
}

#[derive(Debug)]
struct PendingTool {
    name: String,
    started_at: DateTime<Utc>,
    flagged_permission: bool,
}

#[derive(Debug)]
struct AgentInner {
    snapshot: AgentSnapshot,
    pending_tools: HashMap<String, PendingTool>,
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
    pub fn apply_events(
        &self,
        session_id: &str,
        events: &[ClaudeEvent],
    ) -> Option<AgentSnapshot> {
        if events.is_empty() {
            return None;
        }

        let preview_chars = self.settings.read().message_preview_chars;

        let mut agents = self.agents.write();
        let now = Utc::now();
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
                last_activity: now,
                started_at: now,
            },
            pending_tools: HashMap::new(),
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
                }
                ClaudeEvent::ToolUseStart { tool, id } => {
                    agent.pending_tools.insert(
                        id.clone(),
                        PendingTool {
                            name: tool.clone(),
                            started_at: now,
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
                }
                ClaudeEvent::UserMessage | ClaudeEvent::Unknown => {}
            }
        }

        agent.snapshot.last_activity = now;
        agent.snapshot.status = if agent.pending_tools.values().any(|t| t.flagged_permission) {
            AgentStatus::NeedsPermission
        } else {
            AgentStatus::Working
        };

        Some(agent.snapshot.clone())
    }

    /// Periodic tick: flip stale agents to Idle, flag long-pending tools as
    /// NeedsPermission. Returns (status_changes, newly_needing_permission).
    pub fn tick(&self) -> (Vec<AgentSnapshot>, Vec<AgentSnapshot>) {
        let settings = self.settings.read().clone();
        let now = Utc::now();
        let mut changes = Vec::new();
        let mut new_permission = Vec::new();

        let mut agents = self.agents.write();
        for agent in agents.values_mut() {
            let prev_status = agent.snapshot.status;

            let mut newly_flagged = false;
            for tool in agent.pending_tools.values_mut() {
                if !tool.flagged_permission {
                    let elapsed = now
                        .signed_duration_since(tool.started_at)
                        .num_seconds()
                        .max(0) as u64;
                    if elapsed >= settings.permission_timeout_secs {
                        tool.flagged_permission = true;
                        newly_flagged = true;
                    }
                }
            }

            if agent.pending_tools.values().any(|t| t.flagged_permission) {
                agent.snapshot.status = AgentStatus::NeedsPermission;
            } else {
                let idle_secs = now
                    .signed_duration_since(agent.snapshot.last_activity)
                    .num_seconds()
                    .max(0) as u64;
                agent.snapshot.status = if idle_secs >= settings.idle_timeout_secs {
                    AgentStatus::Idle
                } else {
                    AgentStatus::Working
                };
            }

            if agent.snapshot.status != prev_status {
                changes.push(agent.snapshot.clone());
                if agent.snapshot.status == AgentStatus::NeedsPermission && newly_flagged {
                    new_permission.push(agent.snapshot.clone());
                }
            }
        }

        (changes, new_permission)
    }
}

/// Spawn a background task that ticks the registry every second and emits
/// `agent-status` events when an agent's status changes, plus
/// `permission-needed` when a new agent needs approval.
pub fn spawn_tick_loop(app: AppHandle, registry: Arc<AgentRegistry>) {
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            let (changes, permission_needed) = registry.tick();
            for snap in &changes {
                let _ = app.emit("agent-status", snap);
            }
            for snap in &permission_needed {
                let _ = app.emit("permission-needed", snap);
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
