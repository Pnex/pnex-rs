//! Builds firmware — parité des vues Django `firmware_builder` (Phase 6),
//! scoping org (D2) :
//!
//! - `POST /build-firmware` : ordre de vérification Django — validation
//!   champs → modèle inconnu (400) → device introuvable (404) → quota type
//!   (403) → intervalle min entre builds (429) → record `queued` → enqueue
//!   (worker queue PostgreSQL). Réponse 201 adaptée : plus de
//!   `backend`/`job_name` (pas de k8s), `build_id` à la place ;
//! - `GET /build-records` : liste scopée org, paginée (D14 — Django
//!   renvoyait une liste nue, écart assumé), filtres `device_id`/`success` ;
//! - `DELETE /build-records/{id}` : 400 si build réussi, 400 si le device
//!   existe encore, sinon 204 sans body — l'artefact n'est PAS supprimé
//!   (rétention différée D6) ;
//! - `GET /download/firmware/{device_id}` : proxy des octets de l'artefact
//!   (parité Django, pas d'URL présignée), attachment
//!   `{device_id}-firmware.bin`.
//!
//! Les erreurs build utilisent la forme Django `{"error": "..."}` (les
//! endpoints devices utilisent `{"detail": ...}` — contrats respectifs).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::Json;
use loco_rs::bgworker::BackgroundWorker;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
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

// ─────────────────────────── Aides ───────────────────────────

/// Réponse d'erreur des vues build Django : `{"error": "..."}`.
fn error_status(status: StatusCode, msg: &str) -> Response {
    (
        status,
        format::json(serde_json::json!({ "error": msg })),
    )
        .into_response()
}

/// Erreur champ-par-champ, forme DRF : `{"<champ>": "..."}`.
fn field_status(status: StatusCode, field: &str, msg: &str) -> Response {
    (
        status,
        format::json(serde_json::json!({ field: msg })),
    )
        .into_response()
}

fn forbidden(msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new("forbidden", msg.to_string()),
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
        created_at: r.created_at.to_rfc3339(),
        updated_at: r.updated_at.to_rfc3339(),
    }
}

/// Intervalle min du tier de l'org (None = pas de contrainte).
async fn min_build_interval(
    db: &DatabaseConnection,
    org: &OrgContext,
) -> Result<Option<i64>> {
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

/// Nombre de devices du type donné dans l'org (tous états — parité quota
/// Django, cf. devices create).
async fn count_devices_of_type(
    db: &DatabaseConnection,
    org_id: i64,
    type_id: i64,
) -> Result<i64> {
    let rows = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org_id))
        .find_also_related(predefined_devices::Entity)
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(rows
        .iter()
        .filter(|(_, pd)| {
            pd.as_ref().is_some_and(|p| p.device_type_id == type_id)
        })
        .count() as i64)
}

// ─────────────────────────── POST /build-firmware ───────────────────────────

/// `POST /api/v1/build-firmware` — crée (ou réarme) le record et enfile le
/// job de build.
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<pnex_core::CreateBuild>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden("owner ou admin requis pour lancer les builds"));
    }

    // Validation champs (le mot de passe WiFi n'est PAS trimé — il peut
    // contenir des espaces significatifs).
    let checks = [
        ("wifi_ssid", params.wifi_ssid.trim(), 100),
        ("wifi_password", params.wifi_password.as_str(), 100),
        ("predefined_device_name", params.predefined_device_name.trim(), 100),
        ("pnex_host", params.pnex_host.trim(), 200),
        ("device_id", params.device_id.trim(), 100),
    ];
    for (field, value, max) in checks {
        if value.is_empty() {
            return Ok(field_status(
                StatusCode::BAD_REQUEST,
                field,
                "This field is required.",
            ));
        }
        if value.chars().count() > max {
            return Ok(field_status(
                StatusCode::BAD_REQUEST,
                field,
                &format!("Ensure this field has no more than {max} characters."),
            ));
        }
    }
    if params.pnex_host.split_whitespace().count() > 1 {
        return Ok(field_status(
            StatusCode::BAD_REQUEST,
            "pnex_host",
            "Must be a host without spaces (e.g. dev1.pnex.io).",
        ));
    }
    let wifi_ssid = params.wifi_ssid.trim().to_string();
    // Le mot de passe passe tel quel (espaces/dernier espace significatifs).
    let wifi_password = params.wifi_password.clone();
    let device_id = params.device_id.trim().to_string();
    let pnex_host = params.pnex_host.trim().to_string();

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
    // SoC du board → offsets merge-bin.
    let soc = mcu_boards::Entity::find_by_id(predefined.board_id)
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .map(|b| b.soc)
        .unwrap_or_default();

    // Device connu de l'org (Django : registre du user).
    if device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org.org.id))
        .filter(device_registries::Column::DeviceId.eq(&device_id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .is_none()
    {
        return Ok(error_status(
            StatusCode::NOT_FOUND,
            &format!("Device with ID '{device_id}' not found"),
        ));
    }

    // Quota nb devices du type (parité Django : 403 ici, 400 sur /devices).
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

    // Intervalle min : depuis le DERNIER build réussi de l'org (tous devices).
    if let Some(min_secs) = min_build_interval(&ctx.db, &org).await? {
        if let Some(last) = build_records::Entity::find()
            .filter(build_records::Column::OrgId.eq(org.org.id))
            .filter(build_records::Column::Success.eq(true))
            .order_by_desc(build_records::Column::Id)
            .one(&ctx.db)
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
    }

    // Un record par (org, device_id) : rebuild = réarmement (parité
    // update_or_create Django ; l'ancien artefact devient orphelin — D6).
    let existing = build_records::Entity::find()
        .filter(build_records::Column::OrgId.eq(org.org.id))
        .filter(build_records::Column::DeviceId.eq(&device_id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let (record, created) = match existing {
        Some(model) => {
            let mut record: build_records::ActiveModel = model.into();
            record.success = Set(false);
            record.build_phase = Set(Some(PHASE_QUEUED.to_string()));
            record.firmware_bin_s3_key = Set(None);
            (record.update(&ctx.db).await.map_err(|_| Error::InternalServerError)?, false)
        }
        None => (
            build_records::ActiveModel {
                device_id: Set(Some(device_id.clone())),
                success: Set(false),
                build_phase: Set(Some(PHASE_QUEUED.to_string())),
                firmware_bin_s3_key: Set(None),
                org_id: Set(org.org.id),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?,
            true,
        ),
    };

    // Enqueue (ForegroundBlocking : exécution inline — utile aux tests).
    let args = BuildFirmwareArgs {
        build_record_id: record.id,
        org_id: org.org.id,
        device_id,
        predefined_device_name: predefined.name.clone(),
        soc,
        wifi_ssid,
        wifi_password,
        pnex_host,
    };
    if BuildFirmwareWorker::perform_later(&ctx, args).await.is_err() {
        return Ok(error_status(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to submit firmware build job",
        ));
    }

    Ok((
        StatusCode::CREATED,
        format::json(pnex_core::CreateBuildResponse {
            build_record_created: created,
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
    // Pagination en Rust (patron devices : les builds d'une org sont bornés
    // par les quotas — count exact puis découpage).
    let rows = query
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let count = rows.len() as i64;
    let (skip, take) = page.slice(rows.len());
    let results: Vec<pnex_core::BuildRecord> = rows
        .into_iter()
        .skip(skip)
        .take(take)
        .map(record_dto)
        .collect();

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

/// `DELETE /api/v1/build-records/{id}` — règles Django ; 204 sans body,
/// artefact conservé (D6).
async fn delete_record(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden("owner ou admin requis pour gérer les builds"));
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

/// `GET /api/v1/download/firmware/{device_id}` — proxy des octets du
/// dernier build réussi du device (parité Django), pièce jointe
/// `{device_id}-firmware.bin`.
async fn download(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(device_id): Path<String>,
) -> Result<Response> {
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
        .store()
        .map_err(|_| Error::InternalServerError)?;
    let bytes = store
        .get(&key)
        .await
        .map_err(|_| Error::NotFound)?;
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
