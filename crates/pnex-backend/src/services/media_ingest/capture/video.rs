//! Video frames of a stream (media-ingest.md D175): [`publish`] puts each
//! frame cut by [`JpegSplitter`] on the camera bus of the stream (`SET`
//! with the frame TTL + `PUBLISH` of a `BusFrameMeta`), in the stream key
//! space — never a device key.

use pnex_core::camera::{BusFrameMeta, FRAME_TTL_SECS};
pub use pnex_media_capture::video::*;
use redis::aio::ConnectionManager;

/// Publishes one frame of stream `slug` on the camera bus (D74 format,
/// D175 key space). Best effort: a Valkey error is logged by the caller.
pub async fn publish(
    conn: &mut ConnectionManager,
    org_id: i64,
    slug: &str,
    seq: u32,
    frame: &Jpeg,
) -> redis::RedisResult<()> {
    let key = pnex_core::media_ingest::frame_key(org_id, slug, seq);
    let meta = BusFrameMeta {
        seq,
        ts_ms: chrono::Utc::now().timestamp_millis(),
        width: frame.width,
        height: frame.height,
        size: frame.bytes.len() as u32,
        key: key.clone(),
    };
    let meta = serde_json::to_string(&meta).unwrap_or_default();
    redis::pipe()
        .set_ex(&key, frame.bytes.as_slice(), FRAME_TTL_SECS)
        .ignore()
        .publish(pnex_core::media_ingest::frame_channel(org_id, slug), meta)
        .ignore()
        .query_async(conn)
        .await
}
