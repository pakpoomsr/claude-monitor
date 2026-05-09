//! Tiny persistent preferences file. Currently just tracks whether the user
//! wants hooks auto-registered on launch. Lives at
//! `<data_local_dir>/claude-monitor/prefs.json`.

use crate::pricing::ModelPricing;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub hooks_enabled: bool,
    /// Per-model price overrides, keyed by `PricingEntry.id` (e.g.
    /// "claude-opus-4-7"). Missing keys mean "use the compiled default".
    pub pricing_overrides: HashMap<String, ModelPricing>,
    /// ISO 4217 code of the currency to display costs in. USD = no
    /// conversion. Conversion rates are fetched separately and cached
    /// under `currency_cache`.
    pub pricing_currency: String,
    /// Cached FX rates from Frankfurter; refreshed at most daily.
    pub currency_cache: Option<CurrencyCache>,
    /// Capture file snapshots on `Edit`/`Write`/`MultiEdit`/`NotebookEdit`
    /// tool calls (powers the History tab). Hook-driven, so requires
    /// `hooks_enabled` to take effect.
    pub snapshots_enabled: bool,
    /// Days to retain captured snapshots before the startup sweep prunes them.
    pub snapshot_retention_days: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrencyCache {
    /// Map of currency code → units of that currency per 1 USD.
    pub rates: HashMap<String, f64>,
    /// RFC3339 timestamp of the fetch.
    pub fetched_at: String,
    pub source: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            hooks_enabled: true,
            pricing_overrides: HashMap::new(),
            pricing_currency: "USD".to_string(),
            currency_cache: None,
            snapshots_enabled: true,
            snapshot_retention_days: 14,
        }
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
                return;
            }
            restrict_to_owner(&path);
        }
        Err(e) => eprintln!("[claude-monitor] prefs: serialize failed: {e}"),
    }
}

fn restrict_to_owner(path: &std::path::Path) {
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
