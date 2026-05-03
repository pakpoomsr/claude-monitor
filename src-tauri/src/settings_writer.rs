//! Reads / mutates `~/.claude/settings.json` to register Claude Code hooks
//! that POST to our embedded HTTP server. All operations preserve the user's
//! existing settings — we only touch our own tagged hook entries.

use serde_json::{json, Map, Value};
use std::fs;
use std::path::PathBuf;

const TAG_KEY: &str = "_claude_monitor";
const HOOK_EVENTS: &[&str] = &[
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "Stop",
    "Notification",
    "PermissionRequest",
    "SessionStart",
    "SessionEnd",
    "SubagentStart",
    "SubagentStop",
];

fn settings_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".claude").join("settings.json"))
}

fn backup_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".claude").join("settings.json.bak"))
}

/// Read settings.json (or {} if missing). Returns a parse error on malformed
/// JSON so the caller can refuse to overwrite.
pub fn read() -> Result<Value, String> {
    let path = settings_path().ok_or("home dir not found")?;
    if !path.exists() {
        return Ok(Value::Object(Map::new()));
    }
    let raw = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    if raw.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str::<Value>(&raw).map_err(|e| e.to_string())
}

/// Are our hook entries currently registered for the given URL?
pub fn is_registered(url: &str) -> bool {
    let Ok(settings) = read() else { return false };
    let Some(hooks) = settings.get("hooks").and_then(|h| h.as_object()) else {
        return false;
    };
    for arr in hooks.values() {
        let Some(arr) = arr.as_array() else { continue };
        for entry in arr {
            if entry.get(TAG_KEY) == Some(&Value::Bool(true))
                && let Some(inner) = entry.get("hooks").and_then(|h| h.as_array())
            {
                for h in inner {
                    if h.get("url").and_then(|u| u.as_str()) == Some(url) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Register hook entries pointing at `url` with `X-Auth: token`. Backs up
/// existing settings to `.bak` (only if no backup exists yet — avoids
/// clobbering a user-edited backup). Idempotent.
pub fn register(url: &str, token: &str) -> Result<(), String> {
    let path = settings_path().ok_or("home dir not found")?;
    let bak = backup_path().ok_or("home dir not found")?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let mut settings = read()?;
    let original = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;

    // Backup before first write (don't overwrite user-edited backups).
    if path.exists() && !bak.exists() {
        fs::write(&bak, &original).map_err(|e| e.to_string())?;
    }

    let entry = json!({
        TAG_KEY: true,
        "hooks": [
            {
                "type": "http",
                "url": url,
                "headers": { "X-Auth": token },
                "timeout": 5
            }
        ]
    });

    let hooks_obj = settings
        .as_object_mut()
        .ok_or("settings.json root is not an object")?
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or("settings.json `hooks` is not an object")?;

    for evt in HOOK_EVENTS {
        let arr = hooks_obj
            .entry((*evt).to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| format!("settings.json `hooks.{evt}` is not an array"))?;

        // Replace any pre-existing claude_monitor entry, else append.
        let mut replaced = false;
        for existing in arr.iter_mut() {
            if existing.get(TAG_KEY) == Some(&Value::Bool(true)) {
                *existing = entry.clone();
                replaced = true;
                break;
            }
        }
        if !replaced {
            arr.push(entry.clone());
        }
    }

    write_atomic(&path, &settings)?;
    Ok(())
}

/// Remove our tagged hook entries. Empty event arrays are removed too.
pub fn unregister() -> Result<(), String> {
    let path = settings_path().ok_or("home dir not found")?;
    if !path.exists() {
        return Ok(());
    }

    let mut settings = read()?;
    let Some(root) = settings.as_object_mut() else { return Ok(()) };
    let Some(hooks_obj) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return Ok(());
    };

    let event_keys: Vec<String> = hooks_obj.keys().cloned().collect();
    for evt in event_keys {
        if let Some(arr) = hooks_obj.get_mut(&evt).and_then(|a| a.as_array_mut()) {
            arr.retain(|entry| entry.get(TAG_KEY) != Some(&Value::Bool(true)));
            if arr.is_empty() {
                hooks_obj.remove(&evt);
            }
        }
    }

    if hooks_obj.is_empty() {
        root.remove("hooks");
    }

    write_atomic(&path, &settings)?;
    Ok(())
}

fn write_atomic(path: &std::path::Path, value: &Value) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    fs::write(&tmp, body).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}
