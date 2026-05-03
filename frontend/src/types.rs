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
            AgentStatus::Waiting => "Waiting",
            AgentStatus::Error => "Error",
        }
    }

    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        matches!(self, AgentStatus::Working | AgentStatus::Waiting | AgentStatus::Error)
    }

    /// Sort priority — smaller = more important.
    pub fn priority(&self) -> u8 {
        match self {
            AgentStatus::Waiting => 0,
            AgentStatus::Error => 1,
            AgentStatus::Working => 2,
            AgentStatus::Idle => 3,
        }
    }
}

/// A parent agent paired with its sub-agents (Task tool spawns).
#[derive(Debug, Clone)]
pub struct AgentGroup {
    pub parent: AgentSnapshot,
    pub children: Vec<AgentSnapshot>,
}

impl AgentGroup {
    /// Group "headline" status. Rule: any actively-working member makes the
    /// group Working — Claude Code may be using a sub-agent (Task tool) so
    /// the parent is technically "waiting on the sub", but real work is
    /// happening in the group. Only when nothing is working do we surface
    /// Waiting / Error / Idle.
    pub fn aggregate_status(&self) -> AgentStatus {
        let mut has_working = false;
        let mut has_error = false;
        let mut has_waiting = false;
        for s in std::iter::once(&self.parent.status)
            .chain(self.children.iter().map(|c| &c.status))
        {
            match s {
                AgentStatus::Working => has_working = true,
                AgentStatus::Error => has_error = true,
                AgentStatus::Waiting => has_waiting = true,
                AgentStatus::Idle => {}
            }
        }
        if has_working { AgentStatus::Working }
        else if has_error { AgentStatus::Error }
        else if has_waiting { AgentStatus::Waiting }
        else { AgentStatus::Idle }
    }

    pub fn last_activity(&self) -> &str {
        std::iter::once(&self.parent)
            .chain(self.children.iter())
            .map(|a| a.last_activity.as_str())
            .max()
            .unwrap_or("")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Active,
    Idle,
}

impl Filter {
    pub fn matches(&self, status: AgentStatus) -> bool {
        match self {
            Filter::All => true,
            Filter::Active => status != AgentStatus::Idle,
            Filter::Idle => status == AgentStatus::Idle,
        }
    }
}

/// Apply a filter to already-built groups: drop non-matching children inside
/// each group, then drop groups where neither the parent nor any child match.
/// (When the parent doesn't match but a child does, the parent is kept as
/// context — the alternative is orphaned sub-agent tiles.)
pub fn apply_filter(groups: Vec<AgentGroup>, filter: Filter) -> Vec<AgentGroup> {
    groups
        .into_iter()
        .filter_map(|mut g| {
            let parent_match = filter.matches(g.parent.status);
            g.children.retain(|c| filter.matches(c.status));
            if parent_match || !g.children.is_empty() {
                Some(g)
            } else {
                None
            }
        })
        .collect()
}

/// Build groups from a flat list. Orphan sub-agents (parent not seen) are
/// promoted to root entries so nothing gets lost.
pub fn build_groups(agents: &[AgentSnapshot]) -> Vec<AgentGroup> {
    use std::collections::HashMap;

    let by_id: HashMap<&str, &AgentSnapshot> =
        agents.iter().map(|a| (a.session_id.as_str(), a)).collect();

    let mut children_of: HashMap<String, Vec<AgentSnapshot>> = HashMap::new();
    let mut roots: Vec<AgentSnapshot> = Vec::new();

    for a in agents {
        match &a.parent_id {
            Some(pid) if by_id.contains_key(pid.as_str()) => {
                children_of.entry(pid.clone()).or_default().push(a.clone());
            }
            _ => roots.push(a.clone()),
        }
    }

    let mut groups: Vec<AgentGroup> = roots
        .into_iter()
        .map(|parent| {
            let mut children = children_of.remove(&parent.session_id).unwrap_or_default();
            children.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
            AgentGroup { parent, children }
        })
        .collect();

    groups.sort_by(|a, b| {
        a.aggregate_status()
            .priority()
            .cmp(&b.aggregate_status().priority())
            .then_with(|| b.last_activity().cmp(a.last_activity()))
    });

    groups
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
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSettings {
    pub idle_timeout_secs: u64,
    pub permission_timeout_secs: u64,
    pub text_idle_secs: u64,
    pub message_preview_chars: usize,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HooksStatus {
    pub registered: bool,
    pub url: String,
    pub port: u16,
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

const AVATARS: [&str; 12] = [
    "byte_owl",
    "circuit_cat",
    "amber_bot",
    "gray_golem",
    "red_imp",
    "mint_mite",
    "violet_node",
    "teal_turtle",
    "gold_bug",
    "blue_stack",
    "rose_sprite",
    "mono_fallback",
];

/// Pick a sprite name deterministically from the agent id (FNV-1a 32-bit) so
/// the same session always shows the same character across reloads.
pub fn avatar_for(id: &str) -> &'static str {
    let mut h: u32 = 0x811c9dc5;
    for b in id.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    AVATARS[(h as usize) % AVATARS.len()]
}

/// URL of an avatar PNG at a given pixel size (32/48/64/96/128/256).
pub fn avatar_url(name: &str, size: u32) -> String {
    format!("/avatars/{name}/{name}_{size}.png")
}
