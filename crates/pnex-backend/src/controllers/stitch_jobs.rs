//! Take 360 V2 — jobs de stitch serveur (plan cheerful-booping-teacup B2) :
//! l'app upload un anneau de frames + poses, le serveur assemble le
//! panorama HD (worker `stitch_panorama`) et le publie comme version n+1 de
//! l'asset média. Le device garde son aperçu 1024 immédiat (v1) ; la
//! progression du job (`frames_received/frames_total` puis `state`) est
//! pollée par le front — modèle « build_phase ».
//!
//! Upload des frames : **raw bytes octet-stream** (jamais de multipart —
//! école media.rs), une requête par frame, `DefaultBodyLimit` dédié par
//! route. Les octets vivent dans le MediaStore (mêmes réglages que les
//! médias — `settings.media`), jamais en base.
//!
//! Dédoublonnage des frames volontairement ABSENT : chaque POST incrémente
//! `frames_received` (même frame renvoyée → comptage faux). Contrat client :
//! l'app n'uploade jamais deux fois la même frame (retry = nouvelle frame k
//! écrasée dans le store, compteur à majorer côté client). Écart assumé au
//! bitmask « propre » : la colonne de dédoublonnage ne valait pas la
//! complexité pour un client qui contrôle son flux (plan B2).
//!
//! Errors: 400 per-field `{"<field>": msg}` (devices/flows/media school),
//! 403 via `can_write()`, 404 masquant le cross-org (école `device_of_org`),
//! 409 sur mauvais état, 503 si le stitch serveur est désactivé.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::prelude::*;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter, QueryOrder,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::models::_entities::{media_assets, stitch_jobs};
use crate::services::media::MediaSettings;
use crate::services::stitch::{
    StitchSettings, STATE_FAILED, STATE_QUEUED, STATE_RUNNING, STATE_SUCCEEDED,
};

use crate::workers::stitch_panorama::{PoseDeg, StitchPanoramaArgs, StitchPanoramaWorker};

/// Plafond d'une frame uploadée — 8 Mo (frame 1080×1920 JPEG q85 ≈ 400-700
/// Ko : large marge). La limite axum, posée à max+1 Mo sur la route, reste
/// le filet (école media.rs::routes).
pub const FRAME_MAX_BYTES: usize = 8 * 1024 * 1024;

/// Bornes du job : un anneau 360° = jusqu'à 64 frames (plan C1 : 28 en
/// usage réel), pas de pose unique (le stitcher a besoin de recouvrement).
const MAX_FRAMES: u32 = 64;

// ─────────────────────────── Aides ───────────────────────────

/// Forbidden with the machine code + canonical English description
/// (frontend resolves `err-<code>` at render time, verbatim fallback).
fn forbidden(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

/// 400 per-field `{"<field>": msg}`.
fn field_status(field: &str, msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        format::json(serde_json::json!({ field: msg })),
    )
        .into_response()
}

/// 409 — POST frame sur un job terminal (succeeded/failed). Carries the
/// machine code + canonical English description (frontend resolves
/// `err-<code>`, verbatim fallback otherwise).
fn conflict(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::CONFLICT,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

/// 503 — kill-switch `settings.stitch.enabled=false`.
fn disabled() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        format::json(serde_json::json!({
            "error": "stitch-disabled",
            "description": "Server-side stitching is disabled."
        })),
    )
        .into_response()
}

/// Job de l'org courante, sinon None (→ 404 masquant le cross-org, école
/// `device_of_org` / media.rs::find_asset).
async fn find_job(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: Uuid,
) -> Result<Option<stitch_jobs::Model>> {
    stitch_jobs::Entity::find_by_id(id)
        .filter(stitch_jobs::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

/// Asset de l'org courante (vérification avant création du job — le job
/// référence un asset existant, jamais créé à la volée).
async fn find_asset(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: Uuid,
) -> Result<Option<media_assets::Model>> {
    media_assets::Entity::find_by_id(id)
        .filter(media_assets::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

// ─────────────────────────── DTOs ───────────────────────────

#[derive(serde::Serialize)]
struct StitchJobDto {
    id: Uuid,
    asset_id: Uuid,
    state: String,
    error: Option<String>,
    frames_total: i32,
    frames_received: i32,
}

fn job_dto(j: &stitch_jobs::Model) -> StitchJobDto {
    StitchJobDto {
        id: j.id,
        asset_id: j.asset_id,
        state: j.state.clone(),
        error: j.error.clone(),
        frames_total: j.frames_total,
        frames_received: j.frames_received,
    }
}

// ─────────────────────── POST /stitch-jobs ───────────────────────

#[derive(Deserialize)]
struct CreateJob {
    asset_id: Uuid,
    frames_total: u32,
    hfov_deg: f32,
    poses: Vec<PoseDeg>,
}

/// `POST /api/v1/stitch-jobs` — crée le job `queued` (poses + hfov, ~2 Ko).
/// Les frames partent ensuite une par une ; la dernière déclenche le worker.
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(body): Json<CreateJob>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "stitch-write-forbidden",
            "Write access restricted to owner/admin/member roles.",
        ));
    }
    let stitch_settings = StitchSettings::from_config(&ctx.config);
    if !stitch_settings.enabled {
        return Ok(disabled());
    }
    // Validation (plan B2) : 2 ≤ poses == frames_total ≤ 64, hfov plausible
    // (bornes du stitcher : 1..=179°).
    if body.frames_total < 2 || body.frames_total > MAX_FRAMES {
        return Ok(field_status(
            "frames_total",
            "frames_total doit être entre 2 et 64",
        ));
    }
    if body.poses.len() != body.frames_total as usize {
        return Ok(field_status(
            "poses",
            "une pose par frame : poses.len() == frames_total",
        ));
    }
    if !(1.0..=179.0).contains(&body.hfov_deg) {
        return Ok(field_status(
            "hfov_deg",
            "hfov_deg doit être entre 1 et 179",
        ));
    }
    if find_asset(&ctx.db, &org, body.asset_id).await?.is_none() {
        return Err(Error::NotFound);
    }
    let job = stitch_jobs::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org.org.id),
        asset_id: Set(body.asset_id),
        state: Set(STATE_QUEUED.to_string()),
        error: Set(None),
        frames_total: Set(body.frames_total as i32),
        frames_received: Set(0),
        poses_json: Set(serde_json::to_value(&body.poses).unwrap_or_default()),
        hfov_deg: Set(body.hfov_deg),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .map_err(|_| Error::InternalServerError)?;
    Ok((StatusCode::CREATED, format::json(job_dto(&job))).into_response())
}

// ─────────────── POST /stitch-jobs/{id}/frames/{k} ───────────────

#[derive(Deserialize)]
struct FramePath {
    id: Uuid,
    k: u32,
}

/// `POST /api/v1/stitch-jobs/{id}/frames/{k}` — octet-stream JPEG d'une
/// frame. Incrémente `frames_received` (sans dédoublonner, cf. doc de
/// module) ; la dernière frame reçue enchaîne le worker **dans la requête**
/// (ForegroundBlocking en test, queue PG en dev/prod).
async fn upload_frame(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(path): Path<FramePath>,
    body: Bytes,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "stitch-write-forbidden",
            "Write access restricted to owner/admin/member roles.",
        ));
    }
    let Some(job) = find_job(&ctx.db, &org, path.id).await? else {
        return Err(Error::NotFound);
    };
    // Machine à états : un job terminal (succeeded/failed) n'accepte plus
    // rien (409) — pas de mutation silencieuse d'un résultat déjà publié.
    // Tout autre état inconnu est rejeté pareil (défense en profondeur).
    match job.state.as_str() {
        STATE_QUEUED | STATE_RUNNING => {}
        STATE_SUCCEEDED | STATE_FAILED => {
            return Err(conflict(
                "stitch-job-terminal",
                "Job is already terminal — create a new job to re-stitch.",
            ));
        }
        _ => return Err(conflict("stitch-state-unknown", "Unknown job state.")),
    }
    if path.k >= job.frames_total as u32 {
        return Ok(field_status(
            "k",
            "index de frame hors bornes (0..frames_total)",
        ));
    }
    if body.is_empty() {
        return Ok(field_status("k", "corps de la requête vide"));
    }
    if body.len() > FRAME_MAX_BYTES {
        return Ok((
            StatusCode::PAYLOAD_TOO_LARGE,
            format::json(serde_json::json!({
                "error": "stitch-frame-too-large",
                "description": "Frame too large.",
                "errors": { "args": { "limit": FRAME_MAX_BYTES.to_string() } }
            })),
        )
            .into_response());
    }

    // Octets → MediaStore (d'abord, la DB ensuite — école write_version :
    // un blob orphelin est purgeable, une ligne qui ment ne l'est pas).
    let media = MediaSettings::from_config(&ctx.config);
    let store = media.store().map_err(|_| Error::InternalServerError)?;
    let key = MediaSettings::stitch_key(job.id, path.k);
    store
        .put(&key, body)
        .await
        .map_err(|_| Error::InternalServerError)?;

    // Dernière frame → enqueue du worker (ForegroundBlocking en test :
    // exécution inline, le job est déjà terminal quand cette requête
    // répond).
    let frames_received = job.frames_received + 1;
    let enqueue = frames_received == job.frames_total;
    let mut job: stitch_jobs::ActiveModel = job.into();
    job.frames_received = Set(frames_received);
    job.updated_at = Set(chrono::Utc::now().into());
    let job = job
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if enqueue {
        StitchPanoramaWorker::perform_later(
            &ctx,
            StitchPanoramaArgs {
                job_id: job.id,
                org_id: org.org.id,
            },
        )
        .await
        .map_err(|_| Error::InternalServerError)?;
        // Relecture : en ForegroundBlocking (tests, mono-worker saturé), le
        // worker a déjà tourné — la réponse porte l'état terminal, pas un
        // état périmé.
        let job = stitch_jobs::Entity::find_by_id(job.id)
            .one(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .ok_or(Error::NotFound)?;
        return Ok(format::json(job_dto(&job)).into_response());
    }
    Ok(format::json(job_dto(&job)).into_response())
}

// ─────────────────────── GET /stitch-jobs/{id} ───────────────────────

/// `GET /api/v1/stitch-jobs/{id}` — état du job (poll UI : progression
/// d'upload puis assemblage).
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(job) = find_job(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    Ok(format::json(job_dto(&job)).into_response())
}

// ───────────────── GET /stitch-jobs/by-asset/{asset_id} ─────────────────

/// `GET /api/v1/stitch-jobs/by-asset/{asset_id}` — dernier job d'un asset
/// (le plus récent par `created_at`). Source de vérité serveur pour l'UI
/// médias : overlay « version HD en préparation » sur l'aperçu tant que le
/// job est `queued|running` — survit au redémarrage de l'app (contrairement
/// au poller en mémoire du pipeline de capture). 404 si l'asset n'a jamais
/// été stitché serveur.
async fn latest_for_asset(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(asset_id): Path<Uuid>,
) -> Result<Response> {
    let job = stitch_jobs::Entity::find()
        .filter(stitch_jobs::Column::OrgId.eq(org.org.id))
        .filter(stitch_jobs::Column::AssetId.eq(asset_id))
        .order_by_desc(stitch_jobs::Column::CreatedAt)
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    match job {
        Some(j) => Ok(format::json(job_dto(&j)).into_response()),
        None => Err(Error::NotFound),
    }
}

// ─────────────────────────── Routes ───────────────────────────

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/stitch-jobs")
        .add("", post(create))
        // 2 segments après le préfixe : sans ambiguïté avec GET /{id} (1).
        .add("/by-asset/{asset_id}", get(latest_for_asset))
        .add("/{id}", get(detail))
        .add(
            "/{id}/frames/{k}",
            // Limite dédiée à CETTE route (frame ~1 Mo, plafond 8 Mo + 1 Mo
            // de marge pour le 413 JSON propre du handler).
            post(upload_frame)
                .layer(DefaultBodyLimit::max(FRAME_MAX_BYTES + 1024 * 1024))
                .layer(axum::middleware::from_fn(
                    crate::services::compute_limits::upload_gate,
                )),
        )
}
