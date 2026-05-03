//! Thin wrapper around `window.__TAURI__.core.invoke` and `__TAURI__.event.listen`
//! so Leptos components can talk to the Rust backend without pulling in
//! tauri-sys (which is unmaintained for Tauri 2 at time of writing).

use js_sys::{Function, Reflect};
use serde::{de::DeserializeOwned, Serialize};
use serde_wasm_bindgen::{from_value, to_value};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsValue;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke)]
    fn ts_invoke(cmd: &str, args: JsValue) -> js_sys::Promise;

    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "event"], js_name = listen)]
    fn ts_listen(event: &str, handler: &Function) -> js_sys::Promise;
}

/// Invoke a Tauri command. `args` should be a `#[derive(Serialize)]` struct
/// whose fields match the command's parameter names.
pub async fn invoke<R: DeserializeOwned, A: Serialize>(cmd: &str, args: &A) -> Result<R, String> {
    if !is_tauri() {
        return Err("not running inside Tauri".into());
    }
    let args_js = to_value(args).map_err(|e| e.to_string())?;
    let promise = ts_invoke(cmd, args_js);
    let result = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| js_err(&e))?;
    from_value(result).map_err(|e| e.to_string())
}

/// Invoke with no arguments.
pub async fn invoke_no_args<R: DeserializeOwned>(cmd: &str) -> Result<R, String> {
    invoke::<R, ()>(cmd, &()).await
}

/// Listen for events emitted by the backend. The handler receives the
/// deserialized payload. Listener stays alive for the lifetime of the app
/// (we leak the closure deliberately).
///
/// If `window.__TAURI__` is unavailable (e.g. running `trunk serve` in a
/// regular browser for styling work) this is a no-op rather than a panic.
pub fn listen<T, F>(event: &str, mut handler: F)
where
    T: DeserializeOwned + 'static,
    F: FnMut(T) + 'static,
{
    if !is_tauri() {
        return;
    }

    let closure = Closure::wrap(Box::new(move |raw: JsValue| {
        // Tauri wraps the payload as { event, id, payload }
        if let Ok(payload) = Reflect::get(&raw, &JsValue::from_str("payload"))
            && let Ok(decoded) = from_value::<T>(payload)
        {
            handler(decoded);
        }
    }) as Box<dyn FnMut(JsValue)>);

    let _ = ts_listen(event, closure.as_ref().unchecked_ref());
    closure.forget();
}

fn js_err(v: &JsValue) -> String {
    if let Some(s) = v.as_string() {
        return s;
    }
    if let Ok(msg) = Reflect::get(v, &JsValue::from_str("message"))
        && let Some(s) = msg.as_string()
    {
        return s;
    }
    js_sys::JSON::stringify(v)
        .map(String::from)
        .unwrap_or_else(|_| "unknown error".into())
}

/// True when running inside a Tauri webview (window.__TAURI__ is defined).
/// Useful for graceful fallback when running `trunk serve` standalone.
#[allow(dead_code)]
pub fn is_tauri() -> bool {
    let Some(window) = web_sys::window() else { return false };
    let Ok(t) = Reflect::get(&window, &JsValue::from_str("__TAURI__")) else { return false };
    !t.is_undefined() && !t.is_null()
}
