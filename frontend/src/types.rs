use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AgentStatus {
    Idle,
    Working,
    Waiting,
    Error,
}

impl AgentStatus {
    pub fn css_class(&self) -> &'static str {
        match self {
            AgentStatus::Idle => "idle",
            AgentStatus::Working => "working",
            AgentStatus::Waiting => "waiting",
            AgentStatus::Error => "error",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            AgentStatus::Idle => "Ended",
            AgentStatus::Working => "Working",
            AgentStatus::Waiting => "Waiting for response",
            AgentStatus::Error => "Error",
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, AgentStatus::Working | AgentStatus::Waiting | AgentStatus::Error)
    }
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
    pub last_activity: String,
    pub started_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSettings {
    pub idle_timeout_secs: u64,
    pub permission_timeout_secs: u64,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayStats {
    pub date: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailySummary {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub session_count: u64,
    pub top_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageResponse {
    pub period_start: String,
    pub period_end: String,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub by_model: Vec<ModelUsage>,
}

pub fn project_label(path: &str) -> String {
    if path.is_empty() {
        return "(unknown project)".into();
    }
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

pub fn short_id(id: &str) -> String {
    let n = id.len().min(8);
    id[..n].to_string()
}
