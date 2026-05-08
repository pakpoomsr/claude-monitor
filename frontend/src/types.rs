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
            AgentStatus::Idle => "Idle",
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
    pub cache_write_5m_tokens: u64,
    pub cache_write_1h_tokens: u64,
    pub cache_read_tokens: u64,
    /// Sum of the three cache columns, kept for any consumer that wants
    /// a single combined cache figure (e.g. the cache-hit meter).
    pub cache_tokens: u64,
    pub cost_usd: f64,
    pub last_activity: String,
    pub started_at: String,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogSource {
    Jsonl,
    Hook,
}

impl LogSource {
    pub fn css_class(&self) -> &'static str {
        match self {
            LogSource::Jsonl => "jsonl",
            LogSource::Hook => "hook",
        }
    }
}

/// Mirror of `agents::LogEntry`. `timestamp` is RFC3339 (Tauri serializes
/// `chrono::DateTime<Utc>` as a string).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogEntry {
    pub session_id: String,
    pub timestamp: String,
    pub source: LogSource,
    pub kind: String,
    pub summary: String,
    pub details: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct ModelPricing {
    pub base_input: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
    pub output: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingEntry {
    pub id: String,
    pub label: String,
    pub deprecated: bool,
    pub pricing: ModelPricing,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PricingTable {
    pub entries: Vec<PricingEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrencyInfo {
    pub code: String,
    pub symbol: String,
    pub rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CurrencyState {
    pub active: String,
    pub list: Vec<CurrencyInfo>,
    pub fetched_at: Option<String>,
}

impl CurrencyState {
    /// Look up the active currency's rate (units per USD). USD or unknown
    /// defaults to 1.0 — i.e. show the dollar amount unchanged.
    pub fn active_rate(&self) -> (String, f64) {
        for c in &self.list {
            if c.code == self.active {
                return (c.symbol.clone(), c.rate);
            }
        }
        ("$".to_string(), 1.0)
    }
}

/// Format an RFC3339 timestamp as "DD MMM YYYY HH:MM:SS" (e.g.
/// "04 May 2026 14:22:34"). Returns the input unchanged on parse failure
/// so callers don't need to fall back themselves.
pub fn format_datetime(rfc3339: &str) -> String {
    let (date, rest) = match rfc3339.split_once('T') {
        Some(p) => p,
        None => return rfc3339.to_string(),
    };
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return rfc3339.to_string();
    }
    let (year, month, day) = (parts[0], parts[1], parts[2]);
    let month_name = match month {
        "01" => "Jan", "02" => "Feb", "03" => "Mar", "04" => "Apr",
        "05" => "May", "06" => "Jun", "07" => "Jul", "08" => "Aug",
        "09" => "Sep", "10" => "Oct", "11" => "Nov", "12" => "Dec",
        _ => return rfc3339.to_string(),
    };
    // Trim fractional seconds and timezone — keep only HH:MM:SS.
    let time: String = rest
        .chars()
        .take_while(|c| *c != '.' && *c != '+' && *c != 'Z' && *c != '-')
        .collect();
    if time.is_empty() {
        format!("{day} {month_name} {year}")
    } else {
        format!("{day} {month_name} {year} {time}")
    }
}

/// Extract `HH:MM:SS` from an RFC3339 timestamp. Returns the input on parse
/// failure. Used by the per-agent event log where the date is implied.
pub fn format_log_time(rfc3339: &str) -> String {
    let Some((_, rest)) = rfc3339.split_once('T') else {
        return rfc3339.to_string();
    };
    rest.chars()
        .take_while(|c| *c != '.' && *c != '+' && *c != 'Z' && *c != '-')
        .collect()
}

/// Format a USD amount in the currently-active currency. Symbol is prefixed
/// (e.g. "€1,234.56"); precision adapts to magnitude so micro-costs stay
/// legible, and the integer part uses comma thousand-separators.
pub fn format_money(usd: f64, state: &CurrencyState) -> String {
    let (symbol, rate) = state.active_rate();
    let v = usd * rate;
    let decimals = if v >= 1.0 { 2 } else if v >= 0.01 { 3 } else { 4 };
    format!("{symbol}{}", thousand_sep(v, decimals))
}

/// Format a non-negative `f64` with comma thousand-separators on the integer
/// part and a fixed number of fractional digits. Negative numbers fall back
/// to plain `format!`.
fn thousand_sep(v: f64, decimals: usize) -> String {
    if v < 0.0 {
        return format!("{v:.*}", decimals);
    }
    let raw = format!("{v:.*}", decimals);
    let (int_part, frac_part) = raw
        .split_once('.')
        .map(|(i, f)| (i, format!(".{f}")))
        .unwrap_or((raw.as_str(), String::new()));
    let mut with_commas = String::with_capacity(int_part.len() + int_part.len() / 3);
    for (i, c) in int_part.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            with_commas.push(',');
        }
        with_commas.push(c);
    }
    let int_with_commas: String = with_commas.chars().rev().collect();
    format!("{int_with_commas}{frac_part}")
}

/// Format a YYYY-MM-DD date string as "DD MMM YY" (e.g. "04 May 26").
/// Returns the input on parse failure.
pub fn format_date_short(yyyy_mm_dd: &str) -> String {
    let parts: Vec<&str> = yyyy_mm_dd.split('-').collect();
    if parts.len() != 3 {
        return yyyy_mm_dd.to_string();
    }
    let (year, month, day) = (parts[0], parts[1], parts[2]);
    let yy = if year.len() >= 2 { &year[year.len() - 2..] } else { year };
    let month_name = match month {
        "01" => "Jan", "02" => "Feb", "03" => "Mar", "04" => "Apr",
        "05" => "May", "06" => "Jun", "07" => "Jul", "08" => "Aug",
        "09" => "Sep", "10" => "Oct", "11" => "Nov", "12" => "Dec",
        _ => return yyyy_mm_dd.to_string(),
    };
    format!("{day} {month_name} {yy}")
}

impl PricingTable {
    pub fn pricing_for(&self, model: &str) -> ModelPricing {
        let model_lc = model.to_lowercase();
        for e in &self.entries {
            if !e.id.is_empty() && model_lc.contains(&e.id.to_lowercase()) {
                return e.pricing;
            }
        }
        // Frontend fallback mirrors the backend default (Sonnet family).
        ModelPricing {
            base_input: 3.0,
            cache_write_5m: 3.75,
            cache_write_1h: 6.0,
            cache_read: 0.30,
            output: 15.0,
        }
    }
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

const AVATARS: [&str; 8] = [
    "01_monitor_bot",
    "02_teardrop_bot",
    "03_turtle_bot",
    "04_round_bot",
    "05_cat_bot",
    "06_fox_bot",
    "07_red_probe_bot",
    "08_owl_bot",
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
