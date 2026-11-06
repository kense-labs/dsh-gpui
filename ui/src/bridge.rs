//! Typed Rust-to-JavaScript bridge for Tauri IPC in the WASM host.

use serde_json::Value;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
export function dshInvoke(command, args) {
    const api = window.__TAURI__;
    if (!api?.core?.invoke) {
        return Promise.reject(new Error('Tauri IPC 不可用，请通过 dsh-gpui 桌面客户端打开。'));
    }
    return api.core.invoke(command, args);
}

export function dshListen(callback) {
    const api = window.__TAURI__;
    if (!api?.event?.listen) {
        return Promise.reject(new Error('Tauri 事件通道不可用。'));
    }
    return api.event.listen('acp-event', event => callback(JSON.stringify(event.payload)));
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = dshInvoke)]
    fn invoke_js(command: &str, args: JsValue) -> js_sys::Promise;

    #[wasm_bindgen(js_name = dshListen)]
    fn listen_js(callback: &js_sys::Function) -> js_sys::Promise;
}

/// Invoke one Tauri command and normalize its result to JSON.
pub async fn invoke(command: &str, args: Value) -> Result<Value, String> {
    let args = serde_wasm_bindgen::to_value(&args).map_err(|error| error.to_string())?;
    let result = wasm_bindgen_futures::JsFuture::from(invoke_js(command, args)).await;
    match result {
        Ok(value) => serde_wasm_bindgen::from_value(value).map_err(|error| error.to_string()),
        Err(error) => Err(js_error(error)),
    }
}

/// Subscribe to backend ACP events. The returned Promise resolves to an unlisten function.
pub fn listen(callback: &js_sys::Function) -> js_sys::Promise {
    listen_js(callback)
}

fn js_error(value: JsValue) -> String {
    value
        .as_string()
        .or_else(|| {
            js_sys::JSON::stringify(&value)
                .ok()
                .and_then(|text| text.as_string())
        })
        .unwrap_or_else(|| format!("{value:?}"))
}
