//! OTA deployment endpoints:
//! - user-facing assignment lifecycle on a device (OrgContext):
//!   POST create / GET inspect / DELETE cancel — desired state, created
//!   offline-safe (delivered at the next announce, not a 409 like pins
//!   commands);
//! - the device-token download route — same auth posture as `/ws/device`
//!   (`authenticate_device`), works on the `db` ArtifactStore backend and
//!   on `s3`, no presigned URLs.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use loco_rs::prelude::*;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;
use uuid::Uuid;

use pnex_core::{err_codes, ServerMsg};

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

/// 404 `device-not-found` (registry id outside the org).
fn device_not_found(id: i64) -> Error {
    super::coded_error(
        StatusCode::NOT_FOUND,
        err_codes::DEVICE_NOT_FOUND,
        "unknown device",
        Some(serde_json::json!({ "device_id": id.to_string() })),
    )
}

/// Lifecycle state of an OTA assignment (lowercase on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
enum OtaState {
    Pending,
    Downloading,
    Flashing,
    Succeeded,
    Failed,
}

impl OtaState {
    fn from_wire(s: &str) -> Self {
        match s {
            ota::ST_PENDING => Self::Pending,
            ota::ST_DOWNLOADING => Self::Downloading,
            ota::ST_FLASHING => Self::Flashing,
            ota::ST_SUCCEEDED => Self::Succeeded,
            // Only the five states are ever written; anything else is a failure.
            _ => Self::Failed,
        }
    }
}

/// An OTA assignment (desired firmware version of a device).
#[derive(serde::Serialize, utoipa::ToSchema)]
struct OtaAssignmentDto {
    id: i64,
    device_registry_id: i64,
    target_version: String,
    /// Lifecycle state (pending, then device-reported states; terminal: succeeded | failed).
    state: OtaState,
    /// Download progress in percent, when reported.
    progress: Option<i32>,
    /// Failure reason (`cancelled` on cancel).
    error: Option<String>,
    /// RFC 3339.
    #[schema(format = DateTime)]
    created_at: String,
    /// RFC 3339.
    #[schema(format = DateTime)]
    updated_at: String,
}

/// Answer of `POST /api/v1/devices/{id}/ota`.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct OtaDeployed {
    #[serde(flatten)]
    assignment: OtaAssignmentDto,
    /// The order was pushed to the connected device now; false = delivered at its next announce.
    pushed: bool,
}

/// Answer of `GET /api/v1/devices/{id}/ota`.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct OtaStatus {
    /// Active (non-terminal) assignment, if any.
    current: Option<OtaAssignmentDto>,
    /// The 10 newest assignments, newest first.
    history: Vec<OtaAssignmentDto>,
}

fn assignment_dto(a: &ota_assignments::Model) -> OtaAssignmentDto {
    OtaAssignmentDto {
        id: a.id,
        device_registry_id: a.device_registry_id,
        target_version: a.target_version.clone(),
        state: OtaState::from_wire(&a.state),
        progress: a.progress,
        error: a.error.clone(),
        created_at: a.created_at.to_rfc3339(),
        updated_at: a.updated_at.to_rfc3339(),
    }
}

// ─────────────────── Routes + handlers ───────────────────

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/devices/{id}/ota", post(deploy))
        .add("/devices/{id}/ota", get(assignment_status))
        .add("/devices/{id}/ota", delete(cancel))
        .add("/ota/firmware/{version}", get(download_firmware))
}

// ─────────────────── POST /devices/{id}/ota ───────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
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
                not_found_code(
                    err_codes::OTA_VERSION_NOT_FOUND,
                    "no deployable build for this version",
                )
            }),
        None => record.into_iter().next().ok_or_else(|| {
            bad_request_code(
                err_codes::OTA_NO_ARTIFACT,
                "no deployable build (build the firmware with an OTA-stamped server first)",
            )
        }),
    }
}

/// Create an OTA assignment (desired state). Gates in order: device →
/// single active → ota_ready → target artifact → same-version → size
/// guard. Offline is NOT an error: the row stays pending and is pushed at
/// the next announce (response carries `pushed: false`).
#[utoipa::path(
    post,
    path = "/api/v1/devices/{id}/ota",
    tag = "devices",
    operation_id = "devices_ota_deploy",
    summary = "Deploy a firmware update over the air",
    description = "Creates an OTA assignment (desired state) for the device: the given `version` (a successful OTA-stamped build of this device) or the latest deployable one. Pushed immediately to a connected device (signed order), otherwise delivered at its next announce. The device then reflashes itself. One active assignment per device. Body optional (empty = latest, no force). Owner, admin or member.",
    params(("id" = i64, Path, description = "Device registry id")),
    request_body = DeployBody,
    responses(
        (status = 201, body = OtaDeployed),
        (status = 400, description = "Invalid body (`field-invalid`), device not OTA-ready (`ota-not-ready`), no deployable artifact (`ota-no-artifact`), image too large (`ota-too-large`), edge agent device (`agent-unsupported-action`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Viewer (`device-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown device or version (`device-not-found`, `ota-version-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Deployment already in progress (`ota-in-progress`) or same version without force (`ota-same-version`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
        match serde_json::from_str(&body) {
            Ok(body) => body,
            Err(_) => return Ok(super::field_error("body", err_codes::FIELD_INVALID)),
        }
    };
    let device = device_registries::Entity::find_by_id(id)
        .filter(device_registries::Column::OrgId.eq(org.org.id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or_else(|| device_not_found(id))?;
    // Edge agent (D95): no firmware, hence no OTA.
    if super::edge_agents::is_agent(&ctx.db, &device).await {
        return Err(super::edge_agents::unsupported_action());
    }
    // Single active assignment per device: fast-path check here; the
    // partial unique index `uq_ota_assignments_one_active` arbitrates
    // concurrent creates (two pods, double click) at insert time.
    if ota::newest_active(&ctx.db, device.id).await?.is_some() {
        return Err(conflict_code(
            err_codes::OTA_IN_PROGRESS,
            "an OTA deployment is already in progress for this device",
        ));
    }
    let Some(true) = device.ota_ready else {
        return Err(bad_request_code(
            err_codes::OTA_NOT_READY,
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
            err_codes::OTA_SAME_VERSION,
            "device already runs at least the target version (force to redeploy)",
        ));
    }
    // Size guard: the 8266 eboot staging area caps at ~2 MB free after the
    // current sketch (~450 KB app on 4m1m → comfortable, still checked).
    let size = target.ota_size_bytes.unwrap_or(0);
    if device.soc.as_deref() == Some("esp8266") && size > 2_000_000 {
        return Err(bad_request_code(
            err_codes::OTA_TOO_LARGE,
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
    let store = settings.store().map_err(|_| Error::InternalServerError)?;
    let exists = store
        .exists(&key)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if !exists {
        return Err(bad_request_code(
            err_codes::OTA_NO_ARTIFACT,
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
                err_codes::OTA_IN_PROGRESS,
                "an OTA deployment is already in progress for this device",
            )
        } else {
            Error::InternalServerError
        }
    })?;
    let mut payload = OtaDeployed {
        assignment: assignment_dto(&row),
        pushed: false,
    };
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
                    url: format!("/api/v1/ota/firmware/{}", row.target_version),
                    sha256: row.sha256.clone(),
                    size: row.size_bytes.map(|s| s as u64),
                    sig,
                },
            )
            .await;
            payload.pushed = true;
        }
    }
    Ok((StatusCode::CREATED, format::json(payload)).into_response())
}

// ─────────────────── GET /devices/{id}/ota ───────────────────

/// Current assignment + recent history for one device.
#[utoipa::path(
    get,
    path = "/api/v1/devices/{id}/ota",
    tag = "devices",
    operation_id = "devices_ota_status",
    summary = "OTA state of a device",
    description = "Current (non-terminal) assignment and the 10 newest assignments. Any org member.",
    params(("id" = i64, Path, description = "Device registry id")),
    responses(
        (status = 200, body = OtaStatus),
        (status = 404, description = "Unknown device (`device-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
        .ok_or_else(|| device_not_found(id))?;
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
    let history: Vec<OtaAssignmentDto> = rows.iter().map(assignment_dto).collect();
    Ok(format::json(OtaStatus { current, history }).into_response())
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
#[utoipa::path(
    delete,
    path = "/api/v1/devices/{id}/ota",
    tag = "devices",
    operation_id = "devices_ota_cancel",
    summary = "Cancel the active OTA deployment",
    description = "Marks the active assignment failed with error `cancelled` and journals it. Returns the updated assignment. Owner, admin or member.",
    params(("id" = i64, Path, description = "Device registry id")),
    responses(
        (status = 200, body = OtaAssignmentDto),
        (status = 403, description = "Viewer (`device-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown device (`device-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "No active deployment (`ota-no-active`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
        .ok_or_else(|| device_not_found(id))?;
    let Some(row) = ota::newest_active(&ctx.db, device.id).await? else {
        return Err(conflict_code(
            err_codes::OTA_NO_ACTIVE,
            "no active OTA deployment",
        ));
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

// ─────────────────── GET /ota/firmware/{version} ───────────────────

/// Device-token authenticated download (same posture as /ws/device auth).
/// Byte-proxy of the versioned OTA artifact of the token's own device.
#[utoipa::path(
    get,
    path = "/api/v1/ota/firmware/{version}",
    tag = "ota",
    operation_id = "ota_firmware_download",
    summary = "Download an OTA image (device only)",
    description = "Called by the device itself during an OTA (the URL comes from the signed `ota_available` order): raw firmware image of the given version of the device the token designates. Authenticated with the device token (`Authorization: Bearer <device token>`, not a user JWT) plus its TLS client certificate, through the TLS edge only. Every authentication failure answers 404 `ota-auth-failed`.",
    params(("version" = String, Path, description = "Firmware version (build record id)")),
    responses(
        (status = 200, content_type = "application/octet-stream", body = Vec<u8>),
        (status = 404, description = "Authentication failed (`ota-auth-failed`) or artifact missing (`ota-artifact-not-found`)", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn download_firmware(
    State(ctx): State<AppContext>,
    Path(version): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<Response> {
    let ingest = crate::services::settings::IngestSettings::from_config(&ctx.config);
    let auth_failed = || not_found_code(err_codes::OTA_AUTH_FAILED, "authentication failed");
    if !super::device_link::arrived_over_tls(&headers, &ingest) {
        return Err(auth_failed());
    }
    // The token designates the device: only its own artifacts are served.
    let device = super::device_link::authenticate_device(&ctx.db, &headers, &ingest)
        .await
        .map_err(|_| auth_failed())?
        .device;
    // Artifact key is org-scoped: the authenticated device's own org.
    let key = pnex_firmware_builder::ota_artifact_key(device.org_id, &device.device_id, &version);
    let settings = firmware::FirmwareSettings::from_config(&ctx.config);
    let store = settings.store().map_err(|_| Error::InternalServerError)?;
    let bytes = store
        .get(&key)
        .await
        .map_err(|_| not_found_code(err_codes::OTA_ARTIFACT_NOT_FOUND, "artifact missing"))?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    )
        .into_response())
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(deploy, assignment_status, cancel, download_firmware))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
