use leptos::prelude::*;

use crate::types::{avatar_for, avatar_url, format_money, project_label, short_id, AgentGroup, AgentSnapshot, CurrencyState};

#[component]
pub fn AgentGrid(
    groups: Signal<Vec<AgentGroup>>,
    set_selected: WriteSignal<Option<String>>,
    #[prop(into)] section_label: String,
    #[prop(into)] empty_message: Signal<String>,
) -> impl IntoView {
    let label = StoredValue::new(section_label);

    view! {
        <Show when=move || !groups.with(|g| g.is_empty()) fallback=move || {
            let msg = empty_message.get();
            if msg.is_empty() {
                ().into_any()
            } else {
                view! {
                    <section class="agent-section">
                        <h2 class="section-title">{label.get_value()}</h2>
                        <div class="empty">
                            <div class="empty-sprite"></div>
                            <p class="muted">{msg}</p>
                        </div>
                    </section>
                }.into_any()
            }
        }>
            <section class="agent-section">
                <h2 class="section-title">
                    {move || label.get_value()}
                    <span class="section-count muted">
                        {move || format!(" ({})", groups.with(|g| g.len()))}
                    </span>
                </h2>
                <div class="group-list">
                    <For
                        each=move || groups.get()
                        key=|g| g.parent.session_id.clone()
                        children=move |g: AgentGroup| {
                            let agg = g.aggregate_status();
                            let agg_class = agg.css_class();
                            let child_count = g.children.len();
                            view! {
                                <div class=format!("group group--{agg_class}")>
                                    <AgentTile snap=g.parent set_selected is_child=false />
                                    {(child_count > 0).then(|| view! {
                                        <div class="children">
                                            {g.children.into_iter().map(|child| view! {
                                                <AgentTile snap=child set_selected is_child=true />
                                            }).collect_view()}
                                        </div>
                                    })}
                                </div>
                            }
                        }
                    />
                </div>
            </section>
        </Show>
    }
}

#[component]
fn AgentTile(
    snap: AgentSnapshot,
    set_selected: WriteSignal<Option<String>>,
    is_child: bool,
) -> impl IntoView {
    let id = snap.session_id.clone();
    let status_class = snap.status.css_class();
    let avatar_name = avatar_for(&snap.session_id);
    let avatar_src = avatar_url(avatar_name, 96);
    let label = if is_child {
        format!("↳ sub-agent {}", short_id(&snap.session_id))
    } else {
        project_label(&snap.project)
    };
    let id_short = short_id(&snap.session_id);
    let model = model_short(&snap.model);
    let tool = snap.current_tool.clone();
    let cost_usd = snap.cost_usd;
    let currency_sig = use_context::<RwSignal<CurrencyState>>();
    let cost = move || {
        let cur = currency_sig.map(|s| s.get()).unwrap_or_default();
        format_money(cost_usd, &cur)
    };
    let preview = snap.current_message.clone();
    let status_label = snap.status.label();
    let extra_class = if is_child { " tile--child" } else { "" };

    view! {
        <button
            class=format!("tile tile--{status_class}{extra_class}")
            on:click=move |_| set_selected.set(Some(id.clone()))
            title=preview
        >
            <div class=format!("sprite-frame sprite-frame--{status_class}")>
                <img
                    class=format!("avatar avatar--{status_class}")
                    src=avatar_src
                    alt=avatar_name
                    draggable="false"
                />
            </div>
            <div class="tile-body">
                <div class="tile-row">
                    <span class="tile-name">{label}</span>
                    <span class="tile-cost">{cost}</span>
                </div>
                <div class="tile-row muted small">
                    <span>{id_short}</span>
                    <span>{model}</span>
                </div>
                <div class="tile-status">
                    {match tool {
                        Some(t) => t.to_string(),
                        None => status_label.to_string(),
                    }}
                </div>
            </div>
        </button>
    }
}

fn model_short(m: &str) -> String {
    if m.is_empty() {
        return "-".into();
    }
    if m.contains("opus") { "opus".into() }
    else if m.contains("sonnet") { "sonnet".into() }
    else if m.contains("haiku") { "haiku".into() }
    else { m.split('-').next().unwrap_or(m).to_string() }
}
