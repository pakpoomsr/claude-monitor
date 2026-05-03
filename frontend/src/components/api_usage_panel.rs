use leptos::prelude::*;
use serde::Serialize;

use crate::tauri_bridge::{invoke, invoke_no_args};
use crate::types::UsageResponse;

#[derive(Serialize)]
struct SetKeyArgs {
    key: String,
}

#[component]
pub fn ApiUsagePanel() -> impl IntoView {
    let (key_input, set_key_input) = signal(String::new());
    let (data, set_data) = signal::<Option<UsageResponse>>(None);
    let (err, set_err) = signal::<Option<String>>(None);
    let (loading, set_loading) = signal(false);

    let on_save_key = move |_| {
        let k = key_input.get();
        leptos::task::spawn_local(async move {
            if let Err(e) = invoke::<(), _>("set_api_key", &SetKeyArgs { key: k }).await {
                set_err.set(Some(e));
            } else {
                set_err.set(None);
            }
        });
    };

    let on_fetch = move |_| {
        set_loading.set(true);
        leptos::task::spawn_local(async move {
            match invoke_no_args::<UsageResponse>("fetch_api_usage").await {
                Ok(d) => {
                    set_data.set(Some(d));
                    set_err.set(None);
                }
                Err(e) => set_err.set(Some(e)),
            }
            set_loading.set(false);
        });
    };

    view! {
        <section class="panel">
            <h2>"Anthropic API usage"</h2>
            <p class="muted">
                "Optional. Requires an API key with billing read access. \
                 The key is held in memory only and never written to disk."
            </p>

            <div class="form-row">
                <input
                    type="password"
                    placeholder="sk-ant-..."
                    prop:value=move || key_input.get()
                    on:input=move |ev| set_key_input.set(event_target_value(&ev))
                />
                <button class="btn" on:click=on_save_key>"Save key"</button>
                <button class="btn primary" on:click=on_fetch disabled=move || loading.get()>
                    {move || if loading.get() { "Loading..." } else { "Fetch usage" }}
                </button>
            </div>

            {move || err.get().map(|e| view! { <div class="error-box">{e}</div> })}

            {move || data.get().map(|d| view! {
                <div class="api-summary">
                    <div class="muted">{format!("{} → {}", d.period_start, d.period_end)}</div>
                    <div class="summary-cards">
                        <Card label="Input tokens" value=format_num(d.total_input_tokens) />
                        <Card label="Output tokens" value=format_num(d.total_output_tokens) />
                        <Card label="Cost" value=format!("${:.2}", d.total_cost_usd) />
                    </div>
                    <h3>"By model"</h3>
                    <table class="usage-table">
                        <thead>
                            <tr><th>"Model"</th><th>"Input"</th><th>"Output"</th><th>"Cost"</th></tr>
                        </thead>
                        <tbody>
                            {d.by_model.into_iter().map(|m| view! {
                                <tr>
                                    <td>{m.model}</td>
                                    <td>{format_num(m.input_tokens)}</td>
                                    <td>{format_num(m.output_tokens)}</td>
                                    <td>{format!("${:.4}", m.cost_usd)}</td>
                                </tr>
                            }).collect_view()}
                        </tbody>
                    </table>
                </div>
            })}
        </section>
    }
}

#[component]
fn Card(label: &'static str, value: String) -> impl IntoView {
    view! {
        <div class="card">
            <div class="card-label muted">{label}</div>
            <div class="card-value">{value}</div>
        </div>
    }
}

fn format_num(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}
