use leptos::prelude::*;
use serde::Serialize;

use crate::tauri_bridge::invoke;
use crate::types::{
    format_date_short, format_money, BreakdownRow, CurrencyState, DayStats, UsageBreakdown,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum RangePreset {
    Last7,
    Last30,
    Custom,
}

#[derive(Serialize)]
struct RangeArgs {
    #[serde(rename = "startDate")]
    start_date: String,
    #[serde(rename = "endDate")]
    end_date: String,
}

/// Compute YYYY-MM-DD strings for `today - n_days_ago` and `today`.
/// Pure JS-side date math via `js_sys::Date` so we don't pull in chrono.
fn iso_today_minus(days_ago: i32) -> String {
    let ms_per_day = 86_400_000.0_f64;
    let target_ms = js_sys::Date::now() - (days_ago as f64) * ms_per_day;
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(target_ms));
    let y = d.get_full_year();
    let m = d.get_month() + 1;
    let day = d.get_date();
    format!("{y:04}-{m:02}-{day:02}")
}

#[component]
pub fn UsagePanel() -> impl IntoView {
    let (breakdown, set_breakdown) = signal(UsageBreakdown::default());
    let (err, set_err) = signal::<Option<String>>(None);

    let (preset, set_preset) = signal(RangePreset::Last7);
    let (start, set_start) = signal(iso_today_minus(6));
    let (end, set_end) = signal(iso_today_minus(0));

    // One Tauri call per range change — gets totals + per-day chart + every
    // breakdown in a single round-trip. Replaces the old daily-summary +
    // get_usage_range pair.
    Effect::new(move |_| {
        let s = start.get();
        let e = end.get();
        leptos::task::spawn_local(async move {
            match invoke::<UsageBreakdown, _>(
                "get_usage_breakdown",
                &RangeArgs { start_date: s, end_date: e },
            )
            .await
            {
                Ok(b) => set_breakdown.set(b),
                Err(err_msg) => set_err.set(Some(err_msg)),
            }
        });
    });

    let pick_preset = move |p: RangePreset| {
        set_preset.set(p);
        match p {
            RangePreset::Last7 => {
                set_start.set(iso_today_minus(6));
                set_end.set(iso_today_minus(0));
            }
            RangePreset::Last30 => {
                set_start.set(iso_today_minus(29));
                set_end.set(iso_today_minus(0));
            }
            RangePreset::Custom => {
                // Keep current dates — the date inputs become the source.
            }
        }
    };

    view! {
        <section class="panel">
            <div class="usage-header">
                <h2>"Local usage"</h2>
                <div class="range-controls">
                    <div class="range-pills">
                        <RangePill label="Last 7 days"  this=RangePreset::Last7  current=preset on_pick=pick_preset />
                        <RangePill label="Last 30 days" this=RangePreset::Last30 current=preset on_pick=pick_preset />
                        <RangePill label="Custom"       this=RangePreset::Custom current=preset on_pick=pick_preset />
                    </div>
                    <div class="range-dates" class:hidden=move || preset.get() != RangePreset::Custom>
                        <input
                            type="date"
                            class="date-input"
                            prop:value=move || start.get()
                            on:change=move |ev| {
                                set_preset.set(RangePreset::Custom);
                                set_start.set(event_target_value(&ev));
                            }
                        />
                        <span class="muted small">"to"</span>
                        <input
                            type="date"
                            class="date-input"
                            prop:value=move || end.get()
                            on:change=move |ev| {
                                set_preset.set(RangePreset::Custom);
                                set_end.set(event_target_value(&ev));
                            }
                        />
                    </div>
                </div>
            </div>

            {move || err.get().map(|e| view! {
                <div class="error-box">{e}</div>
            })}

            // Range-aware totals row — replaces the old "Today" cards.
            {move || {
                let cur = use_context::<RwSignal<CurrencyState>>()
                    .map(|sig| sig.get())
                    .unwrap_or_default();
                let t = breakdown.get().total;
                view! {
                    <div class="summary-cards">
                        <SummaryCard label="Tokens (in/out)"
                            value=format!("{} / {}", format_num(t.input_tokens), format_num(t.output_tokens)) />
                        <SummaryCard label="Cache tokens" value=format_num(t.cache_tokens) />
                        <SummaryCard label="Cost"         value=format_money(t.cost_usd, &cur) />
                        <SummaryCard label="Sessions"     value=t.session_count.to_string() />
                        <SummaryCard label="Events"       value=format_num(t.event_count) />
                    </div>
                }
            }}

            <div class="chart">
                {move || {
                    let data = breakdown.get().by_day;
                    if data.is_empty() {
                        view! { <p class="muted">"No data in range."</p> }.into_any()
                    } else {
                        view! { <BarChart data /> }.into_any()
                    }
                }}
            </div>

            // Breakdown sections.
            <BreakdownSection
                title="By project"
                rows=Signal::derive(move || breakdown.get().by_project)
                count_label="Sessions"
                cost_kind=CostKind::Real
            />
            <BreakdownSection
                title="By model"
                rows=Signal::derive(move || breakdown.get().by_model)
                count_label="Sessions"
                cost_kind=CostKind::Real
            />
            <BreakdownSection
                title="Core tools"
                rows=Signal::derive(move || breakdown.get().by_tool)
                count_label="Calls"
                cost_kind=CostKind::Approx
            />
            <BreakdownSection
                title="Shell commands"
                rows=Signal::derive(move || breakdown.get().by_shell)
                count_label="Calls"
                cost_kind=CostKind::Approx
            />
            <BreakdownSection
                title="By activity"
                rows=Signal::derive(move || breakdown.get().by_activity)
                count_label="Events"
                cost_kind=CostKind::Approx
            />
        </section>
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CostKind {
    /// Real cost from `sessions.cost_usd` (project / model).
    Real,
    /// Approximate cost split evenly across each session's events
    /// (tools / shell / activity). UI labels it "approx".
    Approx,
}

#[component]
fn RangePill(
    label: &'static str,
    this: RangePreset,
    current: ReadSignal<RangePreset>,
    #[prop(into)] on_pick: Callback<RangePreset>,
) -> impl IntoView {
    let active = move || current.get() == this;
    view! {
        <button
            class="filter-pill"
            class:active=active
            on:click=move |_| on_pick.run(this)
        >
            {label}
        </button>
    }
}

#[component]
fn SummaryCard(label: &'static str, value: String) -> impl IntoView {
    view! {
        <div class="card">
            <div class="card-label muted">{label}</div>
            <div class="card-value">{value}</div>
        </div>
    }
}

const TOP_N: usize = 10;

/// Compact horizontal-bar listing for one breakdown dimension. Top 10 by
/// default with a "show all" toggle when more rows exist.
#[component]
fn BreakdownSection(
    title: &'static str,
    rows: Signal<Vec<BreakdownRow>>,
    count_label: &'static str,
    cost_kind: CostKind,
) -> impl IntoView {
    let (show_all, set_show_all) = signal(false);

    view! {
        <div class="breakdown-section">
            <div class="breakdown-header">
                <h3>{title}</h3>
                <span class="muted small">
                    {match cost_kind {
                        CostKind::Real => "real cost",
                        CostKind::Approx => "approx cost (split per event)",
                    }}
                </span>
            </div>
            {move || {
                let all = rows.get();
                if all.is_empty() {
                    return view! {
                        <div class="muted breakdown-empty">
                            {match title {
                                "Shell commands" => "No bash commands captured in this range. Enable real-time hooks for capture.",
                                _ => "No data in this range.",
                            }}
                        </div>
                    }.into_any();
                }
                let total_count = all.len();
                let truncated = !show_all.get() && total_count > TOP_N;
                let visible: Vec<BreakdownRow> = if truncated {
                    all.into_iter().take(TOP_N).collect()
                } else {
                    all
                };
                let cur = use_context::<RwSignal<CurrencyState>>()
                    .map(|s| s.get())
                    .unwrap_or_default();
                let visible_for_render = visible.clone();
                view! {
                    <div class="breakdown-list">
                        <For
                            each=move || visible_for_render.clone()
                            key=|r: &BreakdownRow| r.name.clone()
                            let:r
                        >
                            <BreakdownRowView row=r count_label cost_kind currency=cur.clone() />
                        </For>
                    </div>
                    {truncated.then(|| view! {
                        <div class="breakdown-more">
                            <button
                                class="btn btn-small"
                                on:click=move |_| set_show_all.set(true)
                            >
                                {format!("Show all {total_count}")}
                            </button>
                        </div>
                    })}
                }.into_any()
            }}
        </div>
    }
}

#[component]
fn BreakdownRowView(
    row: BreakdownRow,
    count_label: &'static str,
    cost_kind: CostKind,
    currency: CurrencyState,
) -> impl IntoView {
    let pct = row.share_pct.clamp(0.0, 100.0);
    let cost_text = format_money(row.cost_usd, &currency);
    let cost_class = match cost_kind {
        CostKind::Real => "breakdown-num breakdown-num--cost",
        CostKind::Approx => "breakdown-num breakdown-num--cost is-approx",
    };
    view! {
        <div class="breakdown-row" title=row.name.clone()>
            <span class="breakdown-name">{row.name.clone()}</span>
            <div class="breakdown-bar">
                <div class="breakdown-fill" style=format!("width: {pct:.1}%")></div>
            </div>
            <span class="breakdown-num">
                {format!("{} {}", row.count, count_label.to_lowercase())}
            </span>
            <span class=cost_class>{cost_text}</span>
        </div>
    }
}

#[component]
fn BarChart(data: Vec<DayStats>) -> impl IntoView {
    let max_cost = data
        .iter()
        .map(|d| d.cost_usd)
        .fold(0.0_f64, f64::max)
        .max(0.01);
    let cur = use_context::<RwSignal<CurrencyState>>()
        .map(|s| s.get())
        .unwrap_or_default();

    let bars = data
        .into_iter()
        .map(|d| {
            let h_pct = ((d.cost_usd / max_cost) * 100.0).clamp(2.0, 100.0);
            let date_label = format_date_short(&d.date);
            let cost_label = format_money(d.cost_usd, &cur);
            view! {
                <div class="bar-col">
                    <div class="bar-value">{cost_label}</div>
                    <div class="bar" style=format!("height: {h_pct:.1}%")></div>
                    <div class="bar-label">{date_label}</div>
                </div>
            }
        })
        .collect_view();

    view! { <div class="bar-chart">{bars}</div> }
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
