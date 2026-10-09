//! Firmware builds — parity with the legacy `firmware_builder` views (Phase 6),
//! org scoping (D2):
//!
//! - `POST /build-firmware`: field validation → WiFi entry (400) → device
//!   not found (404) → type quota
//!   (403) → min interval between builds (429) → `queued` record → enqueue
//!   (PostgreSQL queue worker). Adapted 201 response: no more
//!   `backend`/`job_name` (no k8s), `build_id` instead;
//! - `GET /build-records`: org-scoped list, paginated (D14), `device_id` /
//!   `build_phase` filters (old records are pruned by the worker,
//!   `build_retention`);
//! - `GET /download/firmware/{device_id}`: proxies the artifact bytes
//!   (legacy parity, no presigned URL), attachment
//!   `{device_id}-firmware.bin`.
//!
//! Errors: machine code + English description (`coded_error`); field
//! validation `{"<field>": "<token>"}`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::bgworker::BackgroundWorker;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use serde::Deserialize;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::{
    build_records, device_registries, device_types, mcu_boards, predefined_devices,
    subscription_tiers,
};
use crate::services::firmware::{
    FirmwareSettings, PHASE_FAILED, PHASE_QUEUED, PHASE_RUNNING, PHASE_SUCCEEDED,
};

/// Build phases accepted by the `?build_phase=` filter.
const PHASES: [&str; 4] = [PHASE_QUEUED, PHASE_RUNNING, PHASE_SUCCEEDED, PHASE_FAILED];
use crate::workers::build_firmware::{BuildFirmwareArgs, BuildFirmwareWorker};
use pnex_core::err_codes;

// ─────────────────────────── Aides ───────────────────────────

/// Field-by-field error: `{"<field>": "<token>"}`.
fn field_status(status: StatusCode, field: &str, msg: &str) -> Response {
    (status, format::json(serde_json::json!({ field: msg }))).into_response()
}

/// Forbidden with the machine code + canonical English description
/// (frontend resolves `err-<code>` at render time, verbatim fallback).
fn forbidden(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

/// Record → DTO `pnex_core::BuildRecord`.
fn record_dto(r: build_records::Model) -> pnex_core::BuildRecord {
    pnex_core::BuildRecord {
        id: r.id,
        org_id: r.org_id,
        device_id: r.device_id,
        build_phase: r.build_phase,
        firmware_bin_s3_key: r.firmware_bin_s3_key,
        fw_version: r.fw_version,
        failure_code: r.failure_code,
        failure_detail: r.failure_detail,
        created_at: r.created_at.to_rfc3339(),
        updated_at: r.updated_at.to_rfc3339(),
    }
}

/// Intervalle min du tier de l'org (None = pas de contrainte).
async fn min_build_interval(db: &DatabaseConnection, org: &OrgContext) -> Result<Option<i64>> {
    if !crate::controllers::devices::tiers_enforced() {
        return Ok(None);
    }
    let Some(tier_id) = org.org.subscription_tier_id else {
        return Ok(None);
    };
    Ok(subscription_tiers::Entity::find_by_id(tier_id)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .map(|t| t.min_build_interval_secs)
        .filter(|s| *s > 0))
}

/// Number of devices of the given type in the org (all states — quota parity
/// with the legacy implementation, cf. devices create).
async fn count_devices_of_type(db: &DatabaseConnection, org_id: i64, type_id: i64) -> Result<i64> {
    let rows = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org_id))
        .find_also_related(predefined_devices::Entity)
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(rows
        .iter()
        .filter(|(_, pd)| pd.as_ref().is_some_and(|p| p.device_type_id == type_id))
        .count() as i64)
}

// ─────────────────────────── POST /build-firmware ───────────────────────────

/// `POST /api/v1/build-firmware` — inserts a new build record (new
/// firmware version) and enqueues the build job.
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<pnex_core::CreateBuild>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "build-create-forbidden",
            "Owner, admin or member role required to launch builds.",
        ));
    }

    // An imposed production host (`PNEX_PROD_HOST`) wins over the request:
    // the client may send it, another host or nothing at all.
    let prod_host = crate::app::prod_host();
    let requested_host = prod_host.as_deref().unwrap_or(params.pnex_host.as_str());

    let checks = [
        ("pnex_host", requested_host.trim(), 200),
        ("device_id", params.device_id.trim(), 100),
    ];
    for (field, value, max) in checks {
        if value.is_empty() {
            return Ok(field_status(
                StatusCode::BAD_REQUEST,
                field,
                err_codes::FIELD_REQUIRED,
            ));
        }
        if value.chars().count() > max {
            return Ok(field_status(
                StatusCode::BAD_REQUEST,
                field,
                &format!("{}:{max}", err_codes::FIELD_MAX_LENGTH),
            ));
        }
    }
    if requested_host.split_whitespace().count() > 1 {
        return Ok(field_status(
            StatusCode::BAD_REQUEST,
            "pnex_host",
            "Must be a host without spaces (e.g. dev1.pnex.io).",
        ));
    }
    // WiFi: the build references the vault secret of a referential entry
    // (secrets.md S6), the password never enters the queue.
    let Some(cred) = crate::models::_entities::wifi_credentials::Entity::find()
        .filter(crate::models::_entities::wifi_credentials::Column::OrgId.eq(org.org.id))
        .filter(
            crate::models::_entities::wifi_credentials::Column::Id.eq(params.wifi_credential_id),
        )
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Ok(field_status(
            StatusCode::BAD_REQUEST,
            "wifi_credential_id",
            "WiFi entry not found in this organization.",
        ));
    };
    let Some(wifi_secret_id) = cred.secret_id else {
        return Ok(field_status(
            StatusCode::BAD_REQUEST,
            "wifi_credential_id",
            err_codes::FIELD_REQUIRED,
        ));
    };
    let wifi_ssid = cred.ssid;
    let device_id = params.device_id.trim().to_string();
    let pnex_host = requested_host.trim().to_string();

    // Device of the org; its model is the firmware workspace project.
    let Some(device) = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org.org.id))
        .filter(device_registries::Column::DeviceId.eq(&device_id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(crate::controllers::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::DEVICE_NOT_FOUND,
            format!("Device with ID '{device_id}' not found"),
            Some(serde_json::json!({ "device_id": device_id })),
        ));
    };
    let predefined = predefined_devices::Entity::find_by_id(device.predefined_device_id)
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or(Error::InternalServerError)?;
    // Edge agent (D95): no firmware to build.
    if super::edge_agents::is_agent(&ctx.db, &device).await {
        return Err(super::edge_agents::unsupported_action());
    }
    // Board figée du device : soc merge-bin + pio_board +
    // PNEX_BOARD_NAME. No pio_board on the board → per-project fallback.
    let frozen_board = if let Some(bid) = device.board_id {
        mcu_boards::Entity::find_by_id(bid)
            .one(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
    } else {
        None
    };
    let board_row = match frozen_board {
        Some(b) => b,
        None => mcu_boards::Entity::find_by_id(predefined.board_id)
            .one(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .ok_or_else(|| loco_rs::Error::Message("board du modèle introuvable".to_string()))?,
    };
    let soc = board_row.soc.clone();
    let pio_board = board_row
        .pio_board
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| match predefined.name.as_str() {
            "generic_esp8266" => Some("nodemcuv2".to_string()),
            "generic_esp32c3" => Some("seeed_xiao_esp32c3".to_string()),
            "generic_esp32" => Some("esp32dev".to_string()),
            "generic_esp32cam" => Some("esp32cam".to_string()),
            _ => None,
        });
    let board_name = Some(board_row.name.clone());
    // Écran à compiler — résolution unique : profil v2 de la board figée
    // (builtin prioritaire, kind inconnu fail-closed), sinon déclaration du
    // modèle prédéfini (boîtes noires). `None` = pas d'écran.
    let screen = {
        let details = crate::services::provisioning::load_board_details(&ctx.db, &device)
            .await
            .ok()
            .flatten();
        crate::services::provisioning::screen_for_build(
            details.as_ref(),
            predefined.peripherals.as_ref(),
            &crate::services::provisioning::device_peripherals(&device),
        )
    };

    // Device count quota per type (legacy parity: 403 here, 400 on /devices).
    // The device being built is already registered (and counted): the build
    // is refused only when the org is OVER its quota (tier lowered since),
    // never at it (O36: a Free org could not build its single mixed device).
    let type_name = device_types::Entity::find_by_id(predefined.device_type_id)
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .map(|t| t.name)
        .unwrap_or_default();
    if let Some(limit) =
        crate::controllers::devices::tier_limit_for(&ctx.db, &org, &type_name).await?
    {
        if count_devices_of_type(&ctx.db, org.org.id, predefined.device_type_id).await?
            > i64::from(limit)
        {
            return Err(crate::controllers::coded_error(
                StatusCode::FORBIDDEN,
                err_codes::DEVICE_QUOTA_REACHED,
                format!(
                    "Device limit reached for {} devices in your subscription tier.",
                    type_name.to_ascii_lowercase()
                ),
                Some(serde_json::json!({ "type": type_name.to_ascii_lowercase() })),
            ));
        }
    }

    // Quota check + record upsert atomic per org across pods: the advisory
    // lock is held by this transaction until the record is committed, so
    // two concurrent requests cannot both pass the interval check nor both
    // insert a record for the same device.
    let min_interval = min_build_interval(&ctx.db, &org).await?;
    let txn = ctx
        .db
        .begin()
        .await
        .map_err(|_| Error::InternalServerError)?;
    crate::services::db_lock::xact_lock(
        &txn,
        crate::services::db_lock::ns::BUILD_QUOTA,
        org.org.id,
    )
    .await
    .map_err(|_| Error::InternalServerError)?;

    // Intervalle min : depuis le DERNIER build réussi de l'org (tous devices).
    if let Some(min_secs) = min_interval {
        if let Some(last) = build_records::Entity::find()
            .filter(build_records::Column::OrgId.eq(org.org.id))
            .filter(build_records::Column::BuildPhase.eq(PHASE_SUCCEEDED))
            .order_by_desc(build_records::Column::Id)
            .one(&txn)
            .await
            .map_err(|_| Error::InternalServerError)?
        {
            let elapsed = chrono::Utc::now()
                .signed_duration_since(last.created_at)
                .num_seconds();
            if elapsed < min_secs {
                return Err(crate::controllers::coded_error(
                    StatusCode::TOO_MANY_REQUESTS,
                    err_codes::BUILD_INTERVAL_NOT_MET,
                    "Build interval not met for your subscription tier. Please wait before next build",
                    None,
                ));
            }
        }
        // A build of the org queued/running for less than the interval
        // counts too: otherwise N concurrent requests all pass the check
        // above (none has succeeded yet) and the interval means nothing.
        let since: sea_orm::prelude::DateTimeWithTimeZone =
            (chrono::Utc::now() - chrono::Duration::seconds(min_secs)).into();
        let in_flight = build_records::Entity::find()
            .filter(build_records::Column::OrgId.eq(org.org.id))
            .filter(build_records::Column::BuildPhase.is_in([PHASE_QUEUED, PHASE_RUNNING]))
            .filter(build_records::Column::UpdatedAt.gte(since))
            .one(&txn)
            .await
            .map_err(|_| Error::InternalServerError)?;
        if in_flight.is_some() {
            return Err(crate::controllers::coded_error(
                StatusCode::TOO_MANY_REQUESTS,
                err_codes::BUILD_INTERVAL_NOT_MET,
                "Build interval not met for your subscription tier. Please wait before next build",
                None,
            ));
        }
    }

    // One active build per device: a second request while a build of the
    // device is queued/running is refused (double click, bulk action).
    // Records untouched for longer than twice the build budget are
    // considered stale (crashed worker) and do not block.
    let stale_after = FirmwareSettings::from_config(&ctx.config)
        .timeout_secs
        .saturating_mul(2)
        .max(600) as i64;
    let fresh_since: sea_orm::prelude::DateTimeWithTimeZone =
        (chrono::Utc::now() - chrono::Duration::seconds(stale_after)).into();
    let device_in_flight = build_records::Entity::find()
        .filter(build_records::Column::OrgId.eq(org.org.id))
        .filter(build_records::Column::DeviceId.eq(&device_id))
        .filter(build_records::Column::BuildPhase.is_in([PHASE_QUEUED, PHASE_RUNNING]))
        .filter(build_records::Column::UpdatedAt.gte(fresh_since))
        .one(&txn)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if device_in_flight.is_some() {
        return Err(Error::CustomError(
            StatusCode::CONFLICT,
            loco_rs::controller::ErrorDetail::new(
                err_codes::BUILD_IN_PROGRESS,
                "A build of this device is already queued or running.".to_string(),
            ),
        ));
    }

    // Every build is a NEW record: its id is the firmware version baked
    // into the binary, so two binaries never share a version (OTA compares
    // versions numerically, the firmware refuses downgrades). Old records
    // are pruned by the worker after a successful build (build_retention).
    let record = build_records::ActiveModel {
        device_id: Set(Some(device_id.clone())),
        build_phase: Set(PHASE_QUEUED.to_string()),
        firmware_bin_s3_key: Set(None),
        org_id: Set(org.org.id),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|_| Error::InternalServerError)?;
    // Committed before the enqueue: the worker (possibly inline) reads it.
    txn.commit().await.map_err(|_| Error::InternalServerError)?;

    // Enqueue (ForegroundBlocking : exécution inline — utile aux tests).
    let args = BuildFirmwareArgs {
        build_record_id: record.id,
        org_id: org.org.id,
        device_id,
        predefined_device_name: predefined.name.clone(),
        soc,
        pio_board,
        board_name,
        screen,
        wifi_ssid,
        wifi_secret_id,
        pnex_host,
    };
    if BuildFirmwareWorker::perform_later(&ctx, args)
        .await
        .is_err()
    {
        return Err(crate::controllers::coded_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            err_codes::BUILD_SUBMIT_FAILED,
            "Failed to submit firmware build job",
            None,
        ));
    }

    Ok((
        StatusCode::CREATED,
        format::json(pnex_core::CreateBuildResponse {
            build_id: record.id,
            status: PHASE_QUEUED.to_string(),
            message: "Firmware build job created successfully".to_string(),
        }),
    )
        .into_response())
}

// ─────────────────────────── GET /build-records ───────────────────────────

#[derive(Debug, Default, Deserialize)]
struct ListBuildsQuery {
    device_id: Option<String>,
    /// « true » | « false » ; autre/absent = tous.
    build_phase: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// `GET /api/v1/build-records` — records de l'org, paginés (D14).
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListBuildsQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut query = build_records::Entity::find()
        .filter(build_records::Column::OrgId.eq(org.org.id))
        .order_by_desc(build_records::Column::Id);
    if let Some(device_id) = q.device_id.as_deref().filter(|d| !d.is_empty()) {
        query = query.filter(build_records::Column::DeviceId.eq(device_id));
    }
    if let Some(phase) = q.build_phase.as_deref().filter(|p| PHASES.contains(p)) {
        query = query.filter(build_records::Column::BuildPhase.eq(phase));
    }
    // COUNT + LIMIT/OFFSET in SQL.
    let (count, rows) = pagination::sql_page(&ctx.db, query, page)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let results: Vec<pnex_core::BuildRecord> = rows.into_iter().map(record_dto).collect();

    let mut filters = Vec::new();
    if let Some(d) = q.device_id.as_deref() {
        if !d.is_empty() {
            filters.push(("device_id".to_string(), d.to_string()));
        }
    }
    if let Some(phase) = q.build_phase.as_deref().filter(|p| PHASES.contains(p)) {
        filters.push(("build_phase".to_string(), phase.to_string()));
    }
    Ok(format::json(pagination::envelope(
        "/api/v1/build-records",
        &filters,
        page,
        count,
        results,
    ))
    .into_response())
}

// ─────────────────────────── GET /download/firmware/{device_id} ───────────────────────────

/// `GET /api/v1/download/firmware/{device_id}` — proxies the bytes of the
/// device's last successful build (legacy parity), attachment
/// `{device_id}-firmware.bin`.
async fn download(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(device_id): Path<String>,
) -> Result<Response> {
    // The image embeds the device token, its key and the WiFi credentials
    // (SEC-8): same roles as the build itself, never a viewer.
    if !org.can_write() {
        return Err(forbidden(
            pnex_core::err_codes::DEVICE_WRITE_FORBIDDEN,
            "Owner, admin or member role required to manage devices.",
        ));
    }
    let Some(record) = build_records::Entity::find()
        .filter(build_records::Column::OrgId.eq(org.org.id))
        .filter(build_records::Column::DeviceId.eq(device_id.trim()))
        .filter(build_records::Column::BuildPhase.eq(PHASE_SUCCEEDED))
        .order_by_desc(build_records::Column::Id)
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    let Some(key) = record.firmware_bin_s3_key else {
        return Err(Error::NotFound);
    };
    let settings = FirmwareSettings::from_config(&ctx.config);
    let store = settings
        .store(&ctx.db)
        .map_err(|_| Error::InternalServerError)?;
    let bytes = store.get(&key).await.map_err(|_| Error::NotFound)?;
    let filename = format!(
        "{}-firmware.bin",
        pnex_firmware_builder::sanitize_segment(device_id.trim())
    );
    Ok((
        StatusCode::OK,
        [
            ("content-type", "application/octet-stream".to_string()),
            (
                "content-disposition",
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

// ─────────────────────────── Routes ───────────────────────────

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/build-firmware", post(create))
        .add("/build-records", get(list))
        .add("/download/firmware/{device_id}", get(download))
}
