//! OTA deployment endpoints:
//! - user-facing assignment lifecycle on a device (OrgContext):
//!   POST create / GET inspect / DELETE cancel — desired state, created
//!   offline-safe (delivered at the next announce, not a 409 like pins
//!   commands);
//! - the device-token download route — same auth posture as `/ws/device`
//!   (`authenticate_device`), works on the `db` ArtifactStore backend
//!   (sqlite-on-Pi reference) and on `s3`, no presigned URLs.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use loco_rs::prelude::*;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;
use uuid::Uuid;

use pnex_core::ServerMsg;

use crate::auth::OrgContext;
use crate::controllers::ws_device;
use crate::models::_entities::{build_records, device_registries, ota_assignments};
use crate::services::{firmware, ota};

fn bad_request_code(code: &str, msg: &str) -> Error {
    Error::CustomError(
        axum::http::StatusCode::BAD_REQUEST,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

fn conflict_code(code: &str, msg: &str) -> Error {
    Error::CustomError(
        axum::http::StatusCode::CONFLICT,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

fn not_found_code(code: &str, msg: &str) -> Error {
    Error::CustomError(
        axum::http::StatusCode::NOT_FOUND,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

fn assignment_dto(a: &ota_assignments::Model) -> serde_json::Value {
    serde_json::json!({
        "id": a.id,
        "device_registry_id": a.device_registry_id,
        "target_version": a.target_version,
        "state": a.state,
        "progress": a.progress,
        "error": a.error,
        "created_at": a.created_at.to_rfc3339(),
        "updated_at": a.updated_at.to_rfc3339(),
    })
}

// ─────────────────── Routes + handlers ───────────────────

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/devices/{id}/ota", post(deploy))
        .add("/devices/{id}/ota", get(assignment_status))
        .add("/devices/{id}/ota", delete(cancel))
        .add(
            "/ota/firmware/{device_id}/{version}",
            get(download_firmware),
        )
}

// ─────────────────── POST /devices/{id}/ota ───────────────────

#[derive(Debug, Deserialize)]
struct DeployBody {
    /// Target build version (record id string) — None = latest deployable
    /// build (successful + OTA-stamped).
    version: Option<String>,
    /// Allow redeploying the version the device already runs.
    force: Option<bool>,
}

/// Resolve the target build record: explicit version → that record (org +
/// device + OTA-stamped), else the latest deployable one.
async fn resolve_target(
    db: &DatabaseConnection,
    org_id: i64,
    device: &device_registries::Model,
    version: Option<&str>,
) -> Result<build_records::Model> {
    let record = build_records::Entity::find()
        .filter(build_records::Column::OrgId.eq(org_id))
        .filter(build_records::Column::DeviceId.eq(&device.device_id))
        .filter(build_records::Column::BuildPhase.eq(crate::services::firmware::PHASE_SUCCEEDED))
        .filter(build_records::Column::FwVersion.is_not_null())
        .filter(build_records::Column::OtaSha256.is_not_null())
        .order_by_desc(build_records::Column::Id)
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    match version {
        Some(v) => record
            .into_iter()
            .find(|r| r.fw_version.as_deref() == Some(v))
            .ok_or_else(|| {
                not_found_code("version_not_found", "no deployable build for this version")
            }),
        None => record.into_iter().next().ok_or_else(|| {
            bad_request_code(
                "no_ota_artifact",
                "no deployable build (build the firmware with an OTA-stamped server first)",
            )
        }),
    }
}

/// Create an OTA assignment (desired state). Gates in order: device →
/// single active → ota_ready → target artifact → same-version → size
/// guard. Offline is NOT an error: the row stays pending and is pushed at
/// the next announce (response carries `pushed: false`).
async fn deploy(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    body: String,
) -> Result<Response> {
    // Reflashing a device is a write (SEC-7): viewers are read-only.
    if !org.can_write() {
        return Err(write_forbidden());
    }
    let body: DeployBody = if body.trim().is_empty() {
        DeployBody {
            version: None,
            force: None,
        }
    } else {
        serde_json::from_str(&body).map_err(|e| bad_request_code("invalid_body", &e.to_string()))?
    };
    let device = device_registries::Entity::find_by_id(id)
        .filter(device_registries::Column::OrgId.eq(org.org.id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or_else(|| not_found_code("device_not_found", "unknown device"))?;
    // Edge agent (D95): no firmware, hence no OTA.
    if super::edge_agents::is_agent(&ctx.db, &device).await {
        return Err(super::edge_agents::unsupported_action());
    }
    // Single active assignment per device: fast-path check here; the
    // partial unique index `uq_ota_assignments_one_active` arbitrates
    // concurrent creates (two pods, double click) at insert time.
    if ota::newest_active(&ctx.db, device.id).await?.is_some() {
        return Err(conflict_code(
            "ota_in_progress",
            "an OTA deployment is already in progress for this device",
        ));
    }
    let Some(true) = device.ota_ready else {
        return Err(bad_request_code(
            "ota_not_ready",
            "device firmware predates OTA support — one USB (Web Serial) flash of an OTA build required",
        ));
    };
    let target = resolve_target(&ctx.db, org.org.id, &device, body.version.as_deref()).await?;
    // Same-version guard: numeric compare against the announced version.
    let force = body.force.unwrap_or(false);
    if !force
        && device.fw_version.as_deref().is_some_and(|fw| {
            pnex_core::fw_at_least(fw, &target.fw_version.clone().unwrap_or_default())
        })
    {
        return Err(conflict_code(
            "same_version",
            "device already runs at least the target version (force to redeploy)",
        ));
    }
    // Size guard: the 8266 eboot staging area caps at ~2 MB free after the
    // current sketch (~450 KB app on 4m1m → comfortable, still checked).
    let size = target.ota_size_bytes.unwrap_or(0);
    if device.soc.as_deref() == Some("esp8266") && size > 2_000_000 {
        return Err(bad_request_code(
            "ota_too_large",
            "image exceeds the ESP8266 OTA staging budget (~2 MB)",
        ));
    }
    // Artifact must exist in the store (db/s3) — D6 retention keeps it.
    let key = pnex_firmware_builder::ota_artifact_key(
        org.org.id,
        &device.device_id,
        target.fw_version.as_deref().unwrap_or_default(),
    );
    let settings = firmware::FirmwareSettings::from_config(&ctx.config);
    let store = settings
        .store(&ctx.db)
        .map_err(|_| Error::InternalServerError)?;
    let exists = store
        .exists(&key)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if !exists {
        return Err(bad_request_code(
            "no_ota_artifact",
            "OTA artifact missing from the store",
        ));
    }
    let cmd_id = Uuid::new_v4().simple().to_string();
    let row = ota_assignments::ActiveModel {
        org_id: Set(org.org.id),
        device_registry_id: Set(device.id),
        target_version: Set(target.fw_version.clone().unwrap_or_default()),
        artifact_key: Set(key),
        sha256: Set(target.ota_sha256.clone().unwrap_or_default()),
        size_bytes: Set(target.ota_size_bytes),
        state: Set(ota::ST_PENDING.to_string()),
        progress: Set(None),
        error: Set(None),
        cmd_id: Set(Some(cmd_id)),
        ..Default::default()
    };
    let row = row.insert(&ctx.db).await.map_err(|e| {
        if crate::services::db_lock::is_unique_violation(&e) {
            conflict_code(
                "ota_in_progress",
                "an OTA deployment is already in progress for this device",
            )
        } else {
            Error::InternalServerError
        }
    })?;
    let mut payload = assignment_dto(&row);
    payload["pushed"] = serde_json::json!(false);
    // Online → push immediately; offline → pickup at the next announce.
    if ws_device::is_connected(device.id).await {
        let sig = crate::services::ota_signing::sign_for_order(
            &ctx.db,
            &ctx.config,
            &device.device_id,
            &row.target_version,
            &row.sha256,
        )
        .await;
        if let Some(sig) = sig {
            ws_device::push_command(
                device.id,
                ServerMsg::OtaAvailable {
                    cmd_id: row.cmd_id.clone().unwrap_or_default(),
                    version: row.target_version.clone(),
                    url: format!(
                        "/api/v1/ota/firmware/{}/{}",
                        device.device_id, row.target_version
                    ),
                    sha256: row.sha256.clone(),
                    size: row.size_bytes.map(|s| s as u64),
                    sig,
                },
            )
            .await;
            payload["pushed"] = serde_json::json!(true);
        }
    }
    Ok((StatusCode::CREATED, format::json(payload)).into_response())
}

// ─────────────────── GET /devices/{id}/ota ───────────────────

/// Current assignment + recent history for one device.
async fn assignment_status(
    Path(id): Path<i64>,
    State(ctx): State<AppContext>,
    org: OrgContext,
) -> Result<Response> {
    let device = device_registries::Entity::find_by_id(id)
        .filter(device_registries::Column::OrgId.eq(org.org.id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or_else(|| not_found_code("device_not_found", "unknown device"))?;
    // Only the 10 newest rows are loaded (the history grows forever); the
    // current assignment is the newest non-terminal row, looked up apart
    // when it is not among them.
    use sea_orm::QuerySelect;
    let rows = ota_assignments::Entity::find()
        .filter(ota_assignments::Column::DeviceRegistryId.eq(device.id))
        .order_by_desc(ota_assignments::Column::Id)
        .limit(10)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let current = match rows.iter().find(|r| !ota::is_terminal(&r.state)) {
        Some(r) => Some(assignment_dto(r)),
        None => ota::newest_active(&ctx.db, device.id)
            .await?
            .as_ref()
            .map(assignment_dto),
    };
    let history: Vec<serde_json::Value> = rows.iter().map(assignment_dto).collect();
    Ok(format::json(serde_json::json!({
        "current": current,
        "history": history,
    }))
    .into_response())
}

/// 403 of the OTA write routes (same code as the other device writes).
fn write_forbidden() -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new(
            pnex_core::err_codes::DEVICE_WRITE_FORBIDDEN,
            "Owner, admin or member role required to manage devices.".to_string(),
        ),
    )
}

// ─────────────────── DELETE /devices/{id}/ota ───────────────────

/// Cancel the active assignment (→ failed "cancelled" + journal).
async fn cancel(
    Path(id): Path<i64>,
    State(ctx): State<AppContext>,
    org: OrgContext,
) -> Result<Response> {
    if !org.can_write() {
        return Err(write_forbidden());
    }
    let device = device_registries::Entity::find_by_id(id)
        .filter(device_registries::Column::OrgId.eq(org.org.id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or_else(|| not_found_code("device_not_found", "unknown device"))?;
    let Some(row) = ota::newest_active(&ctx.db, device.id).await? else {
        return Err(conflict_code("ota_no_active", "no active OTA deployment"));
    };
    let row = ota::transition(
        &ctx.db,
        row,
        &device.device_id,
        ota::ST_FAILED,
        None,
        Some("cancelled"),
    )
    .await?;
    Ok(format::json(assignment_dto(&row)).into_response())
}

// ─────────────────── GET /ota/firmware/{device_id}/{version} ───────────────────

/// Device-token authenticated download (same posture as /ws/device auth).
/// Byte-proxy of the versioned OTA artifact.
async fn download_firmware(
    State(ctx): State<AppContext>,
    Path((device_id_str, version)): Path<(String, String)>,
    headers: axum::http::HeaderMap,
) -> Result<Response> {
    let ingest = crate::services::settings::IngestSettings::from_config(&ctx.config);
    if !super::ws_ingest::arrived_over_tls(&headers, &ingest) {
        return Err(not_found_code("auth_failed", "authentication failed"));
    }
    // The token designates the device; the path must name that same one.
    let device = super::ws_ingest::authenticate_device(&ctx.db, &headers, &ingest)
        .await
        .map_err(|_| not_found_code("auth_failed", "authentication failed"))?
        .device;
    if device.device_id != device_id_str {
        return Err(not_found_code("auth_failed", "authentication failed"));
    }
    // Artifact key is org-scoped: the authenticated device's own org.
    let key = pnex_firmware_builder::ota_artifact_key(device.org_id, &device.device_id, &version);
    let settings = firmware::FirmwareSettings::from_config(&ctx.config);
    let store = settings
        .store(&ctx.db)
        .map_err(|_| Error::InternalServerError)?;
    let bytes = store
        .get(&key)
        .await
        .map_err(|_| not_found_code("artifact_not_found", "artifact missing"))?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    )
        .into_response())
}
