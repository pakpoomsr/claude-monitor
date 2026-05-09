//! History tab: lists per-session file edits captured via hooks, with a diff
//! viewer and one-click restore. Restore captures a `pre-restore` snapshot of
//! the current file state first so the operation is reversible.

use leptos::prelude::*;
use serde::Serialize;
use std::collections::BTreeMap;

use crate::components::diff_view::DiffView;
use crate::tauri_bridge::{invoke, invoke_no_args, listen};
use crate::types::{
    format_log_time, project_label, short_id, DiffResult, HooksStatus, RestoreResult, SnapshotRow,
};

#[derive(Serialize)]
struct SnapshotIdArgs {
    #[serde(rename = "snapshotId")]
    snapshot_id: i64,
}

#[derive(Serialize)]
struct SessionIdArgs {
    #[serde(rename = "sessionId")]
    session_id: String,
}

#[component]
pub fn HistoryPanel() -> impl IntoView {
    let (snapshots, set_snapshots) = signal::<Vec<SnapshotRow>>(Vec::new());
    let (selected_id, set_selected_id) = signal::<Option<i64>>(None);
    let (diff, set_diff) = signal::<Option<DiffResult>>(None);
    let (status, set_status) = signal::<Option<String>>(None);
    let (hooks, set_hooks) = signal(HooksStatus::default());
    let (expanded, set_expanded) = signal::<Option<String>>(None);

    // Refresh trigger — bumped by restore + by the snapshot-restored event so
    // the list reloads.
    let (refresh_tick, set_refresh_tick) = signal(0u32);

    listen::<serde_json::Value, _>("snapshot-restored", move |_| {
        set_refresh_tick.update(|n| *n = n.wrapping_add(1));
    });

    Effect::new(move |_| {
        let _ = refresh_tick.get();
        leptos::task::spawn_local(async move {
            if let Ok(rows) = invoke_no_args::<Vec<SnapshotRow>>("list_recent_snapshots").await {
                set_snapshots.set(rows);
            }
            if let Ok(h) = invoke_no_args::<HooksStatus>("hooks_status").await {
                set_hooks.set(h);
            }
        });
    });

    // Load diff when selection changes.
    Effect::new(move |_| {
        let id = match selected_id.get() {
            Some(id) => id,
            None => {
                set_diff.set(None);
                return;
            }
        };
        leptos::task::spawn_local(async move {
            match invoke::<DiffResult, _>("get_snapshot_diff", &SnapshotIdArgs { snapshot_id: id })
                .await
            {
                Ok(d) => set_diff.set(Some(d)),
                Err(e) => {
                    set_diff.set(None);
                    set_status.set(Some(format!("Diff error: {e}")));
                }
            }
        });
    });

    let restore = move |snapshot_id: i64| {
        leptos::task::spawn_local(async move {
            match invoke::<RestoreResult, _>(
                "restore_snapshot",
                &SnapshotIdArgs { snapshot_id },
            )
            .await
            {
                Ok(r) => {
                    let msg = if r.deleted_target {
                        "Restored — file deleted (it didn't exist before the edit).".to_string()
                    } else {
                        "Restored. A pre-restore snapshot was captured so you can undo.".to_string()
                    };
                    set_status.set(Some(msg));
                    set_refresh_tick.update(|n| *n = n.wrapping_add(1));
                }
                Err(e) => set_status.set(Some(format!("Restore failed: {e}"))),
            }
        });
    };

    let purge_session = move |session_id: String| {
        leptos::task::spawn_local(async move {
            match invoke::<usize, _>("purge_session_snapshots", &SessionIdArgs { session_id })
                .await
            {
                Ok(n) => {
                    set_status.set(Some(format!("Purged {n} snapshot(s) for that session.")));
                    set_refresh_tick.update(|n| *n = n.wrapping_add(1));
                }
                Err(e) => set_status.set(Some(format!("Purge failed: {e}"))),
            }
        });
    };

    view! {
        <section class="panel history-panel">
            <div class="usage-header">
                <h2>"History — Claude file edits"</h2>
                <span class="muted small">
                    "Captures " <code>"Edit"</code> ", " <code>"Write"</code> ", "
                    <code>"MultiEdit"</code> ", " <code>"NotebookEdit"</code>
                    " calls. Hook-driven."
                </span>
            </div>

            {move || {
                if !hooks.get().registered {
                    Some(view! {
                        <div class="banner banner--warn">
                            "Real-time hooks aren't registered, so new edits won't be captured. "
                            "Enable them on the Settings tab."
                        </div>
                    })
                } else { None }
            }}

            {move || status.get().map(|m| view! {
                <div class="banner banner--info">{m}</div>
            })}

            <div class="history-layout">
                <div class="history-sessions">
                    {move || {
                        let rows = snapshots.get();
                        if rows.is_empty() {
                            return view! {
                                <div class="muted" style="padding: 12px;">
                                    "No snapshots yet. Use Claude Code to edit a file with hooks enabled."
                                </div>
                            }.into_any();
                        }
                        // Group by session, preserving newest-first order.
                        let mut by_session: BTreeMap<String, Vec<SnapshotRow>> = BTreeMap::new();
                        let mut session_order: Vec<String> = Vec::new();
                        for r in rows {
                            if !by_session.contains_key(&r.session_id) {
                                session_order.push(r.session_id.clone());
                            }
                            by_session.entry(r.session_id.clone()).or_default().push(r);
                        }
                        view! {
                            <For
                                each=move || session_order.clone()
                                key=|sid| sid.clone()
                                let:sid
                            >
                                {
                                    let session_id = sid.clone();
                                    let group = by_session.get(&sid).cloned().unwrap_or_default();
                                    let project = group.first()
                                        .map(|r| project_label(&r.project_path))
                                        .unwrap_or_else(|| "(unknown)".into());
                                    let edit_count = group.iter().filter(|r| r.phase == "post").count();
                                    let session_short = short_id(&session_id);
                                    let session_id_for_toggle = session_id.clone();
                                    let session_id_for_purge = session_id.clone();
                                    let session_id_for_check = session_id.clone();
                                    let group_for_render = group.clone();
                                    view! {
                                        <div class="history-session">
                                            <button
                                                class="history-session-header"
                                                on:click=move |_| {
                                                    let id = session_id_for_toggle.clone();
                                                    set_expanded.update(|e| {
                                                        if e.as_deref() == Some(&id) {
                                                            *e = None;
                                                        } else {
                                                            *e = Some(id);
                                                        }
                                                    });
                                                }
                                            >
                                                <span class="history-session-project">{project}</span>
                                                <span class="muted small">{session_short}</span>
                                                <span class="history-session-count">{format!("{edit_count} edits")}</span>
                                                <button
                                                    class="btn btn-small"
                                                    title="Delete all snapshots for this session"
                                                    on:click=move |ev| {
                                                        ev.stop_propagation();
                                                        let id = session_id_for_purge.clone();
                                                        purge_session(id);
                                                    }
                                                >"Purge"</button>
                                            </button>
                                            {move || {
                                                let is_open = expanded.get().as_deref()
                                                    == Some(session_id_for_check.as_str());
                                                if !is_open { return None; }
                                                let group = group_for_render.clone();
                                                Some(view! {
                                                    <div class="history-edits">
                                                        <For
                                                            each=move || group.clone()
                                                            key=|r: &SnapshotRow| r.id
                                                            let:row
                                                        >
                                                            <EditRow
                                                                row
                                                                selected_id
                                                                set_selected_id
                                                                on_restore=Callback::new(move |id: i64| restore(id))
                                                            />
                                                        </For>
                                                    </div>
                                                })
                                            }}
                                        </div>
                                    }
                                }
                            </For>
                        }.into_any()
                    }}
                </div>

                <div class="history-diff">
                    <DiffView diff />
                </div>
            </div>
        </section>
    }
}

#[component]
fn EditRow(
    row: SnapshotRow,
    selected_id: ReadSignal<Option<i64>>,
    set_selected_id: WriteSignal<Option<i64>>,
    #[prop(into)] on_restore: Callback<i64>,
) -> impl IntoView {
    let row_for_select = row.clone();
    let row_for_restore = row.clone();
    let id = row.id;
    let pre_id = if row.phase == "post" { row.paired_id } else { Some(row.id) };
    // Only show one row per pair — render the `post` row, restore the `pre`.
    if row.phase != "post" {
        // Render `pre`-only entries (no matching post yet) and `pre-restore` rows.
        let phase_class = if row.phase == "pre-restore" {
            "edit-row edit-row--restore"
        } else {
            "edit-row edit-row--unpaired"
        };
        let summary_label = if row.phase == "pre-restore" {
            "restore: ".to_string()
        } else {
            "pending: ".to_string()
        };
        let file_basename = std::path::Path::new(&row.file_path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| row.file_path.clone());
        return view! {
            <button
                class=phase_class
                title=row.file_path.clone()
                on:click=move |_| set_selected_id.set(Some(id))
                class:active=move || selected_id.get() == Some(id)
            >
                <time class="event-time">{format_log_time(&row.ts)}</time>
                <span class="event-kind">{row.tool_name.clone()}</span>
                <span class="event-summary">{format!("{summary_label}{file_basename}")}</span>
            </button>
        }.into_any();
    }
    let file_basename = std::path::Path::new(&row.file_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| row.file_path.clone());
    let oversized_marker = if row.oversized {
        Some(view! { <span class="badge badge--muted" style="margin-left:4px;">"oversized"</span> })
    } else {
        None
    };
    view! {
        <div class="edit-row-wrapper">
            <button
                class="edit-row"
                title=row_for_select.file_path.clone()
                on:click=move |_| set_selected_id.set(Some(id))
                class:active=move || selected_id.get() == Some(id)
            >
                <time class="event-time">{format_log_time(&row_for_select.ts)}</time>
                <span class="event-kind">{row_for_select.tool_name.clone()}</span>
                <span class="event-summary">{file_basename}</span>
                {oversized_marker}
            </button>
            {pre_id.map(|pid| {
                let _ = row_for_restore;
                view! {
                    <button
                        class="btn btn-small btn-restore"
                        title="Revert to the file's state before this edit"
                        on:click=move |ev| {
                            ev.stop_propagation();
                            on_restore.run(pid);
                        }
                    >"Revert"</button>
                }
            })}
        </div>
    }.into_any()
}
