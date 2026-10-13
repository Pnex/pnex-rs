//! Vision Lab API (media-vision-studio.md §4.6, D198–D200). Org from the
//! principal (R1), `require_write` at the head of every write handler (R2);
//! another org's ids answer 404.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, put};
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::lab::{
    LabAnnotationsInput, LabProjectInput, LabRecordingImport, LabSuggestRequest, LabTrainRequest,
    LabVideoImport,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::services::lab::{self, LabError};
use crate::services::media::MediaSettings;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/lab")
        .add("/projects", get(list).post(create))
        .add("/projects/{id}", get(show).put(update).delete(remove))
        .add("/projects/{id}/images", get(images).post(add_images))
        .add("/projects/{id}/stats", get(stats))
        .add(
            "/projects/{id}/import-recording",
            axum::routing::post(import_recording),
        )
        .add(
            "/projects/{id}/import-video",
            axum::routing::post(import_video),
        )
        .add("/projects/{id}/versions", get(versions).post(freeze))
        .add("/images/{id}", delete(remove_image))
        .add("/images/{id}/annotations", put(save_annotations))
        .add("/images/{id}/suggest", axum::routing::post(suggest))
        .add("/images/{id}/interpolate", axum::routing::post(interpolate))
        .add("/versions/{id}/export", get(export))
        .add("/versions/{id}/train", axum::routing::post(train))
        .add("/projects/{id}/runs", get(runs))
        .add("/runs/{id}/cancel", axum::routing::post(cancel_run))
}

fn require_write(org: &OrgContext) -> Result<()> {
    if org.can_write() {
        return Ok(());
    }
    Err(Error::CustomError(
        StatusCode::FORBIDDEN,
        ErrorDetail::new(
            err_codes::LAB_WRITE_FORBIDDEN,
            "Viewer role: Vision Lab projects are read-only.".to_string(),
        ),
    ))
}

/// Service failure → HTTP answer: field errors as `{field: token}`, the
/// rest as a coded `ErrorDetail` (+ `args`).
fn fail(e: LabError) -> Result<Response> {
    if e.code == err_codes::FIELD_INVALID {
        if let Some(fields) = e.args {
            return Ok((StatusCode::BAD_REQUEST, format::json(fields)).into_response());
        }
    }
    if e.status == StatusCode::INTERNAL_SERVER_ERROR {
        return Err(Error::InternalServerError);
    }
    Err(Error::CustomError(
        e.status,
        ErrorDetail {
            error: Some(e.code.to_string()),
            description: Some(e.message),
            errors: e.args.map(|a| serde_json::json!({ "args": a })),
        },
    ))
}

fn reply<T: serde::Serialize>(res: std::result::Result<T, LabError>) -> Result<Response> {
    match res {
        Ok(v) => format::json(v),
        Err(e) => fail(e),
    }
}

fn user_of(org: &OrgContext) -> String {
    org.auth.user.id.to_string()
}

async fn list(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    reply(lab::list_projects(&ctx.db, org.org.id).await)
}

async fn show(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    reply(lab::get_project(&ctx.db, org.org.id, id).await)
}

async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<LabProjectInput>,
) -> Result<Response> {
    require_write(&org)?;
    match lab::create_project(&ctx.db, org.org.id, &user_of(&org), &input).await {
        Ok(p) => Ok((StatusCode::CREATED, format::json(p)).into_response()),
        Err(e) => fail(e),
    }
}

async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<LabProjectInput>,
) -> Result<Response> {
    require_write(&org)?;
    reply(lab::update_project(&ctx.db, org.org.id, id, &input).await)
}

async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_write(&org)?;
    match lab::delete_project(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => fail(e),
    }
}

async fn images(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    reply(lab::list_images(&ctx.db, org.org.id, id).await)
}

#[derive(Debug, Deserialize)]
struct AddImages {
    asset_ids: Vec<Uuid>,
}

async fn add_images(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(body): Json<AddImages>,
) -> Result<Response> {
    require_write(&org)?;
    let store = MediaSettings::from_config(&ctx.config)
        .store()
        .map_err(|_| Error::InternalServerError)?;
    reply(lab::add_images(&ctx.db, store.as_ref(), org.org.id, id, &body.asset_ids).await)
}

async fn remove_image(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_write(&org)?;
    match lab::remove_image(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => fail(e),
    }
}

async fn save_annotations(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<LabAnnotationsInput>,
) -> Result<Response> {
    require_write(&org)?;
    reply(lab::save_annotations(&ctx.db, org.org.id, id, &input).await)
}

/// Frames of a camera recording as pictures of the project (D202).
async fn import_recording(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(req): Json<LabRecordingImport>,
) -> Result<Response> {
    require_write(&org)?;
    reply(lab::import_recording(&ctx, org.org.id, id, &req).await)
}

/// Training of a dataset version (D204): editors only, 201 + the queued run.
async fn train(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(req): Json<LabTrainRequest>,
) -> Result<Response> {
    require_write(&org)?;
    match crate::services::lab_train::start(&ctx, org.org.id, id, &req, &user_of(&org)).await {
        Ok(run) => Ok((StatusCode::CREATED, format::json(run)).into_response()),
        Err(e) => fail(e),
    }
}

/// Training runs of a project (read access).
async fn runs(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    reply(crate::services::lab_train::list(&ctx.db, org.org.id, id).await)
}

async fn cancel_run(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_write(&org)?;
    reply(crate::services::lab_train::cancel(&ctx.db, org.org.id, id).await)
}

/// Frames of a video of the media library: editors only (confined decoder).
async fn import_video(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(req): Json<LabVideoImport>,
) -> Result<Response> {
    require_write(&org)?;
    reply(lab::import_video(&ctx, org.org.id, id, &req).await)
}

/// Interpolated boxes between annotated frames: editors only, proposals
/// only (same gate as pre-annotation).
async fn interpolate(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_write(&org)?;
    reply(lab::interpolate(&ctx.db, org.org.id, id).await)
}

/// Pre-annotation with a registry model: editors only (CPU on the
/// shared vision pool), proposals only.
async fn suggest(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(req): Json<LabSuggestRequest>,
) -> Result<Response> {
    require_write(&org)?;
    let store = MediaSettings::from_config(&ctx.config)
        .store()
        .map_err(|_| Error::InternalServerError)?;
    reply(lab::suggest(&ctx, store.as_ref(), org.org.id, id, &req).await)
}

async fn stats(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    reply(lab::stats(&ctx.db, org.org.id, id).await)
}

async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    reply(lab::list_versions(&ctx.db, org.org.id, id).await)
}

#[derive(Debug, Default, Deserialize)]
struct FreezeBody {
    #[serde(default)]
    note: String,
}

async fn freeze(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(body): Json<FreezeBody>,
) -> Result<Response> {
    require_write(&org)?;
    match lab::freeze(&ctx.db, org.org.id, id, &user_of(&org), &body.note).await {
        Ok(v) => Ok((StatusCode::CREATED, format::json(v)).into_response()),
        Err(e) => fail(e),
    }
}

/// Read access is enough: viewers may download a dataset.
async fn export(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let store = MediaSettings::from_config(&ctx.config)
        .store()
        .map_err(|_| Error::InternalServerError)?;
    match lab::export(&ctx.db, store.as_ref(), org.org.id, id).await {
        Ok((filename, bytes)) => Ok((
            StatusCode::OK,
            [
                ("content-type", "application/zip".to_string()),
                (
                    "content-disposition",
                    format!("attachment; filename=\"{filename}\""),
                ),
            ],
            bytes,
        )
            .into_response()),
        Err(e) => fail(e),
    }
}
