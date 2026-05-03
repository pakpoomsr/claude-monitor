// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agents;
mod api;
mod db;
mod hooks;
mod parser;
mod prefs;
mod settings_writer;
mod watcher;

use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};
use tokio::sync::Mutex;

use crate::agents::{AgentRegistry, AgentSettings, AgentSnapshot};

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
    prefs::save(&prefs::Prefs { hooks_enabled: true });
    Ok(HooksStatus {
        registered: true,
        url: server.url,
        port: server.port,
    })
}

#[tauri::command]
fn unregister_hooks(state: tauri::State<'_, AppState>) -> Result<HooksStatus, String> {
    settings_writer::unregister()?;
    prefs::save(&prefs::Prefs { hooks_enabled: false });
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

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let db = db::Database::new().expect("Failed to init DB");
            let db = Arc::new(Mutex::new(db));
            let registry = Arc::new(AgentRegistry::new());

            let state = AppState {
                db: db.clone(),
                api_key: Arc::new(Mutex::new(None)),
                registry: registry.clone(),
                hook_server: parking_lot::RwLock::new(None),
            };
            app.manage(state);

            // Spawn the agent-status tick loop (idle/permission detection)
            agents::spawn_tick_loop(app.handle().clone(), registry.clone());

            // Spawn the JSONL watcher
            let app_handle = app.handle().clone();
            let db_watcher = db.clone();
            let registry_watcher = registry.clone();
            tauri::async_runtime::spawn(async move {
                watcher::start_watcher(app_handle, db_watcher, registry_watcher).await;
            });

            // Spawn the embedded hook HTTP server. Bind on app start so the
            // URL is stable for the duration of the run; port is ephemeral
            // so we re-register hooks after each bind to keep the entries
            // in ~/.claude/settings.json pointing at the live port.
            let app_handle = app.handle().clone();
            let registry_hooks = registry.clone();
            tauri::async_runtime::spawn(async move {
                match hooks::spawn(app_handle.clone(), registry_hooks).await {
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
            get_agent_settings,
            set_agent_settings,
            get_daily_summary,
            get_sessions,
            get_weekly_chart,
            set_api_key,
            fetch_api_usage,
            hooks_status,
            register_hooks,
            unregister_hooks,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
