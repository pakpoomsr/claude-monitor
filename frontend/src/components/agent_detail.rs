use leptos::prelude::*;
use serde::Serialize;

use crate::tauri_bridge::invoke;
use crate::types::{
    avatar_for, avatar_url, format_log_time, format_money, project_label, short_id, AgentSnapshot,
    CurrencyState, LogEntry, ModelPricing, PricingTable,
};
use crate::EventLogMap;

#[derive(Serialize)]
struct GetEventsArgs {
    #[serde(rename = "sessionId")]
    session_id: String,
    limit: Option<usize>,
}

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

    let log_sig = use_context::<RwSignal<EventLogMap>>();

    // Backfill the event log when an agent is first selected. Only fires
    // when the bucket is empty — repeat selections of an already-streamed
    // agent skip the network round-trip. We read the log untracked so this
    // effect only re-runs on `selected` changes, not on every new event.
    Effect::new(move |_| {
        let Some(id) = selected.get() else { return };
        let bucket_empty = log_sig
            .map(|s| {
                s.with_untracked(|m| m.get(&id).map(|q| q.is_empty()).unwrap_or(true))
            })
            .unwrap_or(true);
        if !bucket_empty {
            return;
        }
        let target_id = id.clone();
        leptos::task::spawn_local(async move {
            let args = GetEventsArgs {
                session_id: target_id.clone(),
                limit: Some(200),
            };
            if let Ok(entries) = invoke::<Vec<LogEntry>, _>("get_agent_events", &args).await
                && let Some(sig) = log_sig
            {
                sig.update(|m| {
                    let q = m.entry(target_id).or_default();
                    // Don't clobber entries that streamed in between selection
                    // and the backfill response landing.
                    if q.is_empty() {
                        for entry in entries {
                            q.push_back(entry);
                        }
                    }
                });
            }
        });
    });

    let entries_for_selected = move || -> Vec<LogEntry> {
        let Some(id) = selected.get() else { return Vec::new() };
        log_sig
            .map(|s| {
                s.with(|m| {
                    m.get(&id)
                        .map(|q| q.iter().cloned().collect::<Vec<_>>())
                        .unwrap_or_default()
                })
            })
            .unwrap_or_default()
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
                                <h3>"Recent message"</h3>
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
                                <div class="token-meter">
                                    <IoMeter input=s.input_tokens output=s.output_tokens />
                                    <CacheMeter input=s.input_tokens cache=s.cache_tokens />
                                </div>
                                <CostTable
                                    input=s.input_tokens
                                    output=s.output_tokens
                                    cache_write_5m=s.cache_write_5m_tokens
                                    cache_write_1h=s.cache_write_1h_tokens
                                    cache_read=s.cache_read_tokens
                                    model=s.model.clone()
                                />
                                <div class="kv-grid">
                                    <span>"Model"</span><span>{s.model.clone()}</span>
                                </div>
                            </section>

                            <section class="detail-section">
                                <h3>"Project"</h3>
                                <code class="path">{
                                    if s.project.is_empty() { "(unknown)".to_string() } else { s.project.clone() }
                                }</code>
                            </section>

                            <section class="detail-section">
                                <h3>"Recent events"</h3>
                                <div class="event-log">
                                    {move || {
                                        let entries = entries_for_selected();
                                        if entries.is_empty() {
                                            view! {
                                                <div class="event-log-empty muted">
                                                    "No events captured yet."
                                                </div>
                                            }.into_any()
                                        } else {
                                            // Newest at the top — reverse so the most recent
                                            // entry is the first thing you read.
                                            let rows: Vec<LogEntry> = entries.into_iter().rev().collect();
                                            view! {
                                                <For
                                                    each=move || rows.clone()
                                                    key=|e: &LogEntry| format!("{}|{}|{}", e.timestamp, e.kind, e.summary)
                                                    let:e
                                                >
                                                    <div class=format!("event-row event-row--{}", e.source.css_class())>
                                                        <time class="event-time">{format_log_time(&e.timestamp)}</time>
                                                        <span class="event-kind">{e.kind.clone()}</span>
                                                        <span class="event-summary">{e.summary.clone()}</span>
                                                    </div>
                                                </For>
                                            }.into_any()
                                        }
                                    }}
                                </div>
                            </section>
                        </div>
                    }.into_any()
                }
            }}
        </aside>
    }
}

/// Input vs Output proportions, normalized to (input + output) only.
/// Cache lives in its own bar — see [`CacheMeter`] — because cache reads
/// dominate total volume and would otherwise hide I/O entirely.
#[component]
fn IoMeter(input: u64, output: u64) -> impl IntoView {
    let total = (input + output).max(1);
    let in_pct = (input as f64 / total as f64) * 100.0;
    let out_pct = (output as f64 / total as f64) * 100.0;

    view! {
        <div class="token-meter-row">
            <div class="token-meter-caption">
                <span>
                    "I/O"
                    <button class="info-icon" type="button"
                            title="Input vs Output ratio">"i"</button>
                </span>
                <span class="token-meter-pct">
                    {format!("{} in · {} out", format_num(input), format_num(output))}
                </span>
            </div>
            <div class="info-tip" role="tooltip">
                "Proportion of fresh input tokens vs generated output tokens for this session. \
                 Cache reads are excluded so the bar stays meaningful — they live in the Cache hit gauge below."
            </div>
            <div class="token-meter-bar"
                 title=format!("Input {} · Output {}", format_num(input), format_num(output))>
                <div class="token-meter-seg token-meter-seg--input"  style=format!("width: {:.2}%;", in_pct) />
                <div class="token-meter-seg token-meter-seg--output" style=format!("width: {:.2}%;", out_pct) />
            </div>
        </div>
    }
}

/// Cache hit rate: cache reads / (cache reads + fresh input). Higher = cheaper.
/// Tints green once we cross 70%, which is roughly where Claude Code starts
/// looking healthy on a long session.
#[component]
fn CacheMeter(input: u64, cache: u64) -> impl IntoView {
    let denom = input + cache;
    let pct = if denom == 0 { 0.0 } else { (cache as f64 / denom as f64) * 100.0 };
    let good = pct >= 70.0;
    let fill_class = if good { "token-meter-fill token-meter-fill--cache is-good" }
                     else    { "token-meter-fill token-meter-fill--cache" };
    let pct_class = if good { "token-meter-pct token-meter-pct--good" }
                    else    { "token-meter-pct" };

    view! {
        <div class="token-meter-row">
            <div class="token-meter-caption">
                <span>
                    "Cache hit"
                    <button class="info-icon" type="button"
                            title="Prompt cache efficiency">"i"</button>
                </span>
                <span class=pct_class>{format!("{:.0}%", pct)}</span>
            </div>
            <div class="info-tip" role="tooltip">
                "Share of input served from Anthropic's prompt cache: cache_reads / (cache_reads + fresh_input). \
                 Cached reads cost ~10% of fresh input, so higher is cheaper. \
                 Turns green at 70%+, which is healthy for long sessions."
            </div>
            <div class="token-meter-bar"
                 title=format!("{} cached / {} fresh input", format_num(cache), format_num(input))>
                <div class=fill_class style=format!("width: {:.2}%;", pct) />
            </div>
        </div>
    }
}

fn format_rate(rate_per_m_usd: f64, currency: &CurrencyState) -> String {
    let (symbol, fx) = currency.active_rate();
    let v = rate_per_m_usd * fx;
    if v < 1.0 {
        format!("{symbol}{v:.3}/M")
    } else {
        format!("{symbol}{v:.2}/M")
    }
}

#[component]
fn CostTable(
    input: u64,
    output: u64,
    cache_write_5m: u64,
    cache_write_1h: u64,
    cache_read: u64,
    model: String,
) -> impl IntoView {
    // Read live pricing + currency from context.
    let pricing_sig = use_context::<RwSignal<PricingTable>>();
    let currency_sig = use_context::<RwSignal<CurrencyState>>();
    let model = StoredValue::new(model);

    let row = move || -> (ModelPricing, u64, CurrencyState) {
        let p = pricing_sig
            .map(|s| s.get().pricing_for(&model.get_value()))
            .unwrap_or(ModelPricing {
                base_input: 3.0,
                cache_write_5m: 3.75,
                cache_write_1h: 6.0,
                cache_read: 0.30,
                output: 15.0,
            });
        let cur = currency_sig.map(|s| s.get()).unwrap_or_default();
        let total_tokens = input + cache_write_5m + cache_write_1h + cache_read + output;
        (p, total_tokens, cur)
    };

    view! {
        {move || {
            let (p, total_tokens, cur) = row();
            let m = 1_000_000.0;
            let c_in   = (input as f64           / m) * p.base_input;
            let c_5m   = (cache_write_5m as f64  / m) * p.cache_write_5m;
            let c_1h   = (cache_write_1h as f64  / m) * p.cache_write_1h;
            let c_hit  = (cache_read as f64      / m) * p.cache_read;
            let c_out  = (output as f64          / m) * p.output;
            let total  = c_in + c_5m + c_1h + c_hit + c_out;
            view! {
                <table class="cost-table">
                    <thead>
                        <tr>
                            <th>"Type"</th>
                            <th class="num">"Tokens"</th>
                            <th class="num">"Rate"</th>
                            <th class="num">"Cost"</th>
                        </tr>
                    </thead>
                    <tbody>
                        <tr>
                            <td class="label"><i class="swatch--input" />"Base Input"</td>
                            <td class="num">{format_num(input)}</td>
                            <td class="num rate-col">{format_rate(p.base_input, &cur)}</td>
                            <td class="num">{format_money(c_in, &cur)}</td>
                        </tr>
                        <tr>
                            <td class="label"><i class="swatch--cache" />"5m Cache Writes"</td>
                            <td class="num">{format_num(cache_write_5m)}</td>
                            <td class="num rate-col">{format_rate(p.cache_write_5m, &cur)}</td>
                            <td class="num">{format_money(c_5m, &cur)}</td>
                        </tr>
                        <tr>
                            <td class="label"><i class="swatch--cache" />"1h Cache Writes"</td>
                            <td class="num">{format_num(cache_write_1h)}</td>
                            <td class="num rate-col">{format_rate(p.cache_write_1h, &cur)}</td>
                            <td class="num">{format_money(c_1h, &cur)}</td>
                        </tr>
                        <tr>
                            <td class="label"><i class="swatch--cache" />"Cache Hits & Refresh"</td>
                            <td class="num">{format_num(cache_read)}</td>
                            <td class="num rate-col">{format_rate(p.cache_read, &cur)}</td>
                            <td class="num">{format_money(c_hit, &cur)}</td>
                        </tr>
                        <tr>
                            <td class="label"><i class="swatch--output" />"Output"</td>
                            <td class="num">{format_num(output)}</td>
                            <td class="num rate-col">{format_rate(p.output, &cur)}</td>
                            <td class="num">{format_money(c_out, &cur)}</td>
                        </tr>
                    </tbody>
                    <tfoot>
                        <tr>
                            <td>"Total"</td>
                            <td class="num">{format_num(total_tokens)}</td>
                            <td class="num"></td>
                            <td class="num">{format_money(total, &cur)}</td>
                        </tr>
                    </tfoot>
                </table>
            }
        }}
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
