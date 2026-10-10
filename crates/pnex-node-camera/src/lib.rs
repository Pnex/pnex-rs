//! Camera flow nodes (camera-video.md D78):
//!
//! - `pnex-camera-source` — first **event-driven** source node: subscribes
//!   to the camera frame bus of one device (Valkey pub/sub fed by the
//!   backend CameraHub) and emits one message per frame (sampled by
//!   `max_fps`). Messages carry a reference (`payload.frame_key`, Valkey
//!   TTL 15 s), never the JPEG bytes.
//! - `pnex-video-record` — accumulates frames into MJPEG-AVI segments (one
//!   buffer per camera) and uploads each segment to the backend
//!   (`/internal/flow/video-segment`), which owns the MediaStore (fs | s3)
//!   and the `video_segments` rows. The runtime holds no storage secret.
//! - `pnex-media-source` — event source of transcribed media segments
//!   (media-ingest.md D163); lives here to share the Valkey bus plumbing.
//! - `pnex-topic-classify` — keyword topic classifier of transcribed text
//!   (media-ingest.md D168), the media node next to `media-source`.
//! - `pnex-range-upsert` — writes time ranges through the backend
//!   (media-ingest.md D169), the third media node.

mod media;
mod ranges;
mod record;
mod source;
pub mod status;
mod topics;

use edgelink_core::runtime::model::Variant;

/// Anti-stripping anchor: without a reference to the crate the linker may
/// drop the `inventory` submissions (same guard as the runtime binary calls).
pub fn registered() {}

/// Reads the `VALKEY_URL` injected by the supervisor — camera and media
/// nodes need the bus: absent = explicit build error (deploy preflight fails loud).
fn valkey_client(node: &str) -> edgelink_core::Result<redis::Client> {
    let url = std::env::var("VALKEY_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| {
            edgelink_core::EdgelinkError::InvalidOperation(format!(
                "{node} [camera_bus_unavailable] : VALKEY_URL is not set in the runtime \
                 environment (the camera / media event bus needs Valkey)"
            ))
        })?;
    redis::Client::open(url.as_str()).map_err(|e| {
        edgelink_core::EdgelinkError::InvalidOperation(format!("{node} : invalid VALKEY_URL: {e}"))
            .into()
    })
}

/// JSON object → message variant (built through `From<serde_json::Value>`,
/// never through the generic serde bridge: arbitrary_precision would turn
/// numbers into magic maps).
fn variant_of(v: serde_json::Value) -> Variant {
    Variant::from(v)
}

/// Sampling gate: `true` when a message at `now_ms` may pass given the last
/// accepted one and a max rate (0 = no limit).
pub(crate) fn sample_ok(max_fps: f64, last_ms: Option<i64>, now_ms: i64) -> bool {
    if max_fps <= 0.0 {
        return true;
    }
    let min_gap = (1000.0 / max_fps) as i64;
    match last_ms {
        None => true,
        // 10 % tolerance so a camera at exactly `max_fps` is not halved by
        // clock jitter.
        Some(last) => now_ms - last >= min_gap - min_gap / 10,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling() {
        assert!(sample_ok(0.0, Some(0), 1));
        assert!(sample_ok(1.0, None, 0));
        assert!(!sample_ok(1.0, Some(0), 500));
        assert!(sample_ok(1.0, Some(0), 950));
        assert!(sample_ok(2.0, Some(0), 500));
    }
}
