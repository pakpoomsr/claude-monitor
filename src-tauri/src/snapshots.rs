//! File snapshot subsystem powering the History / Version Rollback tab.
//!
//! Captures pre/post state of every `Edit`/`Write`/`MultiEdit`/`NotebookEdit`
//! tool call as Claude Code reports it via hooks. Stores blobs on disk under
//! `<data_local>/claude-monitor/snapshots/<session_id>/<id>.bin` with metadata
//! in the `file_snapshots` SQLite table. Powers diff and one-click restore.
//!
//! Single writer to the snapshots dir. Anything that touches blob files goes
//! through this module — mirrors the single-writer rules already in place for
//! `compute_status` (agents.rs), `record()` (agents.rs), and `estimate_cost`
//! (pricing.rs).
//!
//! Restore is itself reversible: before overwriting a file we capture a
//! `pre-restore` snapshot of its current bytes, so the user can undo an
//! unwanted restore from the same UI.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::agents::{AgentRegistry, LogEntry, LogSource};
use crate::db::Database;

/// 1 MB per file. Beyond this we record metadata only and mark the row
/// `oversized` so the UI can show "snapshot skipped (file > 1 MB)". Keeps
/// disk bounded for sessions that touch lock files / generated code.
pub const MAX_SNAPSHOT_BYTES: u64 = 1024 * 1024;

/// File tools we capture. Bash redirects (`>`, `tee`, `sed -i`) are out of
/// scope for v1 — see plan risk #5.
pub const TRACKED_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotRow {
    pub id: i64,
    pub session_id: String,
    pub project_path: String,
    pub file_path: String,
    pub tool_name: String,
    pub phase: String,
    pub paired_id: Option<i64>,
    pub blob_path: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub is_binary: bool,
    pub oversized: bool,
    pub ts: String,
    pub tool_use_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffResult {
    pub unified: String,
    pub plus: u32,
    pub minus: u32,
    pub is_binary: bool,
    pub pre_oversized: bool,
    pub post_oversized: bool,
    pub pre: Option<SnapshotRow>,
    pub post: Option<SnapshotRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotContent {
    pub size_bytes: i64,
    pub is_binary: bool,
    /// base64 of the blob bytes; empty string if oversized or missing.
    pub base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreResult {
    pub ok: bool,
    pub pre_restore_snapshot_id: Option<i64>,
    /// True when the restored snapshot represented "no file" (a `Write` that
    /// originally created the file) — restore means we deleted it.
    pub deleted_target: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotSettings {
    pub enabled: bool,
    pub retention_days: u32,
    pub total_size_bytes: i64,
    pub total_count: i64,
}

fn snapshots_root() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("claude-monitor").join("snapshots"))
}

fn session_dir(session_id: &str) -> Option<PathBuf> {
    snapshots_root().map(|r| r.join(sanitize_id(session_id)))
}

/// Strip path separators and traversal sequences from a session id so it's
/// safe to use as a directory name. Session ids are normally UUIDs but we
/// don't rely on that.
fn sanitize_id(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

fn restrict_to_owner(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// True if the bytes look binary (any null byte in the first 8KB).
fn looks_binary(bytes: &[u8]) -> bool {
    let probe = &bytes[..bytes.len().min(8192)];
    probe.contains(&0u8)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// Resolve the target file path from a hook's `tool_input` payload. Returns
/// `None` if the tool isn't tracked or the field is missing — silent no-op
/// is the right behavior since unknown payloads should not break Claude.
fn resolve_target_path(tool_name: &str, tool_input: &serde_json::Value) -> Option<PathBuf> {
    let key = match tool_name {
        "Edit" | "Write" | "MultiEdit" => "file_path",
        "NotebookEdit" => "notebook_path",
        _ => return None,
    };
    let s = tool_input.get(key).and_then(|v| v.as_str())?;
    let path = PathBuf::from(s);
    if path.is_absolute() {
        Some(path)
    } else {
        None
    }
}

/// Atomically write `bytes` to `dest`, mirroring the symlink-safe pattern
/// from settings_writer.rs. Caller is expected to ensure the parent dir
/// exists.
fn atomic_write(dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = dest.with_extension("tmp");
    if tmp.exists() {
        let _ = fs::remove_file(&tmp);
    }
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    restrict_to_owner(&tmp);
    fs::rename(&tmp, dest)?;
    Ok(())
}

/// Capture a snapshot of `file_path` at the given `phase`. Reads disk
/// synchronously; for local files this is sub-millisecond. Returns the row id
/// (or 0 on a non-fatal error like missing file for a Write-create case,
/// where a sentinel row is still recorded).
#[allow(clippy::too_many_arguments)]
fn capture(
    db: &mut Database,
    session_id: &str,
    project_path: &str,
    file_path: &Path,
    tool_name: &str,
    phase: &str,
    tool_use_id: Option<&str>,
) -> Result<SnapshotRow, String> {
    let session_dir = session_dir(session_id).ok_or("no data_local_dir")?;
    fs::create_dir_all(&session_dir).map_err(|e| format!("snapshot mkdir: {e}"))?;
    restrict_to_owner(&session_dir);

    // Refuse to follow a symlink at the source — opening the symlink could
    // expose an unexpected file. Falls through to "file does not exist" path
    // which is also the right answer for a brand-new Write.
    let meta = fs::symlink_metadata(file_path).ok();
    let is_symlink = meta.as_ref().map(|m| m.file_type().is_symlink()).unwrap_or(false);
    let exists = meta.as_ref().map(|m| m.is_file()).unwrap_or(false);

    let (bytes, size_bytes, oversized, effective_tool_name) = if is_symlink {
        // Treat a symlink target the same as "file not present" — we don't
        // resolve it. The post-snapshot will catch the actual content.
        (Vec::new(), 0i64, false, tool_name.to_string())
    } else if !exists {
        // Pre-snapshot for a new-file Write: empty blob, special marker so
        // restore knows to delete the target rather than write an empty file.
        let marker = if tool_name == "Write" && phase == "pre" {
            "Write:create".to_string()
        } else {
            tool_name.to_string()
        };
        (Vec::new(), 0i64, false, marker)
    } else {
        let size = meta.as_ref().and_then(|m| Some(m.len())).unwrap_or(0);
        if size > MAX_SNAPSHOT_BYTES {
            (Vec::new(), size as i64, true, tool_name.to_string())
        } else {
            match fs::read(file_path) {
                Ok(b) => {
                    let len = b.len() as i64;
                    (b, len, false, tool_name.to_string())
                }
                Err(_) => (Vec::new(), 0i64, false, tool_name.to_string()),
            }
        }
    };

    let sha = sha256_hex(&bytes);
    let is_binary = !bytes.is_empty() && looks_binary(&bytes);
    let ts = Utc::now().to_rfc3339();

    // Insert row first so we have an id to use as the blob filename. This
    // also means a failed blob-write leaves a row whose blob is missing —
    // diff/restore guard against that via SHA check.
    let id = db
        .insert_file_snapshot(
            session_id,
            project_path,
            &file_path.to_string_lossy(),
            &effective_tool_name,
            phase,
            None,
            "", // blob_path filled in below
            size_bytes,
            &sha,
            is_binary,
            oversized,
            &ts,
            tool_use_id,
        )
        .map_err(|e| format!("snapshot insert: {e}"))?;

    let blob_rel = format!("{}/{}.bin", sanitize_id(session_id), id);
    if !oversized && !bytes.is_empty() {
        let blob_abs = snapshots_root().ok_or("no data_local_dir")?.join(&blob_rel);
        atomic_write(&blob_abs, &bytes).map_err(|e| format!("snapshot write: {e}"))?;
    } else if !oversized && bytes.is_empty() {
        // Touch a zero-byte file so blob_path always points at something,
        // simplifying restore logic.
        let blob_abs = snapshots_root().ok_or("no data_local_dir")?.join(&blob_rel);
        let _ = atomic_write(&blob_abs, &[]);
    }

    db.update_file_snapshot_blob_path(id, &blob_rel)
        .map_err(|e| format!("snapshot update: {e}"))?;

    Ok(SnapshotRow {
        id,
        session_id: session_id.to_string(),
        project_path: project_path.to_string(),
        file_path: file_path.to_string_lossy().to_string(),
        tool_name: effective_tool_name,
        phase: phase.to_string(),
        paired_id: None,
        blob_path: blob_rel,
        size_bytes,
        sha256: sha,
        is_binary,
        oversized,
        ts,
        tool_use_id: tool_use_id.map(String::from),
    })
}

/// Capture a `pre`-edit snapshot for an `Edit`/`Write`/`MultiEdit`/`NotebookEdit`
/// `PreToolUse` hook. No-op (returns Ok) for unsupported tools or missing
/// fields — hook is observe-only.
pub async fn capture_pre_edit(
    db: &Arc<Mutex<Database>>,
    registry: &Arc<AgentRegistry>,
    app: &tauri::AppHandle,
    session_id: &str,
    project_path: &str,
    tool_name: &str,
    tool_use_id: Option<&str>,
    tool_input: &serde_json::Value,
) -> Option<SnapshotRow> {
    if !TRACKED_TOOLS.contains(&tool_name) {
        return None;
    }
    let path = resolve_target_path(tool_name, tool_input)?;
    let row = {
        let mut g = db.lock().await;
        match capture(&mut g, session_id, project_path, &path, tool_name, "pre", tool_use_id) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[claude-monitor] snapshot pre-edit failed: {e}");
                return None;
            }
        }
    };
    emit_snapshot_event(app, registry, &row, "Snapshot:PreEdit");
    Some(row)
}

/// Capture a `post`-edit snapshot, paired with its matching `pre` row via
/// `tool_use_id`. Emits the `agent-event` log entry through the registry's
/// `record()` so the live detail-pane stream picks it up.
pub async fn capture_post_edit(
    db: &Arc<Mutex<Database>>,
    registry: &Arc<AgentRegistry>,
    app: &tauri::AppHandle,
    session_id: &str,
    project_path: &str,
    tool_name: &str,
    tool_use_id: Option<&str>,
    tool_input: &serde_json::Value,
) -> Option<SnapshotRow> {
    if !TRACKED_TOOLS.contains(&tool_name) {
        return None;
    }
    let path = resolve_target_path(tool_name, tool_input)?;
    let row = {
        let mut g = db.lock().await;
        let mut r = match capture(&mut g, session_id, project_path, &path, tool_name, "post", tool_use_id) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[claude-monitor] snapshot post-edit failed: {e}");
                return None;
            }
        };
        // Pair with the most recent unpaired `pre` row for the same tool_use_id.
        if let Some(tu_id) = tool_use_id
            && let Ok(Some(pre_id)) = g.find_unpaired_pre(session_id, tu_id)
        {
            let _ = g.set_paired_id(pre_id, r.id);
            let _ = g.set_paired_id(r.id, pre_id);
            r.paired_id = Some(pre_id);
        }
        r
    };
    emit_snapshot_event(app, registry, &row, "Snapshot:PostEdit");
    Some(row)
}

fn emit_snapshot_event(
    app: &tauri::AppHandle,
    registry: &AgentRegistry,
    row: &SnapshotRow,
    kind: &str,
) {
    use tauri::Emitter;
    let entry = LogEntry {
        session_id: row.session_id.clone(),
        timestamp: Utc::now(),
        source: LogSource::Hook,
        kind: kind.to_string(),
        summary: short_path(&row.file_path),
        details: Some(format!("snapshot_id={} phase={} {}b", row.id, row.phase, row.size_bytes)),
    };
    registry.record_external(&row.session_id, entry.clone());
    let _ = app.emit("agent-event", &entry);
}

fn short_path(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// Compute a unified diff between a snapshot's `pre` blob and its paired
/// `post` blob (or the other way around). The snapshot id can be either side
/// of the pair — we resolve to the (pre, post) tuple.
pub async fn diff(db: &Arc<Mutex<Database>>, snapshot_id: i64) -> Result<DiffResult, String> {
    let g = db.lock().await;
    let row = g.get_file_snapshot(snapshot_id).map_err(|e| e.to_string())?;
    let row = row.ok_or("snapshot not found")?;
    let (pre, post) = match row.phase.as_str() {
        "pre" => {
            let post = match row.paired_id {
                Some(pid) => g.get_file_snapshot(pid).map_err(|e| e.to_string())?,
                None => None,
            };
            (Some(row), post)
        }
        "post" => {
            let pre = match row.paired_id {
                Some(pid) => g.get_file_snapshot(pid).map_err(|e| e.to_string())?,
                None => None,
            };
            (pre, Some(row))
        }
        "pre-restore" => {
            // For a restore record, "pre-restore" = state before restore;
            // "paired_id" = the snapshot that was restored. Show the diff
            // between current bytes and the restored bytes.
            let restored = match row.paired_id {
                Some(pid) => g.get_file_snapshot(pid).map_err(|e| e.to_string())?,
                None => None,
            };
            (Some(row), restored)
        }
        _ => (Some(row), None),
    };
    drop(g);

    let pre_bytes = match &pre {
        Some(r) if !r.oversized => read_blob(&r.blob_path).unwrap_or_default(),
        _ => Vec::new(),
    };
    let post_bytes = match &post {
        Some(r) if !r.oversized => read_blob(&r.blob_path).unwrap_or_default(),
        _ => Vec::new(),
    };

    let is_binary = looks_binary(&pre_bytes) || looks_binary(&post_bytes);
    if is_binary {
        return Ok(DiffResult {
            unified: "(binary content — diff suppressed)".to_string(),
            plus: 0,
            minus: 0,
            is_binary: true,
            pre_oversized: pre.as_ref().map(|r| r.oversized).unwrap_or(false),
            post_oversized: post.as_ref().map(|r| r.oversized).unwrap_or(false),
            pre,
            post,
        });
    }

    let pre_text = String::from_utf8_lossy(&pre_bytes).to_string();
    let post_text = String::from_utf8_lossy(&post_bytes).to_string();

    let td = similar::TextDiff::from_lines(&pre_text, &post_text);
    let mut unified = String::new();
    let mut plus: u32 = 0;
    let mut minus: u32 = 0;
    for change in td.iter_all_changes() {
        let (sigil, kind_inc) = match change.tag() {
            similar::ChangeTag::Equal => (" ", false),
            similar::ChangeTag::Insert => ("+", true),
            similar::ChangeTag::Delete => ("-", true),
        };
        if kind_inc {
            match change.tag() {
                similar::ChangeTag::Insert => plus += 1,
                similar::ChangeTag::Delete => minus += 1,
                _ => {}
            }
        }
        unified.push_str(sigil);
        unified.push_str(change.value());
        if !change.value().ends_with('\n') {
            unified.push('\n');
        }
    }

    Ok(DiffResult {
        unified,
        plus,
        minus,
        is_binary: false,
        pre_oversized: pre.as_ref().map(|r| r.oversized).unwrap_or(false),
        post_oversized: post.as_ref().map(|r| r.oversized).unwrap_or(false),
        pre,
        post,
    })
}

fn read_blob(blob_rel: &str) -> std::io::Result<Vec<u8>> {
    let root = snapshots_root().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no data_local_dir")
    })?;
    fs::read(root.join(blob_rel))
}

/// Restore a snapshot's bytes back to its original `file_path`. Captures a
/// `pre-restore` snapshot of the current file content first so the operation
/// is reversible. Returns the new pre-restore row id so the UI can offer
/// "Undo restore".
pub async fn restore(
    db: &Arc<Mutex<Database>>,
    registry: &Arc<AgentRegistry>,
    app: &tauri::AppHandle,
    snapshot_id: i64,
) -> Result<RestoreResult, String> {
    use tauri::Emitter;

    let row = {
        let g = db.lock().await;
        g.get_file_snapshot(snapshot_id)
            .map_err(|e| e.to_string())?
            .ok_or("snapshot not found")?
    };

    let target = PathBuf::from(&row.file_path);
    if !target.is_absolute() {
        return Err("snapshot has non-absolute target path".into());
    }
    if let Ok(meta) = fs::symlink_metadata(&target)
        && meta.file_type().is_symlink()
    {
        return Err("refusing to overwrite a symlink target".into());
    }

    if row.oversized {
        return Err("snapshot is oversized — restore not supported".into());
    }

    // Capture pre-restore (the file's current bytes) so user can undo.
    let pre_restore = {
        let mut g = db.lock().await;
        capture(
            &mut g,
            &row.session_id,
            &row.project_path,
            &target,
            "Restore",
            "pre-restore",
            None,
        )
        .map_err(|e| format!("pre-restore capture: {e}"))?
    };
    {
        let g = db.lock().await;
        let _ = g.set_paired_id(pre_restore.id, row.id);
    }

    // Special case: restore of a `Write:create` pre-snapshot means the file
    // didn't exist before the edit, so restoring means deleting the file.
    let mut deleted_target = false;
    if row.tool_name == "Write:create" && row.phase == "pre" {
        if target.exists() {
            fs::remove_file(&target).map_err(|e| format!("remove target: {e}"))?;
            deleted_target = true;
        }
    } else {
        let bytes = read_blob(&row.blob_path).map_err(|e| format!("read blob: {e}"))?;
        // Verify SHA-256 of disk blob vs DB row before restore.
        if sha256_hex(&bytes) != row.sha256 {
            return Err("snapshot integrity check failed (sha256 mismatch)".into());
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir parent: {e}"))?;
        }
        atomic_write(&target, &bytes).map_err(|e| format!("write target: {e}"))?;
    }

    // Record restore in the live event ring + emit so the UI refreshes.
    let entry = LogEntry {
        session_id: row.session_id.clone(),
        timestamp: Utc::now(),
        source: LogSource::Hook,
        kind: "Snapshot:Restored".to_string(),
        summary: short_path(&row.file_path),
        details: Some(format!(
            "restored snapshot_id={} pre_restore_id={}{}",
            row.id,
            pre_restore.id,
            if deleted_target { " deleted" } else { "" }
        )),
    };
    registry.record_external(&row.session_id, entry.clone());
    let _ = app.emit("agent-event", &entry);
    let _ = app.emit(
        "snapshot-restored",
        serde_json::json!({
            "snapshotId": row.id,
            "preRestoreSnapshotId": pre_restore.id,
            "sessionId": row.session_id,
        }),
    );

    Ok(RestoreResult {
        ok: true,
        pre_restore_snapshot_id: Some(pre_restore.id),
        deleted_target,
        message: "restored".into(),
    })
}

/// Read raw bytes of a snapshot for the UI's "view content" path. Truncates
/// at MAX_SNAPSHOT_BYTES regardless of on-disk size as a safety net.
pub async fn get_content(
    db: &Arc<Mutex<Database>>,
    snapshot_id: i64,
) -> Result<SnapshotContent, String> {
    use base64::{engine::general_purpose, Engine as _};
    let row = {
        let g = db.lock().await;
        g.get_file_snapshot(snapshot_id)
            .map_err(|e| e.to_string())?
            .ok_or("snapshot not found")?
    };
    if row.oversized {
        return Ok(SnapshotContent {
            size_bytes: row.size_bytes,
            is_binary: row.is_binary,
            base64: String::new(),
        });
    }
    let bytes = read_blob(&row.blob_path).unwrap_or_default();
    let truncated = if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
        &bytes[..MAX_SNAPSHOT_BYTES as usize]
    } else {
        &bytes[..]
    };
    Ok(SnapshotContent {
        size_bytes: row.size_bytes,
        is_binary: row.is_binary,
        base64: general_purpose::STANDARD.encode(truncated),
    })
}

/// Delete snapshots older than `days` from both DB and disk. Logs and
/// continues on individual file-removal failures (best-effort prune).
pub async fn prune_older_than(db: &Arc<Mutex<Database>>, days: u32) -> usize {
    if days == 0 {
        return 0;
    }
    let blob_paths = {
        let mut g = db.lock().await;
        match g.delete_file_snapshots_older_than(days as i64) {
            Ok(paths) => paths,
            Err(e) => {
                eprintln!("[claude-monitor] snapshot prune (db) failed: {e}");
                return 0;
            }
        }
    };
    let n = blob_paths.len();
    if let Some(root) = snapshots_root() {
        for rel in &blob_paths {
            let _ = fs::remove_file(root.join(rel));
        }
    }
    n
}

/// Delete every snapshot for a given session, both DB rows and blob files.
pub async fn prune_session(db: &Arc<Mutex<Database>>, session_id: &str) -> Result<usize, String> {
    let blob_paths = {
        let mut g = db.lock().await;
        g.delete_file_snapshots_for_session(session_id)
            .map_err(|e| e.to_string())?
    };
    let n = blob_paths.len();
    if let Some(root) = snapshots_root() {
        for rel in &blob_paths {
            let _ = fs::remove_file(root.join(rel));
        }
        // Also remove the now-empty session dir.
        let _ = fs::remove_dir(root.join(sanitize_id(session_id)));
    }
    Ok(n)
}

/// Settings + disk-usage figure for the Settings panel.
pub async fn get_settings(db: &Arc<Mutex<Database>>) -> SnapshotSettings {
    let p = crate::prefs::load();
    let g = db.lock().await;
    let (count, size) = g.snapshot_totals().unwrap_or((0, 0));
    SnapshotSettings {
        enabled: p.snapshots_enabled,
        retention_days: p.snapshot_retention_days,
        total_count: count,
        total_size_bytes: size,
    }
}

pub fn set_settings(enabled: bool, retention_days: u32) {
    let mut p = crate::prefs::load();
    p.snapshots_enabled = enabled;
    p.snapshot_retention_days = retention_days;
    crate::prefs::save(&p);
}

/// Print the snapshot-store path on startup so the operator knows where
/// blobs land.
pub fn announce_root() {
    if let Some(root) = snapshots_root() {
        let _ = fs::create_dir_all(&root);
        println!("[claude-monitor] snapshot store ready at {}", root.display());
    }
}
