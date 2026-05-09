//! Renders a backend-computed unified-diff string as colored +/- lines.
//! No diff math here — `snapshots::diff` already produced the unified text;
//! this component just splits on '\n' and applies a class per line.

use leptos::prelude::*;

use crate::types::DiffResult;

#[component]
pub fn DiffView(diff: ReadSignal<Option<DiffResult>>) -> impl IntoView {
    view! {
        <div class="diff-view">
            {move || match diff.get() {
                None => view! {
                    <div class="diff-empty muted">"Select an edit to view its diff."</div>
                }.into_any(),
                Some(d) if d.is_binary => view! {
                    <div class="diff-empty muted">{d.unified.clone()}</div>
                }.into_any(),
                Some(d) => {
                    let plus = d.plus;
                    let minus = d.minus;
                    let unified = d.unified.clone();
                    let lines: Vec<(usize, String, &'static str)> = unified
                        .split_inclusive('\n')
                        .enumerate()
                        .map(|(i, line)| {
                            let cls = match line.chars().next() {
                                Some('+') => "diff-line diff-line--add",
                                Some('-') => "diff-line diff-line--del",
                                _ => "diff-line diff-line--ctx",
                            };
                            (i, line.trim_end_matches('\n').to_string(), cls)
                        })
                        .collect();
                    let oversized = d.pre_oversized || d.post_oversized;
                    view! {
                        <div class="diff-meta">
                            <span class="diff-stat diff-stat--plus">{format!("+{plus}")}</span>
                            <span class="diff-stat diff-stat--minus">{format!("-{minus}")}</span>
                        </div>
                        {oversized.then(|| view! {
                            <div class="diff-warning muted">
                                "One side of the diff was over 1 MB and was not snapshotted."
                            </div>
                        })}
                        <pre class="diff-body">
                            <For
                                each=move || lines.clone()
                                key=|item| item.0
                                let:item
                            >
                                <div class=item.2>{item.1.clone()}</div>
                            </For>
                        </pre>
                    }.into_any()
                }
            }}
        </div>
    }
}
