//! Tiny persistent preferences file. Currently just tracks whether the user
//! wants hooks auto-registered on launch. Lives at
//! `<data_local_dir>/claude-monitor/prefs.json`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub hooks_enabled: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { hooks_enabled: true }
    }
}

fn prefs_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("claude-monitor").join("prefs.json"))
}

/// Read prefs from disk; returns default if the file doesn't exist or is
/// unparseable. Never panics.
pub fn load() -> Prefs {
    let Some(path) = prefs_path() else { return Prefs::default() };
    let Ok(raw) = fs::read_to_string(&path) else { return Prefs::default() };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// Persist prefs. Best-effort — logs on failure but doesn't bubble the error
/// because a missing prefs file is recoverable.
pub fn save(p: &Prefs) {
    let Some(path) = prefs_path() else {
        eprintln!("[claude-monitor] prefs: no data_local_dir, skipping save");
        return;
    };
    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        eprintln!("[claude-monitor] prefs: mkdir failed: {e}");
        return;
    }
    match serde_json::to_string_pretty(p) {
        Ok(body) => {
            if let Err(e) = fs::write(&path, body) {
                eprintln!("[claude-monitor] prefs: write failed: {e}");
            }
        }
        Err(e) => eprintln!("[claude-monitor] prefs: serialize failed: {e}"),
    }
}
