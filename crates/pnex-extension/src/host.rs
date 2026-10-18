//! Bindings to `extension/host.js`, the browser glue (extension APIs,
//! WebCrypto, IndexedDB, websocket). Only compiled for wasm32.

use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/extension/host.js")]
extern "C" {
    #[wasm_bindgen(catch)]
    pub async fn http(
        method: &str,
        url: &str,
        bearer: &str,
        org: &str,
        body: &str,
        extra: &str,
    ) -> Result<JsValue, JsValue>;
    pub async fn sleep(ms: f64);
    pub fn now_ms() -> f64;
    pub fn random_b64(n: u32) -> String;
    #[wasm_bindgen(catch)]
    pub async fn sha256_b64(text: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch)]
    pub async fn store_get(key: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    pub async fn store_set(key: &str, value: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    pub async fn store_remove(key: &str) -> Result<JsValue, JsValue>;
    pub fn set_status(json: &str);
    pub async fn bundled_server() -> JsValue;

    #[wasm_bindgen(catch)]
    pub async fn install_key() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    pub async fn sign(message: &str) -> Result<JsValue, JsValue>;
    pub fn fingerprint() -> String;

    #[wasm_bindgen(catch)]
    pub async fn open_tab(url: &str) -> Result<JsValue, JsValue>;
    pub async fn close_tab(id: f64);
    #[wasm_bindgen(catch)]
    pub async fn agent_tab(id: f64, url: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    pub async fn read_value(tab: f64, selector: &str, attr: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    pub async fn read_page(tab: f64) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    pub async fn has_origin(url: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    pub async fn scroll(tab: f64) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    pub async fn read_initial_data(tab: f64) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    pub async fn read_player_response(tab: f64) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    pub async fn fetch_text(url: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    pub async fn fetch_blob(url: &str, max_bytes: f64) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    pub async fn upload_blob(
        id: f64,
        url: &str,
        bearer: &str,
        extra: &str,
    ) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch)]
    pub async fn ws_open(
        url: &str,
        on_text: &Closure<dyn FnMut(String)>,
        on_close: &Closure<dyn FnMut(u16)>,
    ) -> Result<JsValue, JsValue>;
    pub fn ws_send(id: f64, text: &str);
    pub fn ws_close(id: f64);
}

/// Text of a resolved promise (`""` for null/undefined/non-string).
pub fn text(v: &JsValue) -> String {
    v.as_string().unwrap_or_default()
}

/// Error message of a rejected promise.
pub fn err_text(e: &JsValue) -> String {
    e.as_string()
        .or_else(|| {
            js_sys::Reflect::get(e, &"message".into())
                .ok()
                .and_then(|m| m.as_string())
        })
        .unwrap_or_else(|| "browser error".into())
}
