use crate::agents::AgentRegistry;
use crate::db::{Database, Session};
use crate::parser::parse_jsonl_line;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

struct FileState {
    offset: u64,
}

pub async fn start_watcher(
    app: AppHandle,
    db: Arc<Mutex<Database>>,
    registry: Arc<AgentRegistry>,
) {
    let claude_dir = match get_claude_projects_dir() {
        Some(d) => d,
        None => {
            eprintln!("[claude-monitor] ~/.claude/projects not found");
            return;
        }
    };

    println!("[claude-monitor] Watching: {}", claude_dir.display());

    let mut file_states: HashMap<PathBuf, FileState> = HashMap::new();

    scan_existing_files(&claude_dir, &mut file_states, &db, &registry, &app).await;

    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<Event>>();
    let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("[claude-monitor] Failed to create watcher: {e}");
            return;
        }
    };
    if let Err(e) = watcher.watch(&claude_dir, RecursiveMode::Recursive) {
        eprintln!("[claude-monitor] Failed to watch: {e}");
        return;
    }

    for res in rx {
        match res {
            Ok(event) => {
                if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                    for path in &event.paths {
                        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                            process_file_update(path, &mut file_states, &db, &registry, &app)
                                .await;
                        }
                    }
                }
            }
            Err(e) => eprintln!("[claude-monitor] Watch error: {e}"),
        }
    }
}

fn get_claude_projects_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let path = home.join(".claude").join("projects");
    if path.exists() { Some(path) } else { None }
}

async fn scan_existing_files(
    dir: &Path,
    states: &mut HashMap<PathBuf, FileState>,
    db: &Arc<Mutex<Database>>,
    registry: &Arc<AgentRegistry>,
    app: &AppHandle,
) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            Box::pin(scan_existing_files(&path, states, db, registry, app)).await;
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            process_file_update(&path, states, db, registry, app).await;
        }
    }
}

async fn process_file_update(
    path: &Path,
    states: &mut HashMap<PathBuf, FileState>,
    db: &Arc<Mutex<Database>>,
    registry: &Arc<AgentRegistry>,
    app: &AppHandle,
) {
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let Ok(mut file) = File::open(path) else { return };

    let state = states
        .entry(path.to_path_buf())
        .or_insert(FileState { offset: 0 });

    if file.seek(SeekFrom::Start(state.offset)).is_err() {
        return;
    }

    let mut reader = BufReader::new(&file);
    let mut all_events = Vec::new();

    for line in reader.by_ref().lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        all_events.extend(parse_jsonl_line(&line));
    }

    state.offset = file.seek(SeekFrom::Current(0)).unwrap_or(state.offset);

    if all_events.is_empty() {
        return;
    }

    if let Some(snap) = registry.apply_events(&session_id, &all_events) {
        if snap.input_tokens > 0 || snap.output_tokens > 0 {
            let session = Session {
                id: snap.session_id.clone(),
                project_path: snap.project.clone(),
                model: snap.model.clone(),
                input_tokens: snap.input_tokens,
                output_tokens: snap.output_tokens,
                cache_tokens: snap.cache_tokens,
                cost_usd: snap.cost_usd,
                started_at: snap.started_at.to_rfc3339(),
                updated_at: snap.last_activity.to_rfc3339(),
            };
            let db = db.lock().await;
            let _ = db.replace_session(&session);
        }

        let _ = app.emit("agent-status", &snap);
    }
}
