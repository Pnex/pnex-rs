//! OTA assignment lifecycle — server-side state machine + watchdog.
//!
//! The assignment is DESIRED state: created offline-safe (the row stays
//! `pending` and is push-commanded at the next announce). The device
//! reports progress (`DeviceMsg::OtaState`), and the post-reboot announce
//! decides `succeeded` by version comparison. Terminal transitions are
//! journaled (best-effort) through the org's in-app channel.

use loco_rs::prelude::*;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};

use crate::models::_entities::{device_registries, notify_channels, ota_assignments};
use crate::services::notify;

// Assignment states (storage strings — wire-compatible with OtaState phase).
pub const ST_PENDING: &str = "pending";
pub const ST_DOWNLOADING: &str = "downloading";
pub const ST_FLASHING: &str = "flashing";
pub const ST_SUCCEEDED: &str = "succeeded";
pub const ST_FAILED: &str = "failed";

pub fn is_terminal(state: &str) -> bool {
    state == ST_SUCCEEDED || state == ST_FAILED
}

/// Newest non-terminal row for a device (the "current assignment") —
/// single-active-row school: the create handler guards the 409.
pub async fn newest_active(
    db: &DatabaseConnection,
    device_registry_id: i64,
) -> Result<Option<ota_assignments::Model>> {
    ota_assignments::Entity::find()
        .filter(ota_assignments::Column::DeviceRegistryId.eq(device_registry_id))
        .filter(ota_assignments::Column::State.is_in([ST_PENDING, ST_DOWNLOADING, ST_FLASHING]))
        .order_by_desc(ota_assignments::Column::Id)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

/// Newest row whatever its state — post-reboot resolution may need to flip
/// a watchdog-failed row back to succeeded (late announce, slow links).
pub async fn newest_for_device(
    db: &DatabaseConnection,
    device_registry_id: i64,
) -> Result<Option<ota_assignments::Model>> {
    ota_assignments::Entity::find()
        .filter(ota_assignments::Column::DeviceRegistryId.eq(device_registry_id))
        .order_by_desc(ota_assignments::Column::Id)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

// ─────────────────── Transitions + journal ───────────────────

/// Assignation result payload for the journal meta. `kind: "ota"` lets
/// structured consumers (future notification feed UI) compose a localized
/// message from the structured fields instead of the English body.
fn meta_for(device_id: &str, target: &str, state: &str, error: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "kind": "ota",
        "device_id": device_id,
        "target_version": target,
        "state": state,
        "error": error,
    })
}

/// Best-effort in-app journal delivery through the org's first websocket
/// channel (no channel configured → debug log only).
async fn journal_terminal(
    db: &DatabaseConnection,
    org_id: i64,
    device_id: &str,
    target: &str,
    state: &str,
    error: Option<&str>,
) {
    let channel = notify_channels::Entity::find()
        .filter(notify_channels::Column::OrgId.eq(org_id))
        .filter(notify_channels::Column::Kind.eq("websocket"))
        .order_by_asc(notify_channels::Column::Id)
        .one(db)
        .await;
    let Ok(Some(channel)) = channel else {
        tracing::debug!("ota journal: no in-app channel for org {org_id}");
        return;
    };
    // Canonical English text — ws_notify has no language context; the
    // structured `meta.kind` + fields are the migration signal for
    // consumers wanting localized composition.
    let (subject, body) = if state == ST_SUCCEEDED {
        (
            Some(format!("Firmware {target} deployed ({device_id})")),
            format!("The OTA update to build {target} succeeded on {device_id}."),
        )
    } else {
        (
            Some(format!("OTA {target} failed ({device_id})")),
            format!(
                "The OTA update to build {target} failed on {device_id}: {}",
                error.unwrap_or("unknown reason")
            ),
        )
    };
    let msg = pnex_notify::Message {
        subject,
        body,
        meta: meta_for(device_id, target, state, error),
    };
    notify::deliver_in_app(
        pnex_core::NotifyDeliveryEntry {
            org_id,
            channel_id: channel.id,
            source: "ota".into(),
            ..Default::default()
        },
        &msg,
    );
}

/// Apply a state transition + journal terminal outcomes. `device_id` is the
/// declared device string (journal message context).
///
/// Conditional: the row is only updated if it is still in the state it was
/// read in (`row.state`). When a concurrent writer (watchdog, cancel,
/// another pod handling the device socket) moved it first, nothing is
/// written, no notification is sent, and the CURRENT row is returned.
/// Callers needing to know whether they won use [`try_transition`].
pub async fn transition(
    db: &DatabaseConnection,
    row: ota_assignments::Model,
    device_id: &str,
    state: &str,
    progress: Option<i32>,
    error: Option<&str>,
) -> Result<ota_assignments::Model> {
    let id = row.id;
    match try_transition(db, row, device_id, state, progress, error).await? {
        Some(updated) => Ok(updated),
        None => ota_assignments::Entity::find_by_id(id)
            .one(db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .ok_or(Error::NotFound),
    }
}

/// Compare-and-set variant of [`transition`]: `Some(updated)` when this call
/// applied the transition (terminal outcomes journaled exactly once),
/// `None` when the row left `row.state` meanwhile (lost race, no side
/// effect).
pub async fn try_transition(
    db: &DatabaseConnection,
    row: ota_assignments::Model,
    device_id: &str,
    state: &str,
    progress: Option<i32>,
    error: Option<&str>,
) -> Result<Option<ota_assignments::Model>> {
    let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
    let res = ota_assignments::Entity::update_many()
        .col_expr(ota_assignments::Column::State, Expr::value(state))
        .col_expr(ota_assignments::Column::Progress, Expr::value(progress))
        .col_expr(
            ota_assignments::Column::Error,
            Expr::value(error.map(str::to_string)),
        )
        .col_expr(ota_assignments::Column::UpdatedAt, Expr::value(now))
        .filter(ota_assignments::Column::Id.eq(row.id))
        .filter(ota_assignments::Column::State.eq(row.state.as_str()))
        .exec(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if res.rows_affected == 0 {
        tracing::debug!(
            assignment = row.id,
            from = %row.state,
            to = state,
            "ota transition skipped: the row moved concurrently"
        );
        return Ok(None);
    }
    let updated = ota_assignments::Model {
        state: state.to_string(),
        progress,
        error: error.map(str::to_string),
        updated_at: now,
        ..row
    };
    if is_terminal(state) {
        journal_terminal(
            db,
            updated.org_id,
            device_id,
            &updated.target_version,
            state,
            updated.error.as_deref(),
        )
        .await;
    }
    Ok(Some(updated))
}

// ─────────────────── Watchdog ───────────────────

/// Stale-assignment sweep: downloading/flashing rows silent for more than
/// 10 minutes are failed("timeout") + journaled. Called every 60 s by the
/// spawned task (app.rs, next to the notify pruner).
pub async fn sweep_stuck(db: &DatabaseConnection) -> Result<()> {
    let cutoff: chrono::DateTime<chrono::FixedOffset> =
        (chrono::Utc::now() - chrono::Duration::minutes(10)).into();
    let stuck = ota_assignments::Entity::find()
        .filter(ota_assignments::Column::State.is_in([ST_DOWNLOADING, ST_FLASHING]))
        .filter(ota_assignments::Column::UpdatedAt.lt(cutoff))
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    for row in stuck {
        let Some(device) = device_registries::Entity::find_by_id(row.device_registry_id)
            .one(db)
            .await
            .ok()
            .flatten()
        else {
            continue;
        };
        tracing::warn!(device = %device.device_id, target = %row.target_version, "OTA timeout");
        transition(db, row, &device.device_id, ST_FAILED, None, Some("timeout")).await?;
    }
    Ok(())
}

/// Spawned once at boot — periodic stuck-assignment sweep.
pub fn spawn_watchdog(ctx: &loco_rs::app::AppContext) {
    let db = ctx.db.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            // One pod sweeps (D106).
            if !crate::services::singleton::my_turn(
                &db,
                "ota-watchdog",
                std::time::Duration::from_secs(60),
            )
            .await
            {
                continue;
            }
            if let Err(e) = sweep_stuck(&db).await {
                tracing::warn!("ota watchdog sweep failed: {e}");
            }
        }
    });
}
