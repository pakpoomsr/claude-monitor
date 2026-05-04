use leptos::prelude::*;
use serde::Serialize;

use crate::tauri_bridge::{invoke, invoke_no_args};
use crate::types::{format_date_short, format_money, CurrencyState, DailySummary, DayStats};

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
    // Build the target instant in ms then let JS Date normalize it — avoids
    // u32 underflow when stepping back into the previous month.
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
    let (summary, set_summary) = signal::<Option<DailySummary>>(None);
    let (rows, set_rows) = signal::<Vec<DayStats>>(Vec::new());
    let (err, set_err) = signal::<Option<String>>(None);

    let (preset, set_preset) = signal(RangePreset::Last7);
    let (start, set_start) = signal(iso_today_minus(6));
    let (end, set_end) = signal(iso_today_minus(0));

    // Refetch whenever the active range changes. The custom-range inputs
    // also flip the preset to Custom so the pills track reality.
    Effect::new(move |_| {
        let s = start.get();
        let e = end.get();
        leptos::task::spawn_local(async move {
            match invoke::<Vec<DayStats>, _>(
                "get_usage_range",
                &RangeArgs { start_date: s, end_date: e },
            )
            .await
            {
                Ok(r) => set_rows.set(r),
                Err(err_msg) => set_err.set(Some(err_msg)),
            }
        });
    });

    leptos::task::spawn_local(async move {
        if let Ok(s) = invoke_no_args::<DailySummary>("get_daily_summary").await {
            set_summary.set(Some(s));
        }
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

            {move || summary.get().map(|s| {
                let cur = use_context::<RwSignal<CurrencyState>>()
                    .map(|sig| sig.get())
                    .unwrap_or_default();
                view! {
                    <div class="summary-cards">
                        <SummaryCard label="Today input"  value=format_num(s.total_input_tokens) />
                        <SummaryCard label="Today output" value=format_num(s.total_output_tokens) />
                        <SummaryCard label="Today cost"   value=format_money(s.total_cost_usd, &cur) />
                        <SummaryCard label="Sessions"     value=s.session_count.to_string() />
                        <SummaryCard label="Top model"    value=s.top_model.clone() />
                    </div>
                }
            })}

            <div class="chart">
                {move || {
                    let data = rows.get();
                    if data.is_empty() {
                        view! { <p class="muted">"No data in range."</p> }.into_any()
                    } else {
                        view! { <BarChart data /> }.into_any()
                    }
                }}
            </div>
        </section>
    }
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
