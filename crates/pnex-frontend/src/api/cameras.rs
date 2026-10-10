//! Cameras & recordings API (camera-video.md D75/D76/D79/D80) — client of
//! the `/api/v1/cameras` controller: camera list, capture settings, the
//! per-day recording timeline (segment bytes, range export, range delete) and the
//! `/ws/camera/live` URL builder for the live viewer.

use pnex_core::camera::{
    AnnotationLayer, CameraSettings, CameraView, FrameAnnotation, RecordingTimeline,
};
use serde::Serialize;

use crate::api::client;
use crate::api::error::ApiError;
use crate::api::media::urlencode;

/// `GET /api/v1/cameras` — cameras of the current org (settings + live state).
pub async fn list() -> Result<Vec<CameraView>, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/cameras", None).await
}

/// Partial settings update — `None` fields are left untouched server-side.
#[derive(Debug, Default, Clone, Serialize)]
pub struct SettingsPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub framesize: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vflip: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hmirror: Option<bool>,
}

impl SettingsPatch {
    /// Full patch from a settings snapshot (the settings form sends all
    /// fields at once).
    pub fn from_settings(s: &CameraSettings) -> Self {
        Self {
            framesize: Some(s.framesize.wire().to_string()),
            quality: Some(s.quality),
            fps: Some(s.fps),
            capture_mode: Some(s.capture_mode.wire().to_string()),
            vflip: Some(s.vflip),
            hmirror: Some(s.hmirror),
        }
    }
}

/// `PATCH /api/v1/cameras/{device}/settings` — 400 body = field tokens
/// (`{"quality": "range:10..63"}`), see [`field_errors`].
pub async fn patch_settings(device: i64, patch: &SettingsPatch) -> Result<CameraView, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/cameras/{device}/settings"),
        Some(serde_json::to_value(patch).unwrap_or_default()),
    )
    .await
}

/// Field errors of a 400 settings response: `(field, token)` pairs.
pub fn field_errors(err: &ApiError) -> Vec<(String, String)> {
    if err.status != Some(400) {
        return Vec::new();
    }
    err.body
        .as_ref()
        .and_then(|b| b.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// `GET /api/v1/cameras/segments?stream_id=…` — recordings of an IP
/// stream (D175), newest first.
pub async fn stream_segments(
    stream_id: &str,
    limit: i64,
    offset: i64,
) -> Result<pnex_core::Paginated<pnex_core::camera::VideoSegment>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!(
            "/api/v1/cameras/segments?stream_id={}&limit={limit}&offset={offset}",
            urlencode(stream_id)
        ),
        None,
    )
    .await
}

/// `GET /api/v1/cameras/segments/{id}/content` — MJPEG-AVI bytes.
pub async fn segment_bytes(id: &str) -> Result<Vec<u8>, ApiError> {
    client::request_bytes(
        reqwest::Method::GET,
        &format!("/api/v1/cameras/segments/{id}/content"),
    )
    .await
}

/// `?from=…&to=…` of a recording window (RFC 3339 bounds).
fn range_query(from: &str, to: &str) -> String {
    format!("?from={}&to={}", urlencode(from), urlencode(to))
}

/// `GET /api/v1/cameras/{device}/recordings` — ascending segments of the
/// window (≤ 26 h), overlaps included.
pub async fn recordings(device: i64, from: &str, to: &str) -> Result<RecordingTimeline, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!(
            "/api/v1/cameras/{device}/recordings{}",
            range_query(from, to)
        ),
        None,
    )
    .await
}

/// `GET /api/v1/cameras/{device}/recordings/export` — one MJPEG-AVI of the
/// window (chained segments).
pub async fn export_recording(device: i64, from: &str, to: &str) -> Result<Vec<u8>, ApiError> {
    client::request_bytes(
        reqwest::Method::GET,
        &format!(
            "/api/v1/cameras/{device}/recordings/export{}",
            range_query(from, to)
        ),
    )
    .await
}

/// `DELETE /api/v1/cameras/{device}/recordings` — every segment starting
/// in the window; returns `{"deleted": n}`.
pub async fn delete_recordings(
    device: i64,
    from: &str,
    to: &str,
) -> Result<serde_json::Value, ApiError> {
    client::request(
        reqwest::Method::DELETE,
        &format!(
            "/api/v1/cameras/{device}/recordings{}",
            range_query(from, to)
        ),
        None,
    )
    .await
}

/// `GET /api/v1/cameras/{device}/recordings/layers` — annotation layers
/// (detection nodes) with boxes in the window.
pub async fn annotation_layers(
    device: i64,
    from: &str,
    to: &str,
) -> Result<Vec<AnnotationLayer>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!(
            "/api/v1/cameras/{device}/recordings/layers{}",
            range_query(from, to)
        ),
        None,
    )
    .await
}

/// `GET /api/v1/cameras/{device}/recordings/annotations` — boxes of the
/// selected layers in the window, ascending time.
pub async fn annotations(
    device: i64,
    from: &str,
    to: &str,
    layers: &[String],
) -> Result<Vec<FrameAnnotation>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!(
            "/api/v1/cameras/{device}/recordings/annotations{}&layers={}",
            range_query(from, to),
            urlencode(&layers.join(","))
        ),
        None,
    )
    .await
}

/// `/ws/camera/live` URL of one camera (D75): API base with the scheme
/// swapped (http → ws, https → wss) + a one-time ticket and the device pk
/// (browsers cannot set headers on a WebSocket; a ticket instead of the
/// access token keeps the JWT out of URLs). `None` when no ticket could be
/// obtained (no session, no org, server unreachable).
pub async fn live_ws_url(device: i64) -> Option<String> {
    let ticket = ws_ticket().await.ok()?;
    let base = crate::api::config::api_base();
    Some(format!(
        "{}/ws/camera/live?ticket={}&device={device}",
        ws_base(&base),
        urlencode(&ticket)
    ))
}

#[derive(serde::Deserialize)]
struct WsTicket {
    ticket: String,
}

/// `POST /api/v1/ws-ticket` — one-time ticket of the current org (valid
/// 60 s, consumed by the first socket that uses it).
async fn ws_ticket() -> Result<String, ApiError> {
    client::request::<WsTicket>(reqwest::Method::POST, "/api/v1/ws-ticket", None)
        .await
        .map(|t| t.ticket)
}

/// `http(s)://host` → `ws(s)://host` (base without a trailing slash).
fn ws_base(api_base: &str) -> String {
    if let Some(rest) = api_base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = api_base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        api_base.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_base_swaps_scheme() {
        assert_eq!(ws_base("https://edge.local:8443"), "wss://edge.local:8443");
        assert_eq!(ws_base("http://localhost:5150"), "ws://localhost:5150");
    }

    #[test]
    fn settings_patch_skips_unset_fields() {
        let patch = SettingsPatch {
            fps: Some(10),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&patch).unwrap(),
            serde_json::json!({"fps": 10})
        );
    }
}
