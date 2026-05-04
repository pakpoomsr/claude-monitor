use crate::agents::{AgentRegistry, AgentSnapshot, HookEvent};
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use rand::TryRngCore;
use std::net::SocketAddr;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tauri::{AppHandle, Emitter};
use tokio::net::TcpListener;

/// Snapshot of the embedded hook server's bind state. Surfaced to the
/// frontend so the Settings panel can write the right URL into
/// `~/.claude/settings.json` and the header can show a "● HOOKS" dot.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HookServer {
    pub url: String,
    pub token: String,
    pub port: u16,
}

#[derive(Clone)]
struct ServerState {
    registry: Arc<AgentRegistry>,
    app: AppHandle,
    token: String,
}

/// Bind the HTTP server on `127.0.0.1:0` (random ephemeral port), spawn the
/// accept loop on Tauri's async runtime, and return the bound url + auth
/// token so callers can register hooks pointing at it.
pub async fn spawn(app: AppHandle, registry: Arc<AgentRegistry>) -> std::io::Result<HookServer> {
    let token = random_token()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("rng failure: {e}")))?;

    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
    let port = listener.local_addr()?.port();
    let url = format!("http://127.0.0.1:{port}/h");

    let state = ServerState {
        registry,
        app: app.clone(),
        token: token.clone(),
    };

    // 64 KB cap — real hook payloads are at most a few KB. Anything larger is
    // a bug or a local DoS attempt; reject early before allocating.
    let router = Router::new()
        .route("/h", post(hook_handler))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(state);

    tauri::async_runtime::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            eprintln!("[claude-monitor] hook server stopped: {e}");
        }
    });

    println!("[claude-monitor] hook server listening on {url}");

    Ok(HookServer { url, token, port })
}

async fn hook_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    // Auth check — header set by Claude Code from settings.json.
    // Constant-time compare so a co-resident process can't timing-attack the token.
    let auth = headers.get("X-Auth").and_then(|v| v.to_str().ok()).unwrap_or("");
    if auth.as_bytes().ct_eq(state.token.as_bytes()).unwrap_u8() != 1 {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "unauthorized"})));
    }

    // Try to deserialize into our HookEvent. Be lenient — Claude Code may
    // send fields we don't model; serde(rename_all = "snake_case") + flatten
    // handles unknowns by dropping them.
    let ev: HookEvent = match serde_json::from_value(payload.clone()) {
        Ok(e) => e,
        Err(e) => {
            // Don't log payload contents — they include user prompts and tool output.
            let size = serde_json::to_string(&payload).map(|s| s.len()).unwrap_or(0);
            eprintln!("[claude-monitor] hook payload parse error: {e} (payload size: {size} bytes)");
            return (StatusCode::OK, Json(serde_json::json!({})));
        }
    };

    if let Some(snap) = state.registry.apply_hook(&ev) {
        emit_status(&state.app, &snap);
    }

    // Empty 200 — observe-only, never block Claude.
    (StatusCode::OK, Json(serde_json::json!({})))
}

fn emit_status(app: &AppHandle, snap: &AgentSnapshot) {
    let _ = app.emit("agent-status", snap);
    if matches!(snap.status, crate::agents::AgentStatus::Waiting) {
        let _ = app.emit("agent-waiting", snap);
    }
}

fn random_token() -> Result<String, rand::rand_core::OsError> {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.try_fill_bytes(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
