//! Capture boxes (media-ingest.md D159, D160, lot 6b): edge agents that
//! announced `media_capture` and capture the `capture_on = device:<id>`
//! streams. The box gets its stream list and uploads its segments over
//! `/ws/media` (controller `ws_media`); org and device always come from the
//! device identity, never from what the box sends (R1).

use std::collections::HashMap;

use pnex_core::media_ingest::{
    CaptureOn, CaptureState, DeviceStream, MediaCaptureDevice, MediaStreamKind, MediaTracks,
    CAPTURE_ERROR_DEVICE_OFFLINE, MEDIA_CAPTURE_CAP,
};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder};
use uuid::Uuid;

use crate::models::_entities::{device_registries, media_streams, predefined_devices};
use crate::services::secrets::{store as secret_store, Keyring};

/// Whether an announce manifest (`device_registries.announced_caps`)
/// carries the `media_capture` capability.
pub fn announced_media_capture(caps: Option<&serde_json::Value>) -> bool {
    caps.and_then(|v| v.as_array()).is_some_and(|caps| {
        caps.iter()
            .any(|c| c.get("id").and_then(|id| id.as_str()) == Some(MEDIA_CAPTURE_CAP))
    })
}

/// Edge agents of the org (any capability).
async fn agents<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
) -> Result<Vec<device_registries::Model>, DbErr> {
    let Some(agent) = predefined_devices::Entity::find()
        .filter(predefined_devices::Column::Name.eq(pnex_core::EDGE_AGENT_PREDEF))
        .one(db)
        .await?
    else {
        return Ok(Vec::new());
    };
    device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org_id))
        .filter(device_registries::Column::PredefinedDeviceId.eq(agent.id))
        .order_by_asc(device_registries::Column::DeviceId)
        .all(db)
        .await
}

/// Capture boxes of the org: agents that announced `media_capture`.
pub async fn list<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
) -> Result<Vec<MediaCaptureDevice>, DbErr> {
    Ok(agents(db, org_id)
        .await?
        .into_iter()
        .filter(|d| announced_media_capture(d.announced_caps.as_ref()))
        .map(|d| MediaCaptureDevice {
            id: d.id,
            name: d.device_id,
        })
        .collect())
}

/// Device `device_pk` is a capture box of the org (same answer for another
/// org's device and an unknown id: no oracle).
pub async fn is_capture_box<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    device_pk: i64,
) -> Result<bool, DbErr> {
    Ok(list(db, org_id).await?.iter().any(|d| d.id == device_pk))
}

/// Names of the boxes behind `device:<id>` carriers, for the read DTO.
pub async fn names<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    rows: &[media_streams::Model],
) -> Result<HashMap<i64, String>, DbErr> {
    let ids: Vec<i64> = rows
        .iter()
        .filter_map(|r| match CaptureOn::from_wire(&r.capture_on) {
            Some(CaptureOn::Device(id)) => Some(id),
            _ => None,
        })
        .collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org_id))
        .filter(device_registries::Column::Id.is_in(ids))
        .all(db)
        .await?
        .into_iter()
        .map(|d| (d.id, d.device_id))
        .collect())
}

/// Live streams captured by box `device` (its org only).
fn select(device: &device_registries::Model) -> sea_orm::Select<media_streams::Entity> {
    media_streams::Entity::find()
        .filter(media_streams::Column::OrgId.eq(device.org_id))
        .filter(media_streams::Column::CaptureOn.eq(CaptureOn::Device(device.id).wire()))
        .filter(media_streams::Column::DeletedAt.is_null())
}

/// The stream `stream_id` when box `device` captures it; `None` otherwise
/// (another org, another carrier, deleted or unknown).
pub async fn stream_of<C: ConnectionTrait>(
    db: &C,
    device: &device_registries::Model,
    stream_id: Uuid,
) -> Result<Option<media_streams::Model>, DbErr> {
    select(device)
        .filter(media_streams::Column::Id.eq(stream_id))
        .one(db)
        .await
}

/// Stream list pushed to box `device`. A stream is `enabled` for the box
/// when the server supervisor would capture it (profile set: an audio
/// stream without one is never transcribed). The secret of a stream goes
/// with it: it is the stream's own access credential, revealed in memory
/// for this box's authenticated link only (SEC-27), never logged.
pub async fn streams_for<C: ConnectionTrait>(
    db: &C,
    ring: Option<&Keyring>,
    device: &device_registries::Model,
) -> Result<Vec<DeviceStream>, DbErr> {
    let rows = select(device)
        .order_by_asc(media_streams::Column::Id)
        .all(db)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let auth_secret = match (r.secret_id, ring) {
            (Some(id), Some(ring)) => {
                match secret_store::reveal(db, ring, Some(r.org_id), id).await {
                    Ok(v) => Some(v),
                    Err(e) => {
                        tracing::warn!(stream = %r.slug, error = %e, "media stream secret unreadable for its capture box");
                        None
                    }
                }
            }
            _ => None,
        };
        out.push(DeviceStream {
            id: r.id.to_string(),
            slug: r.slug.clone(),
            kind: MediaStreamKind::from_wire(&r.kind).unwrap_or_default(),
            url: r.url.clone(),
            segment_secs: r.segment_secs,
            overlap_secs: r.overlap_secs,
            tracks: MediaTracks::from_wire(&r.tracks).unwrap_or_default(),
            fps: r.fps,
            enabled: r.enabled && r.asr_profile_id.is_some(),
            auth_secret,
        });
    }
    Ok(out)
}

/// Writes the capture state a box reports for one of its streams; never
/// touches `updated_at` (the box's config fingerprint). `error` is an
/// already validated capture error code.
pub async fn report_state<C: ConnectionTrait>(
    db: &C,
    device: &device_registries::Model,
    stream_id: Uuid,
    state: CaptureState,
    error: Option<&str>,
) -> Result<(), DbErr> {
    media_streams::Entity::update_many()
        .col_expr(
            media_streams::Column::CaptureState,
            Expr::value(state.wire()),
        )
        .col_expr(
            media_streams::Column::CaptureError,
            Expr::value(error.map(str::to_string)),
        )
        .col_expr(
            media_streams::Column::CaptureChangedAt,
            Expr::value(Some(sea_orm::prelude::DateTimeWithTimeZone::from(
                chrono::Utc::now(),
            ))),
        )
        .filter(media_streams::Column::OrgId.eq(device.org_id))
        .filter(media_streams::Column::CaptureOn.eq(CaptureOn::Device(device.id).wire()))
        .filter(media_streams::Column::Id.eq(stream_id))
        .exec(db)
        .await?;
    Ok(())
}

/// The box dropped its media link: its enabled streams read `backoff` +
/// `device-offline` (the silent-stream alert fires on its own, D160).
pub async fn mark_offline<C: ConnectionTrait>(
    db: &C,
    device: &device_registries::Model,
) -> Result<(), DbErr> {
    let ids: Vec<Uuid> = select(device)
        .filter(media_streams::Column::Enabled.eq(true))
        .all(db)
        .await?
        .into_iter()
        .map(|s| s.id)
        .collect();
    for id in ids {
        report_state(
            db,
            device,
            id,
            CaptureState::Backoff,
            Some(CAPTURE_ERROR_DEVICE_OFFLINE),
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_capture_cap_is_read_from_the_manifest() {
        let caps = serde_json::json!([{ "id": "ota", "family": "feature" }]);
        assert!(!announced_media_capture(Some(&caps)));
        let caps = serde_json::json!([{ "id": "media_capture", "family": "feature" }]);
        assert!(announced_media_capture(Some(&caps)));
        assert!(!announced_media_capture(None));
        assert!(!announced_media_capture(Some(
            &serde_json::json!({"id": "media_capture"})
        )));
    }
}
