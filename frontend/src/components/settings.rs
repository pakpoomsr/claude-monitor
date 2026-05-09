use leptos::prelude::*;
use serde::Serialize;

use crate::tauri_bridge::{invoke, invoke_no_args};
use crate::types::{
    format_datetime, AgentSettings, CurrencyState, HooksStatus, PricingTable, SnapshotSettings,
};

#[derive(Serialize)]
struct SetSettingsArgs {
    settings: AgentSettings,
}

#[derive(Serialize)]
struct SetPricingArgs {
    table: PricingTable,
}

#[derive(Serialize)]
struct SetCurrencyArgs {
    code: String,
}

#[derive(Serialize)]
struct SetSnapshotSettingsArgs {
    settings: SnapshotSettings,
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

            <PricingSection />

            <SnapshotsSection />
        </section>
    }
}

/// Snapshot store toggle, retention, and disk usage. Powers the History tab.
#[component]
fn SnapshotsSection() -> impl IntoView {
    let (settings, set_settings) = signal(SnapshotSettings::default());
    let (status, set_status) = signal::<Option<String>>(None);

    leptos::task::spawn_local(async move {
        if let Ok(s) = invoke_no_args::<SnapshotSettings>("get_snapshot_settings").await {
            set_settings.set(s);
        }
    });

    let save = move |_| {
        let s = settings.get();
        leptos::task::spawn_local(async move {
            match invoke::<(), _>(
                "set_snapshot_settings",
                &SetSnapshotSettingsArgs { settings: s },
            )
            .await
            {
                Ok(_) => set_status.set(Some("Saved.".into())),
                Err(e) => set_status.set(Some(format!("Error: {e}"))),
            }
        });
    };

    let mb = move || {
        let bytes = settings.get().total_size_bytes;
        if bytes <= 0 {
            "0 KB".to_string()
        } else if bytes < 1024 * 1024 {
            format!("{} KB", bytes / 1024)
        } else {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        }
    };

    view! {
        <h2 style="margin-top: 24px;">"Snapshots (History tab)"</h2>
        <small class="muted" style="display: block; margin-bottom: 8px;">
            "Captures pre/post file content for every "
            <code>"Edit"</code> ", " <code>"Write"</code> ", "
            <code>"MultiEdit"</code> ", " <code>"NotebookEdit"</code>
            " call so you can diff and restore. Hook-driven — requires real-time hooks above."
        </small>

        <div class="form">
            <label class="form-field">
                <span>
                    <input
                        type="checkbox"
                        prop:checked=move || settings.get().enabled
                        on:change=move |ev| {
                            let checked = event_target_checked(&ev);
                            set_settings.update(|s| s.enabled = checked);
                        }
                    />
                    " Enable file snapshots"
                </span>
                <small class="muted">
                    "Disabling stops capture immediately. Existing snapshots remain on disk \
                     until pruned."
                </small>
            </label>

            <label class="form-field">
                <span>"Retention (days)"</span>
                <input
                    type="number"
                    min="1"
                    max="365"
                    prop:value=move || settings.get().retention_days.to_string()
                    on:input=move |ev| {
                        if let Ok(v) = event_target_value(&ev).parse::<u32>() {
                            set_settings.update(|s| s.retention_days = v);
                        }
                    }
                />
                <small class="muted">
                    "Older snapshots are pruned on app startup. 14 days is a sane default."
                </small>
            </label>

            <div class="form-row">
                <span class="muted small">
                    {move || format!(
                        "Disk usage: {} across {} snapshots",
                        mb(),
                        settings.get().total_count
                    )}
                </span>
            </div>

            <div class="form-row">
                <button class="btn primary" on:click=save>"Save"</button>
                {move || status.get().map(|m| view! { <span class="muted">{m}</span> })}
            </div>
        </div>
    }
}

fn event_target_checked(ev: &leptos::ev::Event) -> bool {
    use wasm_bindgen::JsCast;
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|el| el.checked())
        .unwrap_or(false)
}

/// Editable per-model price table. Source of truth is the global pricing
/// signal provided by `App`; edits push back to the backend (which persists
/// the delta as overrides in prefs.json) and update the signal so cost
/// displays elsewhere recalculate immediately.
#[component]
fn PricingSection() -> impl IntoView {
    let global_pricing = use_context::<RwSignal<PricingTable>>()
        .expect("PricingTable signal must be provided by App");
    let pricing = RwSignal::new(global_pricing.get_untracked());
    let (status, set_status) = signal::<Option<String>>(None);

    // Initial fetch in case App's spawn hadn't completed yet when we mounted.
    leptos::task::spawn_local(async move {
        if let Ok(p) = invoke_no_args::<PricingTable>("get_pricing").await {
            pricing.set(p.clone());
            global_pricing.set(p);
        }
    });

    // Push current edits to the backend AND the global signal in one shot.
    let commit = move || {
        let table = pricing.get_untracked();
        global_pricing.set(table.clone());
        leptos::task::spawn_local(async move {
            match invoke::<(), _>("set_pricing", &SetPricingArgs { table }).await {
                Ok(_) => set_status.set(Some("Pricing saved.".into())),
                Err(e) => set_status.set(Some(format!("Error: {e}"))),
            }
        });
    };

    let reset_all = move |_| {
        leptos::task::spawn_local(async move {
            match invoke_no_args::<PricingTable>("reset_pricing").await {
                Ok(p) => {
                    pricing.set(p.clone());
                    global_pricing.set(p);
                    set_status.set(Some("Reset to Anthropic defaults; currency back to USD.".into()));
                }
                Err(e) => set_status.set(Some(format!("Error: {e}"))),
            }
        });
    };

    view! {
        <CurrencySection />

        <h2 style="margin-top: 24px;">"Model pricing (USD per million tokens)"</h2>
        <small class="muted" style="display: block; margin-bottom: 8px;">
            "Defaults match Anthropic's pricing page. Edit any cell and tab out (or click anywhere outside) to save. \
             Costs across the app re-compute instantly with new rates."
        </small>

        <div class="form">
            <table class="pricing-table">
                <thead>
                    <tr>
                        <th>"Model"</th>
                        <th class="num">"Base Input"</th>
                        <th class="num">"5m Cache Write"</th>
                        <th class="num">"1h Cache Write"</th>
                        <th class="num">"Cache Hit & Refresh"</th>
                        <th class="num">"Output"</th>
                    </tr>
                </thead>
                <tbody>
                    {move || {
                        let table = pricing.get();
                        table.entries.into_iter().enumerate().map(|(idx, entry)| {
                            let label = entry.label.clone();
                            let deprecated = entry.deprecated;
                            let p = entry.pricing;
                            view! {
                                <tr class:row-deprecated=deprecated>
                                    <td class="label">
                                        <span>{label}</span>
                                        {deprecated.then(|| view! {
                                            <span class="badge badge--muted" style="margin-left: 6px;">"deprecated"</span>
                                        })}
                                    </td>
                                    <td class="num">
                                        <PriceInput value=p.base_input on_change=move |v| {
                                            pricing.update(|t| t.entries[idx].pricing.base_input = v);
                                        } on_commit=commit />
                                    </td>
                                    <td class="num">
                                        <PriceInput value=p.cache_write_5m on_change=move |v| {
                                            pricing.update(|t| t.entries[idx].pricing.cache_write_5m = v);
                                        } on_commit=commit />
                                    </td>
                                    <td class="num">
                                        <PriceInput value=p.cache_write_1h on_change=move |v| {
                                            pricing.update(|t| t.entries[idx].pricing.cache_write_1h = v);
                                        } on_commit=commit />
                                    </td>
                                    <td class="num">
                                        <PriceInput value=p.cache_read on_change=move |v| {
                                            pricing.update(|t| t.entries[idx].pricing.cache_read = v);
                                        } on_commit=commit />
                                    </td>
                                    <td class="num">
                                        <PriceInput value=p.output on_change=move |v| {
                                            pricing.update(|t| t.entries[idx].pricing.output = v);
                                        } on_commit=commit />
                                    </td>
                                </tr>
                            }
                        }).collect_view()
                    }}
                </tbody>
            </table>

            <div class="form-row" style="margin-top: 12px;">
                <button class="btn" on:click=reset_all>"Reset all to defaults"</button>
                {move || status.get().map(|m| view! { <span class="muted">{m}</span> })}
            </div>
        </div>
    }
}

/// Display-currency dropdown + manual refresh of FX rates. Rates are cached
/// in prefs.json and auto-refreshed every 24h on app startup; this lets the
/// user force a refresh now (e.g. after travelling or after a market move).
#[component]
fn CurrencySection() -> impl IntoView {
    let global_currency = use_context::<RwSignal<CurrencyState>>()
        .expect("CurrencyState signal must be provided by App");
    let (status, set_status) = signal::<Option<String>>(None);

    leptos::task::spawn_local(async move {
        if let Ok(c) = invoke_no_args::<CurrencyState>("get_currency_state").await {
            global_currency.set(c);
        }
    });

    let on_change = move |ev: leptos::ev::Event| {
        let code = event_target_value(&ev);
        // Optimistic local update so the UI re-renders immediately.
        global_currency.update(|c| c.active = code.clone());
        leptos::task::spawn_local(async move {
            match invoke::<(), _>("set_active_currency", &SetCurrencyArgs { code: code.clone() }).await {
                Ok(_) => set_status.set(Some(format!("Display currency set to {code}."))),
                Err(e) => set_status.set(Some(format!("Error: {e}"))),
            }
        });
    };

    let refresh = move |_| {
        leptos::task::spawn_local(async move {
            set_status.set(Some("Refreshing rates from frankfurter.app…".into()));
            match invoke_no_args::<CurrencyState>("refresh_currency_rates").await {
                Ok(c) => {
                    // Compose a status line that reports the current rate
                    // for the *active* currency, plus the formatted refresh
                    // time. For USD-active there's no rate to quote.
                    let active = c.active.clone();
                    let when = c
                        .fetched_at
                        .as_ref()
                        .map(|t| format_datetime(t))
                        .unwrap_or_default();
                    let active_rate = c
                        .list
                        .iter()
                        .find(|i| i.code == active)
                        .map(|i| i.rate);
                    let msg = match (active.as_str(), active_rate) {
                        ("USD", _) => format!("Rates refreshed on {when}."),
                        (code, Some(rate)) => {
                            format!("Refreshed: {rate:.4} {code} / USD on {when}.")
                        }
                        _ => format!("Rates refreshed on {when}."),
                    };
                    global_currency.set(c);
                    set_status.set(Some(msg));
                }
                Err(e) => set_status.set(Some(format!("Refresh failed: {e}"))),
            }
        });
    };

    view! {
        <h2 style="margin-top: 24px;">"Display currency"</h2>
        <div class="form">
            <div class="form-row">
                <select
                    class="currency-select"
                    on:change=on_change
                    prop:value=move || global_currency.get().active
                >
                    {move || global_currency.get().list.into_iter().map(|c| {
                        let code = c.code.clone();
                        let label = format!("{} {} — {:.4} per USD", c.symbol, c.code, c.rate);
                        view! { <option value=code>{label}</option> }
                    }).collect_view()}
                </select>
                <button class="btn" on:click=refresh>"Refresh rates"</button>
                {move || status.get().map(|m| view! { <span class="muted small">{m}</span> })}
            </div>
            <small class="muted">
                "Rates from frankfurter.app (ECB-sourced, updates daily). Cached locally; \
                 switching currency is instant. \"Reset all\" in Pricing returns to USD."
                {move || global_currency.get().fetched_at.map(|t| {
                    view! { <span style="margin-left: 6px;">{format!("Last refresh: {}", format_datetime(&t))}</span> }
                })}
            </small>
        </div>
    }
}

#[component]
fn PriceInput(
    value: f64,
    #[prop(into)] on_change: Callback<f64>,
    #[prop(into)] on_commit: Callback<()>,
) -> impl IntoView {
    view! {
        <input
            class="price-input"
            type="number"
            min="0"
            step="0.01"
            prop:value=move || format!("{value:.4}").trim_end_matches('0').trim_end_matches('.').to_string()
            on:input=move |ev| {
                if let Ok(v) = event_target_value(&ev).parse::<f64>() {
                    if v >= 0.0 {
                        on_change.run(v);
                    }
                }
            }
            on:blur=move |_| on_commit.run(())
        />
    }
}

