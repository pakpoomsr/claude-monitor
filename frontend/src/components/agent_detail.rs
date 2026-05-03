use leptos::prelude::*;

use crate::types::{avatar_for, avatar_url, project_label, short_id, AgentSnapshot};

#[component]
pub fn AgentDetail(
    agents: ReadSignal<Vec<AgentSnapshot>>,
    selected: ReadSignal<Option<String>>,
    set_selected: WriteSignal<Option<String>>,
) -> impl IntoView {
    let snap = move || {
        let id = selected.get()?;
        agents.with(|list| list.iter().find(|a| a.session_id == id).cloned())
    };

    view! {
        <aside class="detail">
            {move || match snap() {
                None => view! {
                    <div class="detail-empty muted">"Agent no longer in the list."</div>
                }.into_any(),
                Some(s) => {
                    let cls = s.status.css_class();
                    let avatar_name = avatar_for(&s.session_id);
                    let avatar_src = avatar_url(avatar_name, 128);
                    view! {
                        <div class=format!("detail-card detail-card--{cls}")>
                            <header class="detail-header">
                                <div class=format!("sprite-frame sprite-frame--lg sprite-frame--{cls}")>
                                    <img
                                        class=format!("avatar avatar--{cls}")
                                        src=avatar_src
                                        alt=avatar_name
                                        draggable="false"
                                    />
                                </div>
                                <div>
                                    <h2 class="detail-title">{project_label(&s.project)}</h2>
                                    <div class="detail-sub muted">{short_id(&s.session_id)}</div>
                                </div>
                                <span class=format!("badge badge--{cls}")>{s.status.label()}</span>
                                <button
                                    class="detail-close"
                                    title="Close"
                                    on:click=move |_| set_selected.set(None)
                                >"×"</button>
                            </header>

                            <section class="detail-section">
                                <h3>"Current message"</h3>
                                <pre class="message">{
                                    if s.current_message.is_empty() {
                                        "(no message yet)".to_string()
                                    } else {
                                        s.current_message.clone()
                                    }
                                }</pre>
                            </section>

                            {s.current_tool.clone().map(|tool| view! {
                                <section class="detail-section">
                                    <h3>"In-flight tool"</h3>
                                    <code class="tool">{tool}</code>
                                </section>
                            })}

                            <section class="detail-section">
                                <h3>"Tokens"</h3>
                                <div class="kv-grid">
                                    <span class="muted">"Input"</span><span>{format_num(s.input_tokens)}</span>
                                    <span class="muted">"Output"</span><span>{format_num(s.output_tokens)}</span>
                                    <span class="muted">"Cache"</span><span>{format_num(s.cache_tokens)}</span>
                                    <span class="muted">"Cost"</span><span>{format!("${:.4}", s.cost_usd)}</span>
                                    <span class="muted">"Model"</span><span>{s.model.clone()}</span>
                                </div>
                            </section>

                            <section class="detail-section">
                                <h3>"Project"</h3>
                                <code class="path">{
                                    if s.project.is_empty() { "(unknown)".to_string() } else { s.project.clone() }
                                }</code>
                            </section>
                        </div>
                    }.into_any()
                }
            }}
        </aside>
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
