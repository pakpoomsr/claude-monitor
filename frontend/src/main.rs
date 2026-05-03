mod components;
mod tauri_bridge;
mod types;

use leptos::prelude::*;

use components::{
    agent_detail::AgentDetail, agent_grid::AgentGrid, api_usage_panel::ApiUsagePanel,
    settings::SettingsPanel, usage_panel::UsagePanel,
};
use tauri_bridge::{invoke_no_args, listen};
use types::{apply_filter, build_groups, AgentGroup, AgentSnapshot, AgentStatus, Filter};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Agents,
    Usage,
    Api,
    Settings,
}

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    let (agents, set_agents) = signal(Vec::<AgentSnapshot>::new());
    let (selected, set_selected) = signal::<Option<String>>(None);
    let (tab, set_tab) = signal(Tab::Agents);
    let (toast, set_toast) = signal::<Option<String>>(None);
    let (filter, set_filter) = signal(Filter::Active);

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
        set_toast.set(Some(format!("⏳ {label} is waiting for your response")));
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
                <h1 class="logo">"CLAUDE MONITOR"</h1>
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
                </div>
            </header>

            <nav class="tabs">
                <TabButton tab=Tab::Agents   current=tab set_tab label="Agents" />
                <TabButton tab=Tab::Usage    current=tab set_tab label="Usage" />
                <TabButton tab=Tab::Api      current=tab set_tab label="API" />
                <TabButton tab=Tab::Settings current=tab set_tab label="Settings" />
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
                    Tab::Usage => view! { <UsagePanel /> }.into_any(),
                    Tab::Api => view! { <ApiUsagePanel /> }.into_any(),
                    Tab::Settings => view! { <SettingsPanel /> }.into_any(),
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
