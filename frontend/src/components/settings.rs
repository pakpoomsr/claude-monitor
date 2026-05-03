use leptos::prelude::*;
use serde::Serialize;

use crate::tauri_bridge::{invoke, invoke_no_args};
use crate::types::AgentSettings;

#[derive(Serialize)]
struct SetSettingsArgs {
    settings: AgentSettings,
}

#[component]
pub fn SettingsPanel() -> impl IntoView {
    let (settings, set_settings) = signal(AgentSettings::default());
    let (status, set_status) = signal::<Option<String>>(None);

    leptos::task::spawn_local(async move {
        if let Ok(s) = invoke_no_args::<AgentSettings>("get_agent_settings").await {
            set_settings.set(s);
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

    view! {
        <section class="panel">
            <h2>"Settings"</h2>

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
                        "If a tool_use has no tool_result after this many seconds, \
                         the agent is flagged as needing permission."
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
