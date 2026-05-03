mod components;
mod tauri_bridge;
mod types;

use leptos::prelude::*;

use components::{
    agent_detail::AgentDetail, agent_grid::AgentGrid, api_usage_panel::ApiUsagePanel,
    settings::SettingsPanel, usage_panel::UsagePanel,
};
use tauri_bridge::{invoke_no_args, listen};
use types::{AgentSnapshot, AgentStatus};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Agents,
    History,
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

    // Initial load
    leptos::task::spawn_local(async move {
        if let Ok(list) = invoke_no_args::<Vec<AgentSnapshot>>("list_agents").await {
            set_agents.set(list);
        }
    });

    // Live updates
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

    let working_count = move || {
        agents.with(|a| a.iter().filter(|s| s.status == AgentStatus::Working).count())
    };
    let waiting_count = move || {
        agents.with(|a| a.iter().filter(|s| s.status == AgentStatus::Waiting).count())
    };
    let idle_count = move || {
        agents.with(|a| a.iter().filter(|s| s.status == AgentStatus::Idle).count())
    };
    let active_count = move || working_count() + waiting_count();

    let active_agents = Signal::derive(move || {
        let mut list: Vec<AgentSnapshot> = agents.with(|a| {
            a.iter().filter(|s| s.status.is_active()).cloned().collect()
        });
        list.sort_by(|a, b| {
            let prio = |s: &AgentSnapshot| match s.status {
                AgentStatus::Waiting => 0,
                AgentStatus::Error => 1,
                AgentStatus::Working => 2,
                AgentStatus::Idle => 3,
            };
            prio(a).cmp(&prio(b)).then_with(|| b.last_activity.cmp(&a.last_activity))
        });
        list
    });

    let history_agents = Signal::derive(move || {
        let mut list: Vec<AgentSnapshot> = agents.with(|a| {
            a.iter().filter(|s| !s.status.is_active()).cloned().collect()
        });
        list.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
        list
    });

    view! {
        <div class="app">
            <header class="header">
                <h1 class="logo">"CLAUDE MONITOR"</h1>
                <div class="stats">
                    <span class="stat">
                        <span class="dot working"></span>
                        {move || format!("{} working", working_count())}
                    </span>
                    <span class="stat alert" class:hidden=move || waiting_count() == 0>
                        <span class="dot waiting"></span>
                        {move || format!("{} waiting", waiting_count())}
                    </span>
                    <span class="stat muted">
                        {move || format!("{} ended", idle_count())}
                    </span>
                </div>
            </header>

            <nav class="tabs">
                <TabButton
                    tab=Tab::Agents current=tab set_tab
                    label="Agents"
                    badge=Signal::derive(move || active_count())
                />
                <TabButton
                    tab=Tab::History current=tab set_tab
                    label="History"
                    badge=Signal::derive(move || idle_count())
                />
                <TabButton tab=Tab::Usage    current=tab set_tab label="Usage"    badge=Signal::derive(|| 0) />
                <TabButton tab=Tab::Api      current=tab set_tab label="API"      badge=Signal::derive(|| 0) />
                <TabButton tab=Tab::Settings current=tab set_tab label="Settings" badge=Signal::derive(|| 0) />
            </nav>

            <main class="main">
                {move || match tab.get() {
                    Tab::Agents => view! {
                        <div class="agents-layout" class:has-detail=move || selected.get().is_some()>
                            <AgentGrid
                                agents=active_agents
                                set_selected
                                empty_message="No active agents. Start a Claude Code session and it will appear here."
                            />
                            {move || selected.get().is_some().then(|| view! {
                                <AgentDetail agents selected set_selected />
                            })}
                        </div>
                    }.into_any(),
                    Tab::History => view! {
                        <div class="agents-layout" class:has-detail=move || selected.get().is_some()>
                            <AgentGrid
                                agents=history_agents
                                set_selected
                                empty_message="No ended sessions yet."
                            />
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
fn TabButton(
    tab: Tab,
    current: ReadSignal<Tab>,
    set_tab: WriteSignal<Tab>,
    label: &'static str,
    badge: Signal<usize>,
) -> impl IntoView {
    let is_active = move || current.get() == tab;
    view! {
        <button
            class="tab-button"
            class:active=is_active
            on:click=move |_| set_tab.set(tab)
        >
            {label}
            {move || {
                let n = badge.get();
                (n > 0).then(|| view! { <span class="tab-badge">{n}</span> })
            }}
        </button>
    }
}

/// Tiny sleep helper that doesn't pull in the gloo crate.
async fn gloo_sleep(ms: i32) {
    use wasm_bindgen_futures::JsFuture;
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let window = web_sys::window().expect("no window");
        let _ = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
    });
    let _ = JsFuture::from(promise).await;
}
