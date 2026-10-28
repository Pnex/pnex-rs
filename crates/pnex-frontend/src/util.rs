//! Small portable wasm32/native utilities.

use std::time::Duration;

/// Local date/time "YYYY-MM-DD HH:MM" — default names for new items
/// (flows, dashboards). Via JS `Date` on the web (chrono::Local panics on
/// wasm32 without the wasmbind feature), chrono natively (tests).
pub fn now_label() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let now = js_sys::Date::new_0();
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            now.get_full_year(),
            now.get_month() + 1,
            now.get_date(),
            now.get_hours(),
            now.get_minutes()
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
    }
}

/// A translated string turned into stored data (default names): drops
/// Fluent's invisible bidi isolation marks (U+2068/U+2069) around
/// placeables, which would otherwise be saved inside the name.
pub fn plain_text(text: impl Into<String>) -> String {
    text.into()
        .chars()
        .filter(|c| !matches!(c, '\u{2068}' | '\u{2069}'))
        .collect()
}

/// Wall clock in epoch seconds (`Date.now()` on the web: `SystemTime`
/// panics on wasm32-unknown-unknown).
pub fn now_secs() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() / 1000.0
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        chrono::Utc::now().timestamp_millis() as f64 / 1000.0
    }
}

/// Same as `now_label` with second precision — auto-saved template names on
/// test/preview (datetime in the name, collisions unlikely within a minute).
pub fn now_label_secs() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let now = js_sys::Date::new_0();
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            now.get_full_year(),
            now.get_month() + 1,
            now.get_date(),
            now.get_hours(),
            now.get_minutes(),
            now.get_seconds()
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
    }
}

/// Portable sleep: `futures_timer::Delay` panics on wasm32
/// (`Instant::now()` → "time not implemented on this platform") —
/// `gloo-timers` (setTimeout) in the browser.
pub async fn sleep(duration: Duration) {
    #[cfg(target_arch = "wasm32")]
    gloo_timers::future::TimeoutFuture::new(duration.as_millis().min(u32::MAX as u128) as u32)
        .await;
    #[cfg(not(target_arch = "wasm32"))]
    futures_timer::Delay::new(duration).await;
}

/// Triggers a browser download of bytes (firmware binary).
///
/// base64 data URI rather than Blob/URL: zero js-sys dependency, and the
/// binaries involved (1–4 MB) go through fine. No-op natively (the
/// desktop target will pick its own dialog).
pub fn save_blob(filename: &str, bytes: &[u8]) {
    #[cfg(target_arch = "wasm32")]
    {
        use base64::Engine as _;
        use web_sys::wasm_bindgen::JsCast;
        let data = format!(
            "data:application/octet-stream;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };
        let Ok(element) = document.create_element("a") else {
            return;
        };
        let Ok(anchor) = element.dyn_into::<web_sys::HtmlAnchorElement>() else {
            return;
        };
        anchor.set_href(&data);
        anchor.set_download(filename);
        anchor.click();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (filename, bytes);
    }
}

/// Pre-filled server host (build forms): the current origin of the
/// front — the backend serves the front same-origin, the device must
/// be able to reach this host.
pub fn default_host() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.location().host().ok())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "localhost:5150".to_string())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        "localhost:5150".to_string()
    }
}

/// A host reachable by the browser but not by a device: loopback.
/// The wizard pre-fills `localhost:5150` when the UI is opened
/// on the server machine — an ESP8266 dialing `localhost` connects
/// to ITSELF (lesson of 2026-09-02, "connection failed" builds).
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim();
    host.starts_with("localhost")
        || host.starts_with("127.")
        || host.starts_with("[::1]")
        || host == "::1"
}

/// Pre-filled host for FIRMWARE build forms: like
/// [`default_host`] but empty on a loopback origin — the field
/// must be filled consciously (server LAN IP), the UI shows a
/// warning if the user still types a local address.
/// The customs' Python snippet URL keeps [`default_host`]: a script
/// on the PC DOES reach localhost.
pub fn default_device_host() -> String {
    if is_loopback_host(&default_host()) {
        String::new()
    } else {
        default_host()
    }
}

/// Copies to the browser clipboard: off-screen temporary textarea
/// then `exec_command("copy")` (synchronous, no Clipboard API nor promise —
/// the value stays shown and selectable next to it). No-op natively.
pub fn copy_text(text: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        use web_sys::wasm_bindgen::JsCast;
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };
        let Ok(element) = document.create_element("textarea") else {
            return;
        };
        let Ok(area) = element.dyn_into::<web_sys::HtmlTextAreaElement>() else {
            return;
        };
        area.set_value(text);
        let _ = area.set_attribute("style", "position:fixed;left:-9999px");
        if let Some(body) = document.body() {
            let _ = body.append_child(&area);
            area.select();
            // exec_command lives on HtmlDocument (cast of a second view).
            if let Ok(html_doc) = document.clone().dyn_into::<web_sys::HtmlDocument>() {
                let _ = html_doc.exec_command("copy");
            }
            let _ = body.remove_child(&area);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = text;
    }
}

/// Triggers the download of a media's bytes: wasm → `<a
/// download>` anchor on a `blob:` URL (large files — no base64, save_blob
/// pattern); Android → MediaStore `Download/Pnex` via JNI (capture.rs
/// pattern, see `download.rs`); other native: no-op. `true` if the
/// download was triggered/saved.
pub fn trigger_download(filename: &str, mime: &str, bytes: &[u8]) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        use web_sys::wasm_bindgen::JsCast as _;
        let _ = mime; // only the native path consumes the mime (MediaStore)
        let Some(url) = bytes_to_blob_url(bytes) else {
            return false;
        };
        let Some(window) = web_sys::window() else {
            return false;
        };
        let Some(document) = window.document() else {
            return false;
        };
        let Ok(element) = document.create_element("a") else {
            return false;
        };
        let Ok(anchor) = element.dyn_into::<web_sys::HtmlAnchorElement>() else {
            return false;
        };
        anchor.set_href(&url);
        anchor.set_download(filename);
        anchor.click();
        // Deferred revocation: done immediately, it can cancel the started
        // download — 60 s then fire-and-forget (closure deliberately leaked:
        // one-shot). The blob URL otherwise lives until the document unloads.
        let Some(window) = web_sys::window() else {
            return true;
        };
        let cb: wasm_bindgen::closure::Closure<dyn FnMut()> =
            wasm_bindgen::closure::Closure::new(move || {
                let _ = web_sys::Url::revoke_object_url(&url);
            });
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.as_ref().unchecked_ref(),
            60_000,
        );
        std::mem::forget(cb); // one-shot: the callback must outlive the fire
        true
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Android: MediaStore Download/Pnex (JNI); other native: the
        // download.rs stub returns false (desktop with no native dialog — no-op,
        // save_blob pattern).
        crate::download::save_to_downloads(filename, mime, bytes)
    }
}

/// `blob:` URL of an authenticated bytes endpoint — `<img>` and WebGL
/// viewers cannot set headers, so the bytes are fetched by the Rust
/// client (Bearer/X-Org-Id/401 refresh handled) then converted into a JS
/// Blob (pure `js_sys`: `Reflect` on `URL.createObjectURL`, no web-sys
/// feature). `None` on failure (or natively: the blob would live in
/// the webview, CORS would forbid the cross-origin JS fetch — V1 limitation,
/// see docs/architecture/media.md).
pub async fn media_blob_url(path: &str, content_type: Option<&str>) -> Option<String> {
    media_blob_url_tracked(path, content_type, None).await
}

/// Download progress of a media: (received bytes, total bytes if known).
pub type DownloadProgress = (u64, Option<u64>);

/// [`media_blob_url`] reporting its download in `progress` (back to `None`
/// once done). On the web the bytes come from the browser media cache when
/// the server confirms (304) they are still current — see `media_cache`.
pub async fn media_blob_url_tracked(
    path: &str,
    content_type: Option<&str>,
    progress: Option<dioxus::prelude::Signal<Option<DownloadProgress>>>,
) -> Option<String> {
    use crate::api::client::{request_bytes_conditional, ConditionalBytes};
    use dioxus::prelude::{ReadableExt, WritableExt};

    #[cfg(target_arch = "wasm32")]
    let key = format!("{}{}", crate::api::config::api_base(), path);
    #[cfg(target_arch = "wasm32")]
    let cached = crate::media_cache::lookup(&key).await;
    #[cfg(target_arch = "wasm32")]
    let if_none_match = cached.as_ref().map(|c| c.etag.clone());
    #[cfg(not(target_arch = "wasm32"))]
    let if_none_match: Option<String> = None;

    let mut last = 0u64;
    let mut report = progress;
    let outcome =
        request_bytes_conditional(path, if_none_match.as_deref(), move |received, total| {
            let Some(signal) = report.as_mut() else {
                return;
            };
            // Re-render about every percent (every MB without a length).
            let step = total.map_or(1 << 20, |t| (t / 100).max(1));
            if received == 0 || received - last >= step || Some(received) == total {
                last = received;
                signal.set(Some((received, total)));
            }
        })
        .await;
    if let Some(mut signal) = progress {
        if signal.peek().is_some() {
            signal.set(None);
        }
    }
    match outcome.ok()? {
        ConditionalBytes::NotModified => {
            #[cfg(target_arch = "wasm32")]
            {
                cached?.blob_url().await
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                None
            }
        }
        ConditionalBytes::Body { bytes, etag } => {
            // Native: base64 data URI (save_blob school) — a JS blob cannot
            // be played cross-origin from the webview; photos stay viewable
            // on Android.
            #[cfg(not(target_arch = "wasm32"))]
            {
                use base64::Engine as _;
                let _ = etag;
                let mime = content_type.unwrap_or("application/octet-stream");
                Some(format!(
                    "data:{mime};base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&bytes)
                ))
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = content_type;
                let blob = bytes_to_blob(&bytes)?;
                if let Some(etag) = etag {
                    // Stored in the background: the viewer does not wait
                    // for the disk write.
                    let blob = blob.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        crate::media_cache::store(&key, &blob, &etag).await;
                    });
                }
                web_sys::Url::create_object_url_with_blob(&blob).ok()
            }
        }
    }
}

/// Converts bytes into a blob: URL (wasm only).
#[cfg(target_arch = "wasm32")]
fn bytes_to_blob_url(bytes: &[u8]) -> Option<String> {
    web_sys::Url::create_object_url_with_blob(&bytes_to_blob(bytes)?).ok()
}

/// JS `Blob` holding a copy of `bytes` (wasm only).
#[cfg(target_arch = "wasm32")]
fn bytes_to_blob(bytes: &[u8]) -> Option<web_sys::Blob> {
    let array = js_sys::Uint8Array::from(bytes);
    let seq = js_sys::Array::of1(&array);
    web_sys::Blob::new_with_u8_array_sequence(&seq).ok()
}

/// Rect of a DOM element by id — (left, top, width, height). Wasm:
/// `get_bounding_client_rect`; native: None (the flat markers of the
/// annotations page only render on web, canvas_rect pattern).
pub fn element_rect(element_id: &str) -> Option<(f64, f64, f64, f64)> {
    #[cfg(target_arch = "wasm32")]
    {
        let document = web_sys::window()?.document()?;
        let element = document.get_element_by_id(element_id)?;
        let rect = element.get_bounding_client_rect();
        Some((rect.left(), rect.top(), rect.width(), rect.height()))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = element_id;
        None
    }
}

/// Image URL of in-memory bytes (camera frames, test images): blob URL on
/// web, `data:` URI on native (the webview cannot fetch cross-origin blobs).
/// Release it with [`release_image_url`].
pub fn image_url_from_bytes(bytes: &[u8], mime: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = mime;
        bytes_to_blob_url(bytes)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use base64::Engine as _;
        Some(format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ))
    }
}

/// Releases a URL from [`image_url_from_bytes`] (blob URLs only).
pub fn release_image_url(url: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = web_sys::Url::revoke_object_url(url);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = url;
    }
}

#[cfg(test)]
mod plain_text_tests {
    #[test]
    fn strips_fluent_isolation_marks() {
        assert_eq!(
            super::plain_text("Tour \u{2068}2026-10-11 11:39\u{2069}"),
            "Tour 2026-10-11 11:39"
        );
    }
}
