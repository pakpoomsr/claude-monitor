mod components;
mod tauri_bridge;
mod types;

use leptos::prelude::*;

use components::{
    agent_detail::AgentDetail, agent_grid::AgentGrid, api_usage_panel::ApiUsagePanel,
    settings::SettingsPanel, usage_panel::UsagePanel,
};
use tauri_bridge::{invoke_no_args, listen};
use types::AgentSnapshot;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Agents,
    Usage,
    Api,
    Settings,
}

fn main() {
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

    // Live updates via Tauri events
    listen::<AgentSnapshot, _>("agent-status", move |snap| {
        set_agents.update(|list| {
            if let Some(existing) = list.iter_mut().find(|a| a.session_id == snap.session_id) {
                *existing = snap;
            } else {
                list.push(snap);
            }
        });
    });

    listen::<AgentSnapshot, _>("permission-needed", move |snap| {
        let label = if snap.project.is_empty() {
            types::short_id(&snap.session_id)
        } else {
            types::project_label(&snap.project)
        };
        set_toast.set(Some(format!("⚠ {label} needs permission")));
        // Auto-clear after 6s
        let set_toast = set_toast.clone();
        leptos::task::spawn_local(async move {
            gloo_sleep(6000).await;
            set_toast.set(None);
        });
    });

    let agent_count = move || agents.with(|a| a.len());
    let working_count = move || {
        agents.with(|a| {
            a.iter()
                .filter(|s| matches!(s.status, types::AgentStatus::Working))
                .count()
        })
    };
    let idle_count = move || {
        agents.with(|a| {
            a.iter()
                .filter(|s| matches!(s.status, types::AgentStatus::Idle))
                .count()
        })
    };
    let permission_count = move || {
        agents.with(|a| {
            a.iter()
                .filter(|s| matches!(s.status, types::AgentStatus::NeedsPermission))
                .count()
        })
    };

    view! {
        <div class="app">
            <header class="header">
                <h1 class="logo">"CLAUDE MONITOR"</h1>
                <div class="stats">
                    <span class="stat">
                        <span class="dot working"></span>
                        {move || format!("{} working", working_count())}
                    </span>
                    <span class="stat">
                        <span class="dot idle"></span>
                        {move || format!("{} idle", idle_count())}
                    </span>
                    <span class="stat alert" class:hidden=move || permission_count() == 0>
                        <span class="dot permission"></span>
                        {move || format!("{} need permission", permission_count())}
                    </span>
                    <span class="stat muted">
                        {move || format!("{} total", agent_count())}
                    </span>
                </div>
            </header>

            <nav class="tabs">
                <TabButton tab=Tab::Agents current=tab set_tab label="Agents" />
                <TabButton tab=Tab::Usage current=tab set_tab label="Usage" />
                <TabButton tab=Tab::Api current=tab set_tab label="API" />
                <TabButton tab=Tab::Settings current=tab set_tab label="Settings" />
            </nav>

            <main class="main">
                {move || match tab.get() {
                    Tab::Agents => view! {
                        <div class="agents-layout">
                            <AgentGrid agents set_selected />
                            <AgentDetail agents selected />
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

/// Tiny sleep helper that doesn't pull in the gloo crate.
async fn gloo_sleep(ms: i32) {
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let window = web_sys::window().expect("no window");
        let _ = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
    });
    let _ = JsFuture::from(promise).await;
}
