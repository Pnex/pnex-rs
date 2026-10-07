//! Firmware builds — parity with the legacy `firmware_builder` views (Phase 6),
//! org scoping (D2):
//!
//! - `POST /build-firmware`: legacy verification order — field validation
//!   → unknown model (400) → device not found (404) → type quota
//!   (403) → min interval between builds (429) → `queued` record → enqueue
//!   (PostgreSQL queue worker). Adapted 201 response: no more
//!   `backend`/`job_name` (no k8s), `build_id` instead;
//! - `GET /build-records`: org-scoped list, paginated (D14 — the legacy
//!   implementation returned a bare list, deliberate divergence), `device_id`/`success` filters;
//! - `DELETE /build-records/{id}` : 400 si build réussi, 400 si le device
//!   existe encore, sinon 204 sans body — l'artefact n'est PAS supprimé
//!   (rétention différée D6) ;
//! - `GET /download/firmware/{device_id}`: proxies the artifact bytes
//!   (legacy parity, no presigned URL), attachment
//!   `{device_id}-firmware.bin`.
//!
//! Build errors use the legacy `{"error": "..."}` shape (the devices
//! endpoints use `{"detail": ...}` — respective contracts).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
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
use crate::services::firmware::{FirmwareSettings, PHASE_QUEUED};
use crate::workers::build_firmware::{BuildFirmwareArgs, BuildFirmwareWorker};
use pnex_core::err_codes;

// ─────────────────────────── Aides ───────────────────────────

/// Error response of the legacy build views: `{"error": "..."}`.
fn error_status(status: StatusCode, msg: &str) -> Response {
    (status, format::json(serde_json::json!({ "error": msg }))).into_response()
}

/// Field-by-field error, legacy shape: `{"<field>": "..."}`.
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
        success: r.success,
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

    // Validation champs (le mot de passe WiFi n'est PAS trimé — il peut
    // contenir des espaces significatifs). WiFi fields only matter without
    // a referential entry (legacy requests).
    let legacy_wifi = params.wifi_credential_id.is_none();
    let wifi_checks = [
        ("wifi_ssid", params.wifi_ssid.trim(), 100),
        ("wifi_password", params.wifi_password.as_str(), 100),
    ];
    let checks = [
        (
            "predefined_device_name",
            params.predefined_device_name.trim(),
            100,
        ),
        ("pnex_host", requested_host.trim(), 200),
        ("device_id", params.device_id.trim(), 100),
    ];
    let wifi_checks = if legacy_wifi { &wifi_checks[..] } else { &[] };
    for &(field, value, max) in wifi_checks.iter().chain(checks.iter()) {
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
    // (secrets.md S6), the password never enters the queue. A legacy
    // request (typed SSID + password) first saves its entry.
    let (wifi_ssid, wifi_secret_id) = match params.wifi_credential_id {
        Some(cid) => {
            let Some(cred) = crate::models::_entities::wifi_credentials::Entity::find()
                .filter(crate::models::_entities::wifi_credentials::Column::OrgId.eq(org.org.id))
                .filter(crate::models::_entities::wifi_credentials::Column::Id.eq(cid))
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
            let Some(secret) = cred.secret_id else {
                return Ok(field_status(
                    StatusCode::BAD_REQUEST,
                    "wifi_password",
                    err_codes::FIELD_REQUIRED,
                ));
            };
            (cred.ssid, secret)
        }
        None => {
            let ring = match crate::controllers::secrets::keyring(&ctx) {
                Ok(ring) => ring,
                Err(e) => return crate::controllers::secrets::store_error(e),
            };
            // Le mot de passe passe tel quel (espaces significatifs).
            match crate::services::secrets::wifi::upsert_typed(
                &ctx.db,
                &ring,
                crate::controllers::secrets::writer(&org),
                org.can_manage_secrets(),
                params.wifi_ssid.trim(),
                &params.wifi_password,
            )
            .await
            {
                Ok(secret) => (params.wifi_ssid.trim().to_string(), secret),
                Err(e) => return crate::controllers::secrets::store_error(e),
            }
        }
    };
    let device_id = params.device_id.trim().to_string();
    let pnex_host = requested_host.trim().to_string();

    // Modèle : sert de sous-répertoire projet du workspace firmware.
    let Some(predefined) = predefined_devices::Entity::find()
        .filter(predefined_devices::Column::Name.eq(params.predefined_device_name.trim()))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Ok(field_status(
            StatusCode::BAD_REQUEST,
            "predefined_device_name",
            &format!(
                "PredefinedDevice with name {} does not exist.",
                params.predefined_device_name.trim()
            ),
        ));
    };
    // Device known to the org (legacy: the user's registry) — model kept
    // (screen peripherals state + frozen board).
    let Some(device) = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org.org.id))
        .filter(device_registries::Column::DeviceId.eq(&device_id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Ok(error_status(
            StatusCode::NOT_FOUND,
            &format!("Device with ID '{device_id}' not found"),
        ));
    };
    // Edge agent (D95): no firmware to build.
    if super::edge_agents::is_agent(&ctx.db, &device).await {
        return Err(super::edge_agents::unsupported_action());
    }
    // Board figée du device : soc merge-bin + pio_board +
    // PNEX_BOARD_NAME. pio_board absent de la board → fallback par projet
    // (compatibilité ini hardcodés ; soil_sensor/custom inchangés).
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
            >= i64::from(limit)
        {
            return Ok(error_status(
                StatusCode::FORBIDDEN,
                &format!(
                    "Device limit reached for {} devices in your subscription tier.",
                    type_name.to_ascii_lowercase()
                ),
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
            .filter(build_records::Column::Success.eq(true))
            .order_by_desc(build_records::Column::Id)
            .one(&txn)
            .await
            .map_err(|_| Error::InternalServerError)?
        {
            let elapsed = chrono::Utc::now()
                .signed_duration_since(last.created_at)
                .num_seconds();
            if elapsed < min_secs {
                return Ok(error_status(
                    StatusCode::TOO_MANY_REQUESTS,
                    "Build interval not met for your subscription tier. Please wait before next build",
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
            .filter(
                build_records::Column::BuildPhase
                    .is_in([PHASE_QUEUED, crate::services::firmware::PHASE_RUNNING]),
            )
            .filter(build_records::Column::UpdatedAt.gte(since))
            .one(&txn)
            .await
            .map_err(|_| Error::InternalServerError)?;
        if in_flight.is_some() {
            return Ok(error_status(
                StatusCode::TOO_MANY_REQUESTS,
                "Build interval not met for your subscription tier. Please wait before next build",
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
        .filter(
            build_records::Column::BuildPhase
                .is_in([PHASE_QUEUED, crate::services::firmware::PHASE_RUNNING]),
        )
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
        success: Set(false),
        build_phase: Set(Some(PHASE_QUEUED.to_string())),
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
        // Always wss through the TLS edge (D70), whatever the client sends.
        ws_ssl: true,
    };
    if BuildFirmwareWorker::perform_later(&ctx, args)
        .await
        .is_err()
    {
        return Ok(error_status(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to submit firmware build job",
        ));
    }

    Ok((
        StatusCode::CREATED,
        format::json(pnex_core::CreateBuildResponse {
            build_record_created: true,
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
    success: Option<String>,
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
    if let Some(success) = q.success.as_deref() {
        match success {
            "true" => query = query.filter(build_records::Column::Success.eq(true)),
            "false" => query = query.filter(build_records::Column::Success.eq(false)),
            _ => {}
        }
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
    if let Some(s) = q.success.as_deref() {
        if s == "true" || s == "false" {
            filters.push(("success".to_string(), s.to_string()));
        }
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

// ─────────────────────────── DELETE /build-records/{id} ───────────────────────────

/// `DELETE /api/v1/build-records/{id}` — legacy rules; 204 with no body,
/// artifact kept (D6).
async fn delete_record(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "build-delete-forbidden",
            "Owner, admin or member role required to manage builds.",
        ));
    }
    let Some(record) = build_records::Entity::find_by_id(id)
        .filter(build_records::Column::OrgId.eq(org.org.id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    if record.success {
        return Ok(error_status(
            StatusCode::BAD_REQUEST,
            "Cannot delete successful firmware builds",
        ));
    }
    let device_exists = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org.org.id))
        .filter(device_registries::Column::DeviceId.eq(record.device_id.clone()))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .is_some();
    if device_exists {
        return Ok(error_status(
            StatusCode::BAD_REQUEST,
            "Cannot delete firmware record while device still exists",
        ));
    }
    build_records::Entity::delete_by_id(record.id)
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
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
        .filter(build_records::Column::Success.eq(true))
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
        .add("/build-records/{id}", delete(delete_record))
        .add("/download/firmware/{device_id}", get(download))
}
