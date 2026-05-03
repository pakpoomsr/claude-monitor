use leptos::prelude::*;
use serde::Serialize;

use crate::tauri_bridge::{invoke_no_args, invoke};
use crate::types::{AgentSettings, HooksStatus};

#[derive(Serialize)]
struct SetSettingsArgs {
    settings: AgentSettings,
}

#[component]
pub fn SettingsPanel() -> impl IntoView {
    let (settings, set_settings) = signal(AgentSettings::default());
    let (status, set_status) = signal::<Option<String>>(None);
    let (hooks, set_hooks) = signal(HooksStatus::default());
    let (hooks_msg, set_hooks_msg) = signal::<Option<String>>(None);

    leptos::task::spawn_local(async move {
        if let Ok(s) = invoke_no_args::<AgentSettings>("get_agent_settings").await {
            set_settings.set(s);
        }
        if let Ok(h) = invoke_no_args::<HooksStatus>("hooks_status").await {
            set_hooks.set(h);
        }
    });

    let save = move |_| {
        let s = settings.get();
        leptos::task::spawn_local(async move {
            match invoke::<(), _>("set_agent_settings", &SetSettingsArgs { settings: s }).await {
                Ok(_) => set_status.set(Some("Saved.".into())),
                Err(e) => set_status.set(Some(format!("Error: {e}"))),
            }
        });
    };

    let toggle_hooks = move |_| {
        let currently_on = hooks.get().registered;
        let cmd = if currently_on { "unregister_hooks" } else { "register_hooks" };
        leptos::task::spawn_local(async move {
            match invoke_no_args::<HooksStatus>(cmd).await {
                Ok(h) => {
                    set_hooks.set(h);
                    set_hooks_msg.set(Some(if currently_on {
                        "Hooks removed from ~/.claude/settings.json".into()
                    } else {
                        "Hooks registered. Claude Code picks them up live (no restart).".into()
                    }));
                }
                Err(e) => set_hooks_msg.set(Some(format!("Error: {e}"))),
            }
        });
    };

    view! {
        <section class="panel">
            <h2>"Real-time hooks"</h2>
            <div class="form">
                <div class="hooks-row">
                    <span class=move || {
                        if hooks.get().registered { "hooks-status on" } else { "hooks-status off" }
                    }>
                        <span class="dot working"></span>
                        {move || if hooks.get().registered { "Registered" } else { "Not registered" }}
                    </span>
                    <code class="muted small" style="margin-left: 12px;">
                        {move || {
                            let h = hooks.get();
                            if h.port == 0 { "(server not yet running)".to_string() }
                            else { format!("server: {}", h.url) }
                        }}
                    </code>
                </div>

                <div class="form-row">
                    <button
                        class=move || if hooks.get().registered { "btn" } else { "btn primary" }
                        on:click=toggle_hooks
                    >
                        {move || if hooks.get().registered { "Disable hooks" } else { "Set up hooks" }}
                    </button>
                    {move || hooks_msg.get().map(|m| view! { <span class="muted small">{m}</span> })}
                </div>

                <small class="muted">
                    "Adds 10 hook entries to ~/.claude/settings.json (a backup is written to \
                     settings.json.bak first). Hooks POST to a localhost server bound to a \
                     random ephemeral port — observe-only, never blocks Claude. When active, \
                     they replace the JSONL-based timer heuristics with authoritative events."
                </small>
            </div>

            <h2 style="margin-top: 16px;">"Heuristics (JSONL fallback)"</h2>

            <div class="form">
                <label class="form-field">
                    <span>"Idle timeout (seconds)"</span>
                    <input
                        type="number"
                        min="5"
                        max="3600"
                        prop:value=move || settings.get().idle_timeout_secs.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = event_target_value(&ev).parse::<u64>() {
                                set_settings.update(|s| s.idle_timeout_secs = v);
                            }
                        }
                    />
                    <small class="muted">"How long without events before an agent is marked idle."</small>
                </label>

                <label class="form-field">
                    <span>"Permission timeout (seconds)"</span>
                    <input
                        type="number"
                        min="1"
                        max="60"
                        prop:value=move || settings.get().permission_timeout_secs.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = event_target_value(&ev).parse::<u64>() {
                                set_settings.update(|s| s.permission_timeout_secs = v);
                            }
                        }
                    />
                    <small class="muted">
                        "If a tool has no result after this many seconds, treat as Waiting \
                         (Claude is likely blocked on a permission prompt)."
                    </small>
                </label>

                <label class="form-field">
                    <span>"Text idle delay (seconds)"</span>
                    <input
                        type="number"
                        min="1"
                        max="60"
                        prop:value=move || settings.get().text_idle_secs.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = event_target_value(&ev).parse::<u64>() {
                                set_settings.update(|s| s.text_idle_secs = v);
                            }
                        }
                    />
                    <small class="muted">
                        "On a text-only turn (no tool used), wait this many seconds after \
                         the last assistant text before flipping to Waiting. \
                         Backup signal when turn_duration markers are missing."
                    </small>
                </label>

                <label class="form-field">
                    <span>"Hook grace (seconds)"</span>
                    <input
                        type="number"
                        min="5"
                        max="600"
                        prop:value=move || settings.get().hook_grace_secs.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = event_target_value(&ev).parse::<u64>() {
                                set_settings.update(|s| s.hook_grace_secs = v);
                            }
                        }
                    />
                    <small class="muted">
                        "When a hook event has fired within this window, hooks are treated \
                         as authoritative for the agent. Outside it, JSONL heuristics take over."
                    </small>
                </label>

                <label class="form-field">
                    <span>"Message preview length"</span>
                    <input
                        type="number"
                        min="40"
                        max="2000"
                        prop:value=move || settings.get().message_preview_chars.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = event_target_value(&ev).parse::<usize>() {
                                set_settings.update(|s| s.message_preview_chars = v);
                            }
                        }
                    />
                </label>

                <div class="form-row">
                    <button class="btn primary" on:click=save>"Save"</button>
                    {move || status.get().map(|m| view! { <span class="muted">{m}</span> })}
                </div>
            </div>
        </section>
    }
}
