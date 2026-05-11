mod components;
mod office;
mod tauri_bridge;
mod types;

use leptos::prelude::*;
use std::collections::{HashMap, VecDeque};

use components::{
    agent_detail::AgentDetail, agent_grid::AgentGrid, api_usage_panel::ApiUsagePanel,
    history_panel::HistoryPanel, office_panel::OfficePanel, settings::SettingsPanel,
    sponsor::SponsorPanel, usage_panel::UsagePanel,
};
use tauri_bridge::{invoke_no_args, listen};
use types::{apply_filter, build_groups, AgentGroup, AgentSnapshot, AgentStatus, CurrencyState, Filter, HooksStatus, LogEntry, PricingTable};

/// Per-session ring buffer of streamed log entries. Mirrors the backend
/// `EVENT_RING_CAP` so a long-running session can't grow this unbounded.
pub type EventLogMap = HashMap<String, VecDeque<LogEntry>>;
const FRONTEND_RING_CAP: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Agents,
    Office,
    Usage,
    History,
    Api,
    Settings,
    Sponsor,
}

fn main() {
    console_error_panic_hook::set_once();
    init_theme_from_storage();
    leptos::mount::mount_to_body(App);
}

/// Read the saved theme (if any) from localStorage and apply it before mount,
/// so the UI doesn't flash the wrong palette.
fn init_theme_from_storage() {
    if let Some(window) = web_sys::window()
        && let Ok(Some(storage)) = window.local_storage()
        && let Ok(Some(theme)) = storage.get_item("cm-theme")
        && (theme == "light" || theme == "dark")
        && let Some(doc) = window.document()
        && let Some(el) = doc.document_element()
    {
        let _ = el.set_attribute("data-theme", &theme);
    }
}

fn current_theme() -> String {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|el| el.get_attribute("data-theme"))
        .unwrap_or_else(|| {
            // Fall back to the OS preference so the toggle's first click flips
            // to the *other* theme rather than re-asserting the OS one.
            web_sys::window()
                .and_then(|w| w.match_media("(prefers-color-scheme: light)").ok().flatten())
                .map(|mql| if mql.matches() { "light".to_string() } else { "dark".to_string() })
                .unwrap_or_else(|| "dark".to_string())
        })
}

fn set_theme(theme: &str) {
    if let Some(window) = web_sys::window() {
        if let Some(doc) = window.document()
            && let Some(el) = doc.document_element()
        {
            let _ = el.set_attribute("data-theme", theme);
        }
        if let Ok(Some(storage)) = window.local_storage() {
            let _ = storage.set_item("cm-theme", theme);
        }
    }
}

#[component]
fn App() -> impl IntoView {
    let (agents, set_agents) = signal(Vec::<AgentSnapshot>::new());
    let (selected, set_selected) = signal::<Option<String>>(None);
    let (tab, set_tab) = signal(Tab::Agents);
    let (toast, set_toast) = signal::<Option<String>>(None);
    let (filter, set_filter) = signal(Filter::Active);
    let (hooks, set_hooks) = signal(HooksStatus::default());
    let (theme, set_theme_sig) = signal(current_theme());

    // Shared pricing table — populated on startup, mutated by the Settings
    // panel, read by every cost calculation in the UI. Provided via context
    // so leaf components don't need props plumbed through.
    let pricing: RwSignal<PricingTable> = RwSignal::new(PricingTable::default());
    provide_context(pricing);
    leptos::task::spawn_local(async move {
        if let Ok(p) = invoke_no_args::<PricingTable>("get_pricing").await {
            pricing.set(p);
        }
    });

    // Shared currency state — display currency + cached FX rates. Same
    // pattern as pricing.
    let currency: RwSignal<CurrencyState> = RwSignal::new(CurrencyState::default());
    provide_context(currency);
    leptos::task::spawn_local(async move {
        if let Ok(c) = invoke_no_args::<CurrencyState>("get_currency_state").await {
            currency.set(c);
        }
    });

    // Per-session event log buckets. Populated by the `agent-event` listener
    // and the on-selection backfill effect in AgentDetail. Kept in context so
    // the detail pane (a leaf) can read/write without prop drilling.
    let event_log: RwSignal<EventLogMap> = RwSignal::new(HashMap::new());
    provide_context(event_log);

    listen::<LogEntry, _>("agent-event", move |entry| {
        event_log.update(|m| {
            let q = m.entry(entry.session_id.clone()).or_default();
            q.push_back(entry);
            while q.len() > FRONTEND_RING_CAP {
                q.pop_front();
            }
        });
    });

    // Periodically poll hooks_status so the indicator updates when the user
    // toggles registration in the Settings panel. Cheap (single Tauri call).
    leptos::task::spawn_local(async move {
        loop {
            if let Ok(h) = invoke_no_args::<HooksStatus>("hooks_status").await {
                set_hooks.set(h);
            }
            gloo_sleep(2000).await;
        }
    });

    leptos::task::spawn_local(async move {
        if let Ok(list) = invoke_no_args::<Vec<AgentSnapshot>>("list_agents").await {
            set_agents.set(list);
        }
    });

    listen::<AgentSnapshot, _>("agent-status", move |snap| {
        set_agents.update(|list| {
            if let Some(existing) = list.iter_mut().find(|a| a.session_id == snap.session_id) {
                *existing = snap;
            } else {
                list.push(snap);
            }
        });
    });

    listen::<AgentSnapshot, _>("agent-waiting", move |snap| {
        let label = if snap.project.is_empty() {
            types::short_id(&snap.session_id)
        } else {
            types::project_label(&snap.project)
        };
        set_toast.set(Some(format!("⏳ {label} is waiting")));
        leptos::task::spawn_local(async move {
            gloo_sleep(6000).await;
            set_toast.set(None);
        });
    });

    let groups = Signal::derive(move || agents.with(|list| build_groups(list)));

    // Group-level counts (matches what the user sees in tiles).
    let working_groups = Signal::derive(move || {
        groups.with(|gs| gs.iter().filter(|g| g.aggregate_status() == AgentStatus::Working).count())
    });
    let waiting_groups = Signal::derive(move || {
        groups.with(|gs| gs.iter().filter(|g| g.aggregate_status() == AgentStatus::Waiting).count())
    });
    let idle_groups = Signal::derive(move || {
        groups.with(|gs| gs.iter().filter(|g| g.aggregate_status() == AgentStatus::Idle).count())
    });

    // Apply the user's filter to the groups (also drops non-matching subs
    // inside each group). Then split into Active / Idle sections — when the
    // user filtered to one of those, the other section just renders empty
    // and gets hidden by AgentGrid's empty-message fallback.
    let filtered_groups = Signal::derive(move || apply_filter(groups.get(), filter.get()));

    let active_groups = Signal::derive(move || {
        filtered_groups.with(|gs| {
            gs.iter()
                .filter(|g| g.aggregate_status() != AgentStatus::Idle)
                .cloned()
                .collect::<Vec<AgentGroup>>()
        })
    });
    let idle_groups_list = Signal::derive(move || {
        filtered_groups.with(|gs| {
            gs.iter()
                .filter(|g| g.aggregate_status() == AgentStatus::Idle)
                .cloned()
                .collect::<Vec<AgentGroup>>()
        })
    });

    view! {
        <div class="app">
            <header class="header">
                <h1 class="logo">
                    "CLAUDE MONITOR"
                    <span class="logo-version">{format!("v{}", env!("CARGO_PKG_VERSION"))}</span>
                </h1>
                <div class="stats">
                    <span class="stat">
                        <span class="dot working"></span>
                        {move || format!("{} working", working_groups.get())}
                    </span>
                    <span class="stat alert" class:hidden=move || waiting_groups.get() == 0>
                        <span class="dot waiting"></span>
                        {move || format!("{} waiting", waiting_groups.get())}
                    </span>
                    <span class="stat muted">
                        {move || format!("{} idle", idle_groups.get())}
                    </span>
                    <span
                        class="stat hook-badge"
                        class:on=move || hooks.get().registered
                        title=move || if hooks.get().registered {
                            "Real-time hooks active — Claude Code is reporting events live."
                        } else {
                            "Hooks not registered. Using JSONL heuristics. Set up in Settings."
                        }
                    >
                        <span class="dot"></span>
                        "HOOKS"
                    </span>
                    <button
                        class="theme-toggle"
                        title=move || if theme.get() == "light" { "Switch to dark theme" } else { "Switch to light theme" }
                        on:click=move |_| {
                            let next = if theme.get() == "light" { "dark" } else { "light" };
                            set_theme(next);
                            set_theme_sig.set(next.into());
                        }
                    >
                        {move || if theme.get() == "light" {
                            // moon
                            view! {
                                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
                                </svg>
                            }.into_any()
                        } else {
                            // sun
                            view! {
                                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <circle cx="12" cy="12" r="4" />
                                    <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41" />
                                </svg>
                            }.into_any()
                        }}
                    </button>
                </div>
            </header>

            <nav class="tabs">
                <TabButton tab=Tab::Agents   current=tab set_tab label="Agents" />
                <TabButton tab=Tab::Office   current=tab set_tab label="Office" />
                <TabButton tab=Tab::Usage    current=tab set_tab label="Usage" />
                <TabButton tab=Tab::History  current=tab set_tab label="History" />
                <TabButton tab=Tab::Api      current=tab set_tab label="API" />
                <TabButton tab=Tab::Settings current=tab set_tab label="Settings" />
                <TabButton tab=Tab::Sponsor  current=tab set_tab label="❤ Sponsor" />
            </nav>

            <main class="main">
                {move || match tab.get() {
                    Tab::Agents => view! {
                        <div class="agents-layout" class:has-detail=move || selected.get().is_some()>
                            <div class="agent-sections">
                                <FilterBar filter set_filter
                                    working_count=working_groups
                                    waiting_count=waiting_groups
                                    idle_count=idle_groups
                                />
                                <AgentGrid
                                    groups=active_groups
                                    set_selected
                                    section_label="Active"
                                    empty_message=Signal::derive(move || match filter.get() {
                                        Filter::Idle => String::new(),
                                        _ => "No active agents.".into(),
                                    })
                                />
                                <AgentGrid
                                    groups=idle_groups_list
                                    set_selected
                                    section_label="Idle"
                                    empty_message=Signal::derive(move || match filter.get() {
                                        Filter::Idle => "No idle agents.".into(),
                                        _ => String::new(),
                                    })
                                />
                            </div>
                            {move || selected.get().is_some().then(|| view! {
                                <AgentDetail agents selected set_selected />
                            })}
                        </div>
                    }.into_any(),
                    Tab::Office => view! { <OfficePanel agents /> }.into_any(),
                    Tab::Usage => view! { <UsagePanel /> }.into_any(),
                    Tab::History => view! { <HistoryPanel /> }.into_any(),
                    Tab::Api => view! { <ApiUsagePanel /> }.into_any(),
                    Tab::Settings => view! { <SettingsPanel /> }.into_any(),
                    Tab::Sponsor => view! { <SponsorPanel /> }.into_any(),
                }}
            </main>

            {move || toast.get().map(|msg| view! {
                <div class="toast">{msg}</div>
            })}
        </div>
    }
}

#[component]
fn FilterBar(
    filter: ReadSignal<Filter>,
    set_filter: WriteSignal<Filter>,
    working_count: Signal<usize>,
    waiting_count: Signal<usize>,
    idle_count: Signal<usize>,
) -> impl IntoView {
    let pill = move |f: Filter, label: &'static str, count: Signal<usize>| {
        let is_active = move || filter.get() == f;
        view! {
            <button
                class="filter-pill"
                class:active=is_active
                on:click=move |_| set_filter.set(f)
            >
                <span>{label}</span>
                <span class="filter-count">{move || count.get()}</span>
            </button>
        }
    };

    let total = Signal::derive(move || working_count.get() + waiting_count.get() + idle_count.get());
    let active = Signal::derive(move || working_count.get() + waiting_count.get());

    view! {
        <div class="filter-bar">
            {pill(Filter::All,    "All",    total)}
            {pill(Filter::Active, "Active", active)}
            {pill(Filter::Idle,   "Idle",   idle_count)}
        </div>
    }
}

#[component]
fn TabButton(
    tab: Tab,
    current: ReadSignal<Tab>,
    set_tab: WriteSignal<Tab>,
    label: &'static str,
) -> impl IntoView {
    let is_active = move || current.get() == tab;
    view! {
        <button
            class="tab-button"
            class:active=is_active
            on:click=move |_| set_tab.set(tab)
        >
            {label}
        </button>
    }
}

async fn gloo_sleep(ms: i32) {
    use wasm_bindgen_futures::JsFuture;
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let window = web_sys::window().expect("no window");
        let _ = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
    });
    let _ = JsFuture::from(promise).await;
}
