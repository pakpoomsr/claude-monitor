use leptos::prelude::*;

use crate::types::{project_label, short_id, AgentSnapshot};

#[component]
pub fn AgentGrid(
    agents: Signal<Vec<AgentSnapshot>>,
    set_selected: WriteSignal<Option<String>>,
    #[prop(into)] empty_message: String,
) -> impl IntoView {
    let empty_msg = StoredValue::new(empty_message);

    view! {
        <section class="agent-grid">
            <Show
                when=move || !agents.with(|a| a.is_empty())
                fallback=move || view! {
                    <div class="empty">
                        <div class="empty-sprite"></div>
                        <p class="muted">{empty_msg.get_value()}</p>
                    </div>
                }
            >
                <div class="grid">
                    <For
                        each=move || agents.get()
                        key=|a| a.session_id.clone()
                        children=move |snap: AgentSnapshot| {
                            let id = snap.session_id.clone();
                            let status_class = snap.status.css_class();
                            let label = project_label(&snap.project);
                            let id_short = short_id(&snap.session_id);
                            let model = model_short(&snap.model);
                            let tool = snap.current_tool.clone();
                            let cost = format!("${:.3}", snap.cost_usd);
                            let preview = snap.current_message.clone();
                            let status_label = snap.status.label();
                            view! {
                                <button
                                    class=format!("tile tile--{status_class}")
                                    on:click=move |_| set_selected.set(Some(id.clone()))
                                    title=preview
                                >
                                    <div class="sprite-frame">
                                        <div class=format!("sprite sprite--{status_class}")></div>
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
                                                Some(t) => format!("{t}"),
                                                None => status_label.to_string(),
                                            }}
                                        </div>
                                    </div>
                                </button>
                            }
                        }
                    />
                </div>
            </Show>
        </section>
    }
}

fn model_short(m: &str) -> String {
    if m.is_empty() {
        return "-".into();
    }
    if m.contains("opus") {
        "opus".into()
    } else if m.contains("sonnet") {
        "sonnet".into()
    } else if m.contains("haiku") {
        "haiku".into()
    } else {
        m.split('-').next().unwrap_or(m).to_string()
    }
}
