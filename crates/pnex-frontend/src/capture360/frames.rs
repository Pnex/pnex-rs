//! JS camera bridge — getUserMedia in the webview (Android cfg module).
//!
//! wry already handles the runtime-permission flow (RustWebChromeClient.kt maps
//! `VIDEO_CAPTURE` → `CAMERA` with a Capacitor launcher) and the content is
//! served via WebViewAssetLoader (secure context): on the Rust side, injecting
//! the JS is enough (`media_viewer.rs` pattern, `document::eval`).
//!
//! Per-frame sequence: canvas scaled to ~1000 px → `toDataURL('image/jpeg')`
//! → global stash → fetched in 128 KiB chunks (we don't push large untested
//! payloads through the IPC bridge) → base64 decoding (`base64` crate,
//! already a dep).

use base64::Engine as _;

/// Frame width cap (V2): the canvas captures the track at full
/// resolution up to 1080 wide (beyond that, downscale) — the corrected hfov
/// (43.7°) gives ~16 px/° at 720; 1080 would bring it to ~25 px/°.
const FRAME_MAX_WIDTH: u32 = 1080;
/// JPEG quality of the frames.
const FRAME_QUALITY: f64 = 0.8;
/// Size of the base64 chunks crossing the IPC bridge.
const CHUNK: usize = 128 * 1024;

/// Camera bridge error.
#[derive(Debug)]
pub enum FrameError {
    /// Permission denied (NotAllowedError) or pre-check lost.
    Permission,
    /// No camera (NotFoundError / OverconstrainedError / missing track).
    NoVideo,
    /// Other JS failure (detail for the log).
    Unknown(String),
}

/// Opens the rear camera and attaches it to the `<video>` (srcObject).
/// Returns the track's (width, height) (0 if unknown).
///
/// ⚠️ NO async in the eval: natively, `document::eval` does not await
/// promises (proven tron.rs/media_viewer.rs pattern: sync return) —
/// getUserMedia is fired and forgotten (.then/.catch set globals),
/// then a SYNCHRONOUS eval polls.
pub async fn open_camera(video_id: &str) -> Result<(u32, u32), FrameError> {
    let id_q = video_id.replace('\'', "");
    // 1) Fire-and-forget launch — the result lands in
    //    window.__p360Ready / __p360Err.
    let _ = dioxus::document::eval(&format!(
        "window.__p360Ready = false; \
         window.__p360Err = ''; \
         navigator.mediaDevices.getUserMedia({{ audio: false, \
             video: {{ facingMode: {{ ideal: 'environment' }}, \
                 width: {{ ideal: 1920 }}, height: {{ ideal: 1080 }} }} }}) \
         .then(s => {{ \
             window.__p360Stream = s; \
             const v = document.getElementById('{id_q}'); \
             if (v) {{ v.srcObject = s; v.muted = true; v.play(); }} \
             window.__p360Ready = true; \
         }}) \
         .catch(e => {{ window.__p360Err = (e && e.name) || 'Error'; }});",
        id_q = id_q
    ));

    // 2) Synchronous polling (~6 s max).
    for _ in 0..40 {
        crate::util::sleep(std::time::Duration::from_millis(150)).await;
        let value = eval_value(&format!(
            "const s = window.__p360Stream; \
             const v = document.getElementById('{id_q}'); \
             if (s && v && v.srcObject !== s) {{ v.srcObject = s; v.muted = true; v.play(); }} \
             return JSON.stringify({{ ready: !!window.__p360Ready, \
                 err: window.__p360Err || '', \
                 w: s ? (s.getVideoTracks()[0]?.getSettings().width || 0) : 0, \
                 h: s ? (s.getVideoTracks()[0]?.getSettings().height || 0) : 0 }})",
        ))
        .await?;
        if value.get("err").and_then(|v| v.as_str()).unwrap_or("") != "" {
            let err = value.get("err").and_then(|v| v.as_str()).unwrap_or("");
            return match err {
                "NotAllowedError" => Err(FrameError::Permission),
                "NotFoundError" | "OverconstrainedError" => Err(FrameError::NoVideo),
                _ => Err(FrameError::Unknown(err.to_string())),
            };
        }
        if value
            .get("ready")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let w = value.get("w").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let h = value.get("h").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            return Ok((w, h));
        }
    }
    Err(FrameError::Unknown("getUserMedia : timeout".into()))
}

/// Captures the current `<video>` frame: scaled canvas + JPEG + base64
/// fetched in chunks.
pub async fn grab_frame(video_id: &str) -> Result<Vec<u8>, FrameError> {
    let id_q = video_id.replace('\'', "");
    // 1) Render + stash, return the chunking metadata.
    let meta = eval_value(&format!(
        "const v = document.getElementById('{id_q}'); \
         if (!v || !v.videoWidth) return JSON.stringify({{ ok: false, err: 'NoVideo' }}); \
         const tw = Math.min({frame_w}, v.videoWidth); \
         const c = document.createElement('canvas'); \
         c.width = tw; c.height = Math.round(v.videoHeight * tw / v.videoWidth); \
         c.getContext('2d').drawImage(v, 0, 0, c.width, c.height); \
         window.__p360Frame = c.toDataURL('image/jpeg', {quality}); \
         return JSON.stringify({{ ok: true, \
             len: window.__p360Frame.length, \
             chunks: Math.ceil(window.__p360Frame.length / {chunk}) }});",
        id_q = id_q,
        // Pass-through up to 1080 wide: the getUserMedia track may
        // deliver more than the historical 720 cap (recent devices
        // offer 1080×1920 portrait) — no needless downscale.
        frame_w = FRAME_MAX_WIDTH,
        quality = FRAME_QUALITY,
        chunk = CHUNK
    ))
    .await?;
    if !meta.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        return Err(FrameError::NoVideo);
    }
    let chunks = meta.get("chunks").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    if chunks == 0 {
        return Err(FrameError::Unknown("empty payload".into()));
    }

    // 2) Fetch chunk by chunk.
    let mut payload = String::with_capacity(chunks * CHUNK);
    for i in 0..chunks {
        // ⚠️ chunk = RAW base64 — not JSON: eval_string, not eval_value.
        let piece = eval_string(&format!(
            "return window.__p360Frame.slice({start}, {end})",
            start = i * CHUNK,
            end = (i + 1) * CHUNK
        ))
        .await?;
        payload.push_str(&piece);
    }

    // 3) Free the global stash.
    let _ = dioxus::document::eval("window.__p360Frame = null").await;

    // 4) Strip the data: prefix + base64 decoding.
    let Some(b64) = payload.strip_prefix("data:image/jpeg;base64,") else {
        return Err(FrameError::Unknown("missing data: prefix".into()));
    };
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| FrameError::Unknown(format!("base64 : {e}")))
}

/// Stops the camera stream and clears the stash — fire-and-forget.
pub fn close_camera() {
    dioxus::prelude::spawn(async move {
        let _ = dioxus::document::eval(
            "try { window.__p360Stream && window.__p360Stream.getTracks().forEach(t => t.stop()); } catch (e) {} window.__p360Stream = null; window.__p360Frame = null; window.__p360Ready = false; window.__p360Err = '';",

        )
        .await;
    });
}

/// JS eval → raw String (NO JSON parse — for base64 payloads).
async fn eval_string(js: &str) -> Result<String, FrameError> {
    let value = dioxus::document::eval(js)
        .await
        .map_err(|e| FrameError::Unknown(format!("eval : {e:?}")))?;
    if value.is_string() {
        Ok(value.as_str().unwrap_or("").to_string())
    } else {
        serde_json::to_string(&value).map_err(|e| FrameError::Unknown(format!("json : {e}")))
    }
}

/// JS eval → serde_json::Value (the JS must return a JSON string or a
/// JSON-able value; failure → FrameError::Unknown).
async fn eval_value(js: &str) -> Result<serde_json::Value, FrameError> {
    let value = dioxus::document::eval(js)
        .await
        .map_err(|e| FrameError::Unknown(format!("eval : {e:?}")))?;
    // The dioxus bridge may deliver a JSON string or an already parsed object.
    if value.is_string() {
        let s = value.as_str().unwrap_or("");
        serde_json::from_str(s).map_err(|e| FrameError::Unknown(format!("json : {e}")))
    } else {
        serde_json::to_value(&value).map_err(|e| FrameError::Unknown(format!("json : {e}")))
    }
}
