use crate::agents::AgentRegistry;
use crate::db::{Database, Session};
use crate::parser::parse_jsonl_line;
use chrono::{DateTime, Utc};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
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

/// Sub-agent JSONL files live at:
///   `<projects>/<project>/<parent_session_uuid>/subagents/agent-<sub_id>.jsonl`
/// Root sessions live at:
///   `<projects>/<project>/<session_uuid>.jsonl`
fn detect_parent_id(path: &Path) -> Option<String> {
    let parent_dir = path.parent()?;
    if parent_dir.file_name()?.to_str()? != "subagents" {
        return None;
    }
    let candidate = parent_dir.parent()?.file_name()?.to_str()?;
    // Only accept UUID-shaped parents — prevents a crafted JSONL placed under
    // `<projects>/<arbitrary>/subagents/` from claiming an attacker-chosen
    // parent_id and corrupting the agent grouping in the UI.
    if is_uuid_like(candidate) {
        Some(candidate.to_string())
    } else {
        None
    }
}

/// 8-4-4-4-12 hex with dashes, case-insensitive. Doesn't validate version
/// nibble — Claude Code session UUIDs are RFC 4122 v4 in practice but we
/// don't want to reject any legit format change.
fn is_uuid_like(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        let expect_dash = matches!(i, 8 | 13 | 18 | 23);
        if expect_dash {
            if *b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
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

    let parent_id = detect_parent_id(path);

    let Ok(mut file) = File::open(path) else { return };

    let state = states
        .entry(path.to_path_buf())
        .or_insert(FileState { offset: 0 });

    if file.seek(SeekFrom::Start(state.offset)).is_err() {
        return;
    }

    let mut reader = BufReader::new(&file);
    let mut all_events = Vec::new();

    // Cap per-line read at 8 MB. Real Claude transcript lines are at most a
    // few hundred KB; anything larger is corruption or a malicious file
    // dropped under ~/.claude/projects to OOM the watcher.
    const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
    loop {
        let mut buf = Vec::new();
        let n = match (&mut reader).take(MAX_LINE_BYTES as u64 + 1).read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        if n > MAX_LINE_BYTES {
            eprintln!(
                "[claude-monitor] skipping oversized JSONL line ({n} bytes) in {}",
                path.display()
            );
            // Drain to next newline so we resync on subsequent lines.
            let mut sink = Vec::new();
            let _ = (&mut reader).take(64 * 1024 * 1024).read_until(b'\n', &mut sink);
            continue;
        }
        let Ok(line) = std::str::from_utf8(&buf) else { continue };
        let line = line.trim_end_matches('\n').trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        all_events.extend(parse_jsonl_line(line));
    }

    state.offset = file.stream_position().unwrap_or(state.offset);

    if all_events.is_empty() {
        return;
    }

    // Use the file's mtime as the activity timestamp. For an initial scan of
    // an old JSONL file this is the time the session actually ended; for
    // live updates from the watcher it's effectively "now". Either way it
    // prevents ancient sessions from looking fresh on startup.
    let activity_at: DateTime<Utc> = file
        .metadata()
        .and_then(|m| m.modified())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(|_| Utc::now());

    let (maybe_snap, log_entries) =
        registry.apply_events(&session_id, &all_events, activity_at, parent_id.clone());
    for entry in &log_entries {
        let _ = app.emit("agent-event", entry);
    }
    if !log_entries.is_empty() {
        let mut db = db.lock().await;
        if let Err(e) = db.insert_events(&log_entries) {
            eprintln!("[claude-monitor] event log persist failed: {e}");
        }
    }
    if let Some(snap) = maybe_snap {
        if snap.input_tokens > 0 || snap.output_tokens > 0 {
            let session = Session {
                id: snap.session_id.clone(),
                project_path: snap.project.clone(),
                model: snap.model.clone(),
                input_tokens: snap.input_tokens,
                output_tokens: snap.output_tokens,
                cache_tokens: snap.cache_tokens,
                cache_write_5m_tokens: snap.cache_write_5m_tokens,
                cache_write_1h_tokens: snap.cache_write_1h_tokens,
                cache_read_tokens: snap.cache_read_tokens,
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
