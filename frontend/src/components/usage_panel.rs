use leptos::prelude::*;

use crate::tauri_bridge::invoke_no_args;
use crate::types::{DailySummary, DayStats};

#[component]
pub fn UsagePanel() -> impl IntoView {
    let (summary, set_summary) = signal::<Option<DailySummary>>(None);
    let (week, set_week) = signal::<Vec<DayStats>>(Vec::new());
    let (err, set_err) = signal::<Option<String>>(None);

    leptos::task::spawn_local(async move {
        match invoke_no_args::<DailySummary>("get_daily_summary").await {
            Ok(s) => set_summary.set(Some(s)),
            Err(e) => set_err.set(Some(e)),
        }
        match invoke_no_args::<Vec<DayStats>>("get_weekly_chart").await {
            Ok(s) => set_week.set(s),
            Err(e) => set_err.set(Some(e)),
        }
    });

    view! {
        <section class="panel">
            <h2>"Local usage (last 7 days)"</h2>

            {move || err.get().map(|e| view! {
                <div class="error-box">{e}</div>
            })}

            {move || summary.get().map(|s| view! {
                <div class="summary-cards">
                    <SummaryCard label="Today input"  value=format_num(s.total_input_tokens) />
                    <SummaryCard label="Today output" value=format_num(s.total_output_tokens) />
                    <SummaryCard label="Today cost"   value=format!("${:.2}", s.total_cost_usd) />
                    <SummaryCard label="Sessions"     value=s.session_count.to_string() />
                    <SummaryCard label="Top model"    value=s.top_model.clone() />
                </div>
            })}

            <div class="chart">
                {move || {
                    let data = week.get();
                    if data.is_empty() {
                        view! { <p class="muted">"No data yet."</p> }.into_any()
                    } else {
                        view! { <BarChart data /> }.into_any()
                    }
                }}
            </div>
        </section>
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

    let bars = data
        .into_iter()
        .map(|d| {
            let h_pct = ((d.cost_usd / max_cost) * 100.0).clamp(2.0, 100.0);
            let label = d.date.split('-').last().unwrap_or(&d.date).to_string();
            let cost = format!("${:.2}", d.cost_usd);
            view! {
                <div class="bar-col" title=cost.clone()>
                    <div class="bar" style=format!("height: {h_pct:.1}%")></div>
                    <div class="bar-label muted">{label}</div>
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
