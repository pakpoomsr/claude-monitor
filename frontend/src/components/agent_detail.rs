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
                                    cache=s.cache_tokens
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

/// Per-million-token pricing in USD. Mirrors the backend's
/// `agents::estimate_cost` exactly so per-row costs sum to `cost_usd`.
fn model_pricing(model: &str) -> (f64, f64, f64) {
    let m = model.to_lowercase();
    if m.contains("opus") {
        (15.0, 75.0, 1.875)
    } else if m.contains("sonnet") {
        (3.0, 15.0, 0.375)
    } else {
        (0.80, 4.0, 0.10)
    }
}

fn format_rate(rate_per_m: f64) -> String {
    if rate_per_m < 1.0 {
        format!("${:.3}/M", rate_per_m)
    } else {
        format!("${:.2}/M", rate_per_m)
    }
}

fn format_cost(cost: f64) -> String {
    if cost >= 1.0       { format!("${:.2}", cost) }
    else if cost >= 0.01 { format!("${:.3}", cost) }
    else                 { format!("${:.4}", cost) }
}

#[component]
fn CostTable(input: u64, output: u64, cache: u64, model: String) -> impl IntoView {
    let (rin, rout, rcache) = model_pricing(&model);
    let m = 1_000_000.0;
    let cin    = (input as f64  / m) * rin;
    let cout   = (output as f64 / m) * rout;
    let ccache = (cache as f64  / m) * rcache;
    let total  = cin + cout + ccache;

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
                    <td class="label"><i class="swatch--input" />"Input"</td>
                    <td class="num">{format_num(input)}</td>
                    <td class="num rate-col">{format_rate(rin)}</td>
                    <td class="num">{format_cost(cin)}</td>
                </tr>
                <tr>
                    <td class="label"><i class="swatch--output" />"Output"</td>
                    <td class="num">{format_num(output)}</td>
                    <td class="num rate-col">{format_rate(rout)}</td>
                    <td class="num">{format_cost(cout)}</td>
                </tr>
                <tr>
                    <td class="label"><i class="swatch--cache" />"Cache"</td>
                    <td class="num">{format_num(cache)}</td>
                    <td class="num rate-col">{format_rate(rcache)}</td>
                    <td class="num">{format_cost(ccache)}</td>
                </tr>
            </tbody>
            <tfoot>
                <tr>
                    <td>"Total"</td>
                    <td class="num">{format_num(input + output + cache)}</td>
                    <td class="num"></td>
                    <td class="num">{format_cost(total)}</td>
                </tr>
            </tfoot>
        </table>
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
