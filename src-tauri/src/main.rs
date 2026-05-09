// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agents;
mod api;
mod currency;
mod db;
mod hooks;
mod parser;
mod prefs;
mod pricing;
mod settings_writer;
mod snapshots;
mod watcher;

use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};
use tokio::sync::Mutex;

use crate::agents::{AgentRegistry, AgentSettings, AgentSnapshot, LogEntry};
use crate::pricing::PricingTable;

pub struct AppState {
    pub db: Arc<Mutex<db::Database>>,
    pub api_key: Arc<Mutex<Option<String>>>,
    pub registry: Arc<AgentRegistry>,
    pub hook_server: parking_lot::RwLock<Option<hooks::HookServer>>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HooksStatus {
    pub registered: bool,
    pub url: String,
    pub port: u16,
}

#[tauri::command]
fn hooks_status(state: tauri::State<'_, AppState>) -> Result<HooksStatus, String> {
    let server = state.hook_server.read();
    match &*server {
        None => Ok(HooksStatus { registered: false, url: String::new(), port: 0 }),
        Some(s) => Ok(HooksStatus {
            registered: settings_writer::is_registered(&s.url),
            url: s.url.clone(),
            port: s.port,
        }),
    }
}

#[tauri::command]
fn register_hooks(state: tauri::State<'_, AppState>) -> Result<HooksStatus, String> {
    let server = state.hook_server.read().clone();
    let server = server.ok_or("hook server not running")?;
    settings_writer::register(&server.url, &server.token)?;
    let mut p = prefs::load();
    p.hooks_enabled = true;
    prefs::save(&p);
    Ok(HooksStatus {
        registered: true,
        url: server.url,
        port: server.port,
    })
}

#[tauri::command]
fn unregister_hooks(state: tauri::State<'_, AppState>) -> Result<HooksStatus, String> {
    settings_writer::unregister()?;
    let mut p = prefs::load();
    p.hooks_enabled = false;
    prefs::save(&p);
    let server = state.hook_server.read().clone();
    Ok(HooksStatus {
        registered: false,
        url: server.as_ref().map(|s| s.url.clone()).unwrap_or_default(),
        port: server.as_ref().map(|s| s.port).unwrap_or(0),
    })
}

#[tauri::command]
async fn list_agents(state: tauri::State<'_, AppState>) -> Result<Vec<AgentSnapshot>, String> {
    Ok(state.registry.snapshot_all())
}

#[tauri::command]
async fn get_agent(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Option<AgentSnapshot>, String> {
    Ok(state.registry.snapshot_one(&session_id))
}

#[tauri::command]
async fn get_agent_events(
    session_id: String,
    limit: Option<usize>,
    include_history: Option<bool>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<LogEntry>, String> {
    let cap = limit.unwrap_or(200);
    let ring = state.registry.events_for(&session_id, cap);

    // Live ring already saturates the window, OR the caller opted out of
    // SQLite — return the ring as-is. SQLite is a strict superset of the
    // ring (both fed by the same `record()` call), so when the ring has
    // capacity we top up from SQLite which has older entries the ring
    // already evicted.
    if !include_history.unwrap_or(true) || ring.len() >= cap {
        return Ok(ring);
    }

    let history = {
        let db = state.db.lock().await;
        db.events_for(&session_id, cap).map_err(|e| e.to_string())?
    };
    // SQLite is canonical when present — it contains everything currently
    // in the ring plus the older history that already aged out.
    if history.is_empty() {
        Ok(ring)
    } else {
        Ok(history)
    }
}

#[tauri::command]
async fn get_agent_settings(state: tauri::State<'_, AppState>) -> Result<AgentSettings, String> {
    Ok(state.registry.settings())
}

#[tauri::command]
async fn set_agent_settings(
    settings: AgentSettings,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.registry.update_settings(settings);
    Ok(())
}

#[tauri::command]
async fn get_pricing(state: tauri::State<'_, AppState>) -> Result<PricingTable, String> {
    Ok(state.registry.pricing())
}

#[tauri::command]
async fn set_pricing(
    table: PricingTable,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    // Persist the per-id overrides (delta from defaults), then apply to the
    // live registry so future Usage events are costed at the new rates.
    let defaults = pricing::default_pricing_table();
    let mut overrides = std::collections::HashMap::new();
    for entry in &table.entries {
        if let Some(default_entry) = defaults.entries.iter().find(|d| d.id == entry.id) {
            if entry.pricing != default_entry.pricing {
                overrides.insert(entry.id.clone(), entry.pricing);
            }
        }
    }
    let mut p = prefs::load();
    p.pricing_overrides = overrides;
    prefs::save(&p);
    state.registry.update_pricing(table);
    Ok(())
}

#[derive(serde::Serialize)]
struct CurrencyState {
    pub active: String,
    pub list: Vec<currency::CurrencyInfo>,
    pub fetched_at: Option<String>,
}

#[tauri::command]
async fn get_currency_state() -> Result<CurrencyState, String> {
    let p = prefs::load();
    let cache = p.currency_cache.as_ref();
    Ok(CurrencyState {
        active: p.pricing_currency.clone(),
        list: currency::currency_list(cache),
        fetched_at: cache.map(|c| c.fetched_at.clone()),
    })
}

#[tauri::command]
async fn set_active_currency(code: String) -> Result<(), String> {
    if !currency::SUPPORTED.iter().any(|(c, _)| *c == code.as_str()) {
        return Err(format!("unsupported currency: {}", code));
    }
    let mut p = prefs::load();
    p.pricing_currency = code;
    prefs::save(&p);
    Ok(())
}

#[tauri::command]
async fn refresh_currency_rates() -> Result<CurrencyState, String> {
    let cache = currency::fetch_rates().await?;
    let mut p = prefs::load();
    p.currency_cache = Some(cache.clone());
    prefs::save(&p);
    Ok(CurrencyState {
        active: p.pricing_currency.clone(),
        list: currency::currency_list(Some(&cache)),
        fetched_at: Some(cache.fetched_at),
    })
}

#[tauri::command]
async fn reset_pricing(state: tauri::State<'_, AppState>) -> Result<PricingTable, String> {
    // Clear pricing overrides AND reset currency back to USD — wipe both
    // the rate edits and any non-default display currency in one shot.
    let mut p = prefs::load();
    p.pricing_overrides.clear();
    p.pricing_currency = "USD".to_string();
    prefs::save(&p);
    let table = pricing::default_pricing_table();
    state.registry.update_pricing(table.clone());
    Ok(table)
}

#[tauri::command]
async fn get_daily_summary(
    state: tauri::State<'_, AppState>,
) -> Result<db::DailySummary, String> {
    let db = state.db.lock().await;
    db.get_daily_summary().map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_sessions(
    state: tauri::State<'_, AppState>,
    limit: usize,
) -> Result<Vec<db::Session>, String> {
    let db = state.db.lock().await;
    db.get_recent_sessions(limit).map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_weekly_chart(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<db::DayStats>, String> {
    let db = state.db.lock().await;
    db.get_weekly_stats().map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_usage_range(
    start_date: String,
    end_date: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<db::DayStats>, String> {
    let db = state.db.lock().await;
    db.get_usage_range(&start_date, &end_date)
        .map_err(|e| e.to_string())
}

/// One-shot Usage tab payload: totals + per-day chart + 5 breakdowns.
/// Replaces the per-section round-trips so a date-range change is one call.
#[tauri::command]
async fn get_usage_breakdown(
    start_date: String,
    end_date: String,
    state: tauri::State<'_, AppState>,
) -> Result<db::UsageBreakdown, String> {
    let db = state.db.lock().await;
    let total = db.get_totals_in_range(&start_date, &end_date).map_err(|e| e.to_string())?;
    let by_day = db.get_usage_range(&start_date, &end_date).map_err(|e| e.to_string())?;
    let by_project = db.get_breakdown_by_project(&start_date, &end_date).map_err(|e| e.to_string())?;
    let by_model = db.get_breakdown_by_model(&start_date, &end_date).map_err(|e| e.to_string())?;
    let by_tool = db.get_breakdown_by_tool(&start_date, &end_date).map_err(|e| e.to_string())?;
    let by_shell = db.get_breakdown_by_shell(&start_date, &end_date).map_err(|e| e.to_string())?;
    let by_activity = db.get_breakdown_by_activity(&start_date, &end_date).map_err(|e| e.to_string())?;
    Ok(db::UsageBreakdown {
        total,
        by_day,
        by_project,
        by_model,
        by_tool,
        by_shell,
        by_activity,
    })
}

#[tauri::command]
async fn set_api_key(
    key: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let mut api_key = state.api_key.lock().await;
    *api_key = if key.is_empty() { None } else { Some(key) };
    Ok(())
}

#[tauri::command]
async fn fetch_api_usage(
    state: tauri::State<'_, AppState>,
) -> Result<api::UsageResponse, String> {
    let api_key = state.api_key.lock().await;
    match &*api_key {
        Some(key) => api::fetch_usage(key).await.map_err(|e| e.to_string()),
        None => Err("No API key configured".to_string()),
    }
}

// ---- Snapshot history (issue #3) ----

#[tauri::command]
async fn list_session_snapshots(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<snapshots::SnapshotRow>, String> {
    let db = state.db.lock().await;
    db.list_session_snapshots(&session_id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_recent_snapshots(
    limit: Option<usize>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<snapshots::SnapshotRow>, String> {
    let db = state.db.lock().await;
    db.list_recent_snapshots(limit.unwrap_or(200)).map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_snapshot_diff(
    snapshot_id: i64,
    state: tauri::State<'_, AppState>,
) -> Result<snapshots::DiffResult, String> {
    snapshots::diff(&state.db, snapshot_id).await
}

#[tauri::command]
async fn get_snapshot_content(
    snapshot_id: i64,
    state: tauri::State<'_, AppState>,
) -> Result<snapshots::SnapshotContent, String> {
    snapshots::get_content(&state.db, snapshot_id).await
}

#[tauri::command]
async fn restore_snapshot(
    snapshot_id: i64,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<snapshots::RestoreResult, String> {
    snapshots::restore(&state.db, &state.registry, &app, snapshot_id).await
}

#[tauri::command]
async fn purge_session_snapshots(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<usize, String> {
    snapshots::prune_session(&state.db, &session_id).await
}

#[tauri::command]
async fn get_snapshot_settings(
    state: tauri::State<'_, AppState>,
) -> Result<snapshots::SnapshotSettings, String> {
    Ok(snapshots::get_settings(&state.db).await)
}

#[tauri::command]
async fn set_snapshot_settings(
    settings: snapshots::SnapshotSettings,
    _state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    snapshots::set_settings(settings.enabled, settings.retention_days);
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let db = db::Database::new().expect("Failed to init DB");
            let db = Arc::new(Mutex::new(db));
            let registry = Arc::new(AgentRegistry::new());

            // Apply persisted pricing overrides on startup so the live
            // registry uses the user's edited rates from the first event.
            let p = prefs::load();
            let merged = pricing::merge_overrides(
                pricing::default_pricing_table(),
                &p.pricing_overrides,
            );
            registry.update_pricing(merged);

            let state = AppState {
                db: db.clone(),
                api_key: Arc::new(Mutex::new(None)),
                registry: registry.clone(),
                hook_server: parking_lot::RwLock::new(None),
            };
            app.manage(state);

            // Spawn the agent-status tick loop (idle/permission detection)
            agents::spawn_tick_loop(app.handle().clone(), registry.clone());

            // Snapshot store: announce path and prune anything older than the
            // configured retention window. Best-effort — failures are logged
            // but never fatal.
            snapshots::announce_root();
            let db_prune = db.clone();
            tauri::async_runtime::spawn(async move {
                let p = prefs::load();
                let n = snapshots::prune_older_than(&db_prune, p.snapshot_retention_days).await;
                if n > 0 {
                    println!(
                        "[claude-monitor] pruned {n} snapshots older than {} days",
                        p.snapshot_retention_days
                    );
                }
            });

            // Spawn the JSONL watcher
            let app_handle = app.handle().clone();
            let db_watcher = db.clone();
            let registry_watcher = registry.clone();
            tauri::async_runtime::spawn(async move {
                watcher::start_watcher(app_handle, db_watcher, registry_watcher).await;
            });

            // Refresh currency rates non-blockingly if the cache is missing
            // or older than 24h. Failures are silent — UI falls back to USD.
            tauri::async_runtime::spawn(async move {
                let p = prefs::load();
                let needs_refresh = match &p.currency_cache {
                    None => true,
                    Some(c) => currency::is_stale(c),
                };
                if needs_refresh {
                    match currency::fetch_rates().await {
                        Ok(cache) => {
                            let mut p = prefs::load();
                            p.currency_cache = Some(cache);
                            prefs::save(&p);
                            println!("[claude-monitor] currency rates refreshed");
                        }
                        Err(e) => eprintln!("[claude-monitor] currency refresh failed: {e}"),
                    }
                }
            });

            // Spawn the embedded hook HTTP server. Bind on app start so the
            // URL is stable for the duration of the run; port is ephemeral
            // so we re-register hooks after each bind to keep the entries
            // in ~/.claude/settings.json pointing at the live port.
            let app_handle = app.handle().clone();
            let registry_hooks = registry.clone();
            let db_hooks = db.clone();
            tauri::async_runtime::spawn(async move {
                match hooks::spawn(app_handle.clone(), registry_hooks, db_hooks).await {
                    Ok(server) => {
                        if let Some(state) = app_handle.try_state::<AppState>() {
                            *state.hook_server.write() = Some(server.clone());
                        }
                        // Auto-register hooks unless the user explicitly
                        // disabled them in a previous run.
                        let p = prefs::load();
                        if p.hooks_enabled {
                            match settings_writer::register(&server.url, &server.token) {
                                Ok(_) => println!(
                                    "[claude-monitor] auto-registered hooks at {}",
                                    server.url
                                ),
                                Err(e) => eprintln!(
                                    "[claude-monitor] auto-register failed: {e}"
                                ),
                            }
                        } else {
                            println!("[claude-monitor] hooks disabled by user prefs, skipping auto-register");
                        }
                    }
                    Err(e) => eprintln!("[claude-monitor] failed to start hook server: {e}"),
                }
            });

            // Tray icon
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", "Open Dashboard", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;

            TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("Claude Monitor")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_agents,
            get_agent,
            get_agent_events,
            get_agent_settings,
            set_agent_settings,
            get_daily_summary,
            get_sessions,
            get_weekly_chart,
            get_usage_range,
            get_usage_breakdown,
            set_api_key,
            fetch_api_usage,
            hooks_status,
            register_hooks,
            unregister_hooks,
            get_pricing,
            set_pricing,
            reset_pricing,
            get_currency_state,
            set_active_currency,
            refresh_currency_rates,
            list_session_snapshots,
            list_recent_snapshots,
            get_snapshot_diff,
            get_snapshot_content,
            restore_snapshot,
            purge_session_snapshots,
            get_snapshot_settings,
            set_snapshot_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
