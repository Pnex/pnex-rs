//! Vision Lab API (media-vision-studio.md §4.6, D198–D200). Org from the
//! principal (R1), `require_write` at the head of every write handler (R2);
//! another org's ids answer 404.

use axum::extract::{Path, Query, State};
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
use crate::controllers::ontology::PageQuery;
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
        .add("/images/{id}/propagate", axum::routing::post(propagate))
        .add("/versions/{id}/export", get(export))
        .add("/versions/{id}/train", axum::routing::post(train))
        .add("/projects/{id}/runs", get(runs))
        .add("/runs/{id}/cancel", axum::routing::post(cancel_run))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        list,
        create,
        show,
        update,
        remove,
        images,
        add_images,
        stats,
        import_recording,
        import_video,
        versions,
        freeze,
        remove_image,
        save_annotations,
        suggest,
        interpolate,
        propagate,
        export,
        train,
        runs,
        cancel_run
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
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

/// Service failure → HTTP answer: field errors as `field-invalid`, the
/// rest as a coded `ErrorDetail` (+ `args`).
fn fail(e: LabError) -> Result<Response> {
    if e.code == err_codes::FIELD_INVALID {
        if let Some(serde_json::Value::Object(fields)) = e.args {
            return Ok(super::fields_error(StatusCode::BAD_REQUEST, fields));
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

/// A list page (D240) of a service result.
fn paged<T: serde::Serialize>(
    path: &str,
    q: &PageQuery,
    res: std::result::Result<Vec<T>, LabError>,
) -> Result<Response> {
    match res {
        Ok(rows) => super::ontology::paged(path, &[], q, Ok(rows)),
        Err(e) => fail(e),
    }
}

fn user_of(org: &OrgContext) -> String {
    org.auth.user.id.to_string()
}

#[utoipa::path(
    get,
    path = "/api/v1/lab/projects",
    tag = "lab",
    operation_id = "lab_projects_list",
    summary = "List the Vision Lab projects",
    description = "Annotation projects of the org with image, annotated and version counts. Paginated (`limit`/`offset`). Any member.",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::lab::LabProject>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    paged(
        "/api/v1/lab/projects",
        &q,
        lab::list_projects(&ctx.db, org.org.id).await,
    )
}

#[utoipa::path(
    get,
    path = "/api/v1/lab/projects/{id}",
    tag = "lab",
    operation_id = "lab_projects_get",
    summary = "Get a Vision Lab project",
    description = "Project with its classes and counts. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Project id")),
    responses(
        (status = 200, body = pnex_core::lab::LabProject),
        (status = 404, description = "`lab-project-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn show(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    reply(lab::get_project(&ctx.db, org.org.id, id).await)
}

#[utoipa::path(
    post,
    path = "/api/v1/lab/projects",
    tag = "lab",
    operation_id = "lab_projects_create",
    summary = "Create a Vision Lab project",
    description = "Creates an annotation project with its class list. Owner, admin or member.",
    request_body = pnex_core::lab::LabProjectInput,
    responses(
        (status = 201, body = pnex_core::lab::LabProject),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`lab-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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

#[utoipa::path(
    put,
    path = "/api/v1/lab/projects/{id}",
    tag = "lab",
    operation_id = "lab_projects_update",
    summary = "Update a Vision Lab project",
    description = "Replaces name, description and classes; a class still used by a box cannot be removed. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Project id")),
    request_body = pnex_core::lab::LabProjectInput,
    responses(
        (status = 200, body = pnex_core::lab::LabProject),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`lab-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`lab-project-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`lab-class-in-use`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<LabProjectInput>,
) -> Result<Response> {
    require_write(&org)?;
    reply(lab::update_project(&ctx.db, org.org.id, id, &input).await)
}

#[utoipa::path(
    delete,
    path = "/api/v1/lab/projects/{id}",
    tag = "lab",
    operation_id = "lab_projects_delete",
    summary = "Delete a Vision Lab project",
    description = "Deletes the project, its images (annotations), dataset versions and runs; the source media stay in the media library. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Project id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`lab-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`lab-project-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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

#[utoipa::path(
    get,
    path = "/api/v1/lab/projects/{id}/images",
    tag = "lab",
    operation_id = "lab_images_list",
    summary = "List the images of a project",
    description = "Every image of the project with its boxes, split, review flag and pending proposals. Paginated (`limit`/`offset`). Any member.",
    params(("id" = String, Path, format = "uuid", description = "Project id"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::lab::LabImage>),
        (status = 404, description = "`lab-project-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn images(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let rows = lab::list_images(&ctx.db, org.org.id, id).await;
    paged(&format!("/api/v1/lab/projects/{id}/images"), &q, rows)
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct AddImages {
    /// Media asset ids (images).
    #[schema(value_type = Vec<String>)]
    asset_ids: Vec<Uuid>,
}

#[utoipa::path(
    post,
    path = "/api/v1/lab/projects/{id}/images",
    tag = "lab",
    operation_id = "lab_images_add",
    summary = "Add media images to a project",
    description = "Adds images of the media library to the project (already present ones are skipped) and returns the added images. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Project id")),
    request_body = AddImages,
    responses(
        (status = 200, body = pnex_core::lab::LabImagesAdded),
        (status = 400, description = "`lab-asset-not-image` or field errors", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`lab-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`lab-project-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
    let added = lab::add_images(&ctx.db, store.as_ref(), org.org.id, id, &body.asset_ids).await;
    reply(added.map(|images| pnex_core::lab::LabImagesAdded { images }))
}

#[utoipa::path(
    delete,
    path = "/api/v1/lab/images/{id}",
    tag = "lab",
    operation_id = "lab_images_delete",
    summary = "Remove an image from its project",
    description = "Removes the image and its annotations from the project; the media stays in the library. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Image id")),
    responses(
        (status = 204, description = "Removed"),
        (status = 403, description = "`lab-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`lab-image-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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

#[utoipa::path(
    put,
    path = "/api/v1/lab/images/{id}/annotations",
    tag = "lab",
    operation_id = "lab_images_save_annotations",
    summary = "Save the annotations of an image",
    description = "Replaces the boxes, negative flag and split of an image. Optimistic concurrency: `expected_version` must match the image version, else 409. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Image id")),
    request_body = pnex_core::lab::LabAnnotationsInput,
    responses(
        (status = 200, body = pnex_core::lab::LabImage),
        (status = 400, description = "`lab-box-invalid` or `lab-negative-with-boxes`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`lab-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`lab-image-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`lab-annotations-conflict`: stale `expected_version`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    post,
    path = "/api/v1/lab/projects/{id}/import-recording",
    tag = "lab",
    operation_id = "lab_projects_import_recording",
    summary = "Import frames of a camera recording",
    description = "Samples one frame every `every_secs` between `from` and `to` (RFC 3339) of a camera's recording and adds them as images of the project. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Project id")),
    request_body = pnex_core::lab::LabRecordingImport,
    responses(
        (status = 200, body = pnex_core::lab::LabImagesAdded),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`lab-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Project or camera not found, or `camera-export-empty`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn import_recording(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(req): Json<LabRecordingImport>,
) -> Result<Response> {
    require_write(&org)?;
    let added = lab::import_recording(&ctx, org.org.id, id, &req).await;
    reply(added.map(|images| pnex_core::lab::LabImagesAdded { images }))
}

/// Training of a dataset version (D204): editors only, 201 + the queued run.
#[utoipa::path(
    post,
    path = "/api/v1/lab/versions/{id}/train",
    tag = "lab",
    operation_id = "lab_versions_train",
    summary = "Train a model on a dataset version",
    description = "Queues a fine-tuning run of a registry base model on the frozen dataset version; the run produces a new registry model when it succeeds. One active run per org. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Dataset version id")),
    request_body = pnex_core::lab::LabTrainRequest,
    responses(
        (status = 201, body = pnex_core::lab::LabTrainingRun),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`lab-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`lab-version-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`lab-train-busy`: a run is already active", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    get,
    path = "/api/v1/lab/projects/{id}/runs",
    tag = "lab",
    operation_id = "lab_runs_list",
    summary = "List the training runs of a project",
    description = "Training runs with status and progress, newest first. Paginated (`limit`/`offset`). Any member.",
    params(("id" = String, Path, format = "uuid", description = "Project id"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::lab::LabTrainingRun>),
        (status = 404, description = "`lab-project-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn runs(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let rows = crate::services::lab_train::list(&ctx.db, org.org.id, id).await;
    paged(&format!("/api/v1/lab/projects/{id}/runs"), &q, rows)
}

#[utoipa::path(
    post,
    path = "/api/v1/lab/runs/{id}/cancel",
    tag = "lab",
    operation_id = "lab_runs_cancel",
    summary = "Cancel a training run",
    description = "Cancels a queued or running training run and returns it. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Training run id")),
    responses(
        (status = 200, body = pnex_core::lab::LabTrainingRun),
        (status = 403, description = "`lab-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`lab-run-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn cancel_run(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_write(&org)?;
    reply(crate::services::lab_train::cancel(&ctx.db, org.org.id, id).await)
}

/// Frames of a video of the media library: editors only (confined decoder).
#[utoipa::path(
    post,
    path = "/api/v1/lab/projects/{id}/import-video",
    tag = "lab",
    operation_id = "lab_projects_import_video",
    summary = "Import frames of a video",
    description = "Decodes a video of the media library (confined decoder) and adds one frame every `every_secs` between `from_secs` and `to_secs` as images of the project. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Project id")),
    request_body = pnex_core::lab::LabVideoImport,
    responses(
        (status = 200, body = pnex_core::lab::LabImagesAdded),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`lab-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Project or video not found", body = crate::openapi::ErrorDetail),
        (status = 422, description = "`lab-video-unsupported`, `lab-video-too-large` or `lab-video-decode-failed`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn import_video(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(req): Json<LabVideoImport>,
) -> Result<Response> {
    require_write(&org)?;
    let added = lab::import_video(&ctx, org.org.id, id, &req).await;
    reply(added.map(|images| pnex_core::lab::LabImagesAdded { images }))
}

/// Propagation by tracking: editors only (vision pool), proposals only.
#[utoipa::path(
    post,
    path = "/api/v1/lab/images/{id}/propagate",
    tag = "lab",
    operation_id = "lab_images_propagate",
    summary = "Propagate boxes from the previous frame",
    description = "Tracks the boxes of the previous annotated frame onto this video frame and returns proposals; nothing is written. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Image id")),
    request_body = pnex_core::lab::LabSuggestRequest,
    responses(
        (status = 200, body = pnex_core::lab::LabSuggestions),
        (status = 403, description = "`lab-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`lab-image-not-found`", body = crate::openapi::ErrorDetail),
        (status = 422, description = "`lab-propagate-no-previous` or a model error", body = crate::openapi::ErrorDetail),
        (status = 503, description = "Vision pool saturated", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn propagate(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(req): Json<LabSuggestRequest>,
) -> Result<Response> {
    require_write(&org)?;
    let store = MediaSettings::from_config(&ctx.config)
        .store()
        .map_err(|_| Error::InternalServerError)?;
    let found = lab::propagate(&ctx, store.as_ref(), org.org.id, id, &req).await;
    reply(found.map(|suggestions| pnex_core::lab::LabSuggestions { suggestions }))
}

/// Interpolated boxes between annotated frames: editors only, proposals
/// only (same gate as pre-annotation).
#[utoipa::path(
    post,
    path = "/api/v1/lab/images/{id}/interpolate",
    tag = "lab",
    operation_id = "lab_images_interpolate",
    summary = "Interpolate boxes between keyframes",
    description = "Proposes boxes for this frame interpolated between the surrounding annotated frames; nothing is written. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Image id")),
    responses(
        (status = 200, body = pnex_core::lab::LabSuggestions),
        (status = 403, description = "`lab-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`lab-image-not-found`", body = crate::openapi::ErrorDetail),
        (status = 422, description = "`lab-interpolate-no-keyframes`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn interpolate(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_write(&org)?;
    let found = lab::interpolate(&ctx.db, org.org.id, id).await;
    reply(found.map(|suggestions| pnex_core::lab::LabSuggestions { suggestions }))
}

/// Pre-annotation with a registry model: editors only (CPU on the
/// shared vision pool), proposals only.
#[utoipa::path(
    post,
    path = "/api/v1/lab/images/{id}/suggest",
    tag = "lab",
    operation_id = "lab_images_suggest",
    summary = "Pre-annotate an image with a model",
    description = "Runs a registry detection model on the image and returns up to 100 proposals; nothing is written. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Image id")),
    request_body = pnex_core::lab::LabSuggestRequest,
    responses(
        (status = 200, body = pnex_core::lab::LabSuggestions),
        (status = 403, description = "`lab-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Image or model not found", body = crate::openapi::ErrorDetail),
        (status = 422, description = "`ml-model-missing-bytes`, `ml-model-load-failed` or `ml-model-inference-failed`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "Vision pool saturated", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
    let found = lab::suggest(&ctx, store.as_ref(), org.org.id, id, &req).await;
    reply(found.map(|suggestions| pnex_core::lab::LabSuggestions { suggestions }))
}

#[utoipa::path(
    get,
    path = "/api/v1/lab/projects/{id}/stats",
    tag = "lab",
    operation_id = "lab_projects_stats",
    summary = "Get the dataset statistics of a project",
    description = "Image, split and per-class counts plus non-blocking warnings (`lab-warn-…`) shown before a training. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Project id")),
    responses(
        (status = 200, body = pnex_core::lab::LabStats),
        (status = 404, description = "`lab-project-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn stats(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    reply(lab::stats(&ctx.db, org.org.id, id).await)
}

#[utoipa::path(
    get,
    path = "/api/v1/lab/projects/{id}/versions",
    tag = "lab",
    operation_id = "lab_versions_list",
    summary = "List the dataset versions of a project",
    description = "Frozen dataset versions with their statistics. Paginated (`limit`/`offset`). Any member.",
    params(("id" = String, Path, format = "uuid", description = "Project id"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::lab::LabDatasetVersion>),
        (status = 404, description = "`lab-project-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let rows = lab::list_versions(&ctx.db, org.org.id, id).await;
    paged(&format!("/api/v1/lab/projects/{id}/versions"), &q, rows)
}

#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
struct FreezeBody {
    #[serde(default)]
    note: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/lab/projects/{id}/versions",
    tag = "lab",
    operation_id = "lab_versions_create",
    summary = "Freeze a dataset version",
    description = "Snapshots the current annotations of the project as a new immutable dataset version. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Project id")),
    request_body = FreezeBody,
    responses(
        (status = 201, body = pnex_core::lab::LabDatasetVersion),
        (status = 400, description = "`lab-dataset-empty`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`lab-write-forbidden` (viewer)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`lab-project-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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

#[utoipa::path(
    get,
    path = "/api/v1/lab/versions/{id}/export",
    tag = "lab",
    operation_id = "lab_versions_export",
    summary = "Download a dataset version",
    description = "ZIP archive (images + COCO annotations per split) of a frozen dataset version, as an attachment. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Dataset version id")),
    responses(
        (status = 200, content_type = "application/zip", body = Vec<u8>),
        (status = 404, description = "`lab-version-not-found`", body = crate::openapi::ErrorDetail),
        (status = 413, description = "`lab-export-too-large`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
