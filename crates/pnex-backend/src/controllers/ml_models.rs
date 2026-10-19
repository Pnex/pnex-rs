//! Vision model registry API (camera-video.md D81) — org-scoped, writes
//! gated by their IAM permission.
//!
//! - `GET    /api/v1/ml/models` — D14 envelope (`search`)
//! - `POST   /api/v1/ml/models` — `{name, description?, asset_id, asset_version?, spec?}`
//! - `GET/PATCH/DELETE /api/v1/ml/models/{id}`
//! - `POST   /api/v1/ml/models/{id}/test` — image bytes (JPEG/PNG) →
//!   `DetectionResult` (the "test with an image" of the registry)
//! - `POST   /api/v1/ml/models/{id}/check` — re-runs the load + test
//!   inference check and stores it (D100)
//! - `POST   /api/v1/ml/models/{id}/test-live?device=` — latest camera frame
//!   through the model, every detection above the floor (D104)
//! - `GET    /api/v1/ml/models/inspect?asset_id=&asset_version=` — what the
//!   ONNX file declares (input size, classes) for the form (D100)
//!
//! Create and spec/file updates run the D100 check first: fixed input
//! dimensions are taken from the file, a wrong label count is a field
//! error, a model that does not load or run is refused (`ml-model-invalid`).
//!
//! Internal (flow runtime, service token): `GET /internal/flow/ml-model/{id}`
//! (spec + sha) and `/internal/flow/ml-model/{id}/content` (ONNX bytes).

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use loco_rs::controller::format;
use loco_rs::prelude::*;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::{device_cameras, media_assets, ml_models};
use crate::services::camera;
use crate::services::flow::FlowSettings;
use crate::services::vision;
use pnex_core::vision::{
    LiveCameraState, LiveTestResult, ModelCheckStatus, ModelSpec, DETECT_FLOOR,
};

/// Test images are small (camera frames, photos) — 16 MB cap.
const TEST_IMAGE_MAX_BYTES: usize = 16 * 1024 * 1024;

fn not_found() -> Error {
    Error::CustomError(
        StatusCode::NOT_FOUND,
        loco_rs::controller::ErrorDetail::new(
            pnex_core::err_codes::ML_MODEL_NOT_FOUND,
            "Vision model not found",
        ),
    )
}

fn unprocessable(code: &str, msg: String) -> Error {
    Error::CustomError(
        StatusCode::UNPROCESSABLE_ENTITY,
        loco_rs::controller::ErrorDetail::new(code, msg),
    )
}

/// Model refused by the D100 check — the loader diagnostic travels as the
/// `detail` arg (verbatim runtime text, documented i18n exception).
fn invalid_model(detail: String) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        format::json(serde_json::json!({
            "error": pnex_core::err_codes::ML_MODEL_INVALID,
            "description": format!("The model does not work with these settings: {detail}"),
            "errors": { "args": { "detail": detail } },
        })),
    )
        .into_response()
}

/// Maps a D100 problem to its response.
fn spec_problem(p: vision::SpecProblem) -> Response {
    match p {
        vision::SpecProblem::Field(field, token) => field_errors(vec![(field, token)]),
        vision::SpecProblem::Load(detail) => invalid_model(detail),
        vision::SpecProblem::Busy(s) => {
            crate::services::compute_limits::busy_error(s).into_response()
        }
    }
}

fn field_errors(errs: Vec<(&'static str, String)>) -> Response {
    let fields = errs
        .into_iter()
        .map(|(k, v)| (k.to_string(), serde_json::Value::String(v)))
        .collect();
    super::fields_error(StatusCode::BAD_REQUEST, fields)
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/ml/models")
        .add("", get(list).post(create))
        .add("/inspect", get(inspect))
        .add("/{id}", get(detail).patch(update).delete(remove))
        .add("/{id}/check", post(check))
        .add("/{id}/test-live", post(test_live))
        .add(
            "/{id}/test",
            post(test_image).layer(DefaultBodyLimit::max(TEST_IMAGE_MAX_BYTES)),
        )
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        list, create, inspect, detail, update, remove, check, test_live, test_image
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

pub fn internal_routes() -> Routes {
    Routes::new()
        .add("/internal/flow/ml-model/{id}", get(internal_spec))
        .add(
            "/internal/flow/ml-model/{id}/content",
            get(internal_content),
        )
}

async fn find(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: Uuid,
) -> Result<Option<ml_models::Model>> {
    ml_models::Entity::find_by_id(id)
        .filter(ml_models::Column::OrgId.eq(org.org.id))
        .filter(ml_models::Column::Task.eq(vision::VISION_TASK))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// Case-insensitive match on name and description.
    search: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
    /// Label filter `name` / `name:value` (D218).
    label: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/ml/models",
    tag = "ml",
    operation_id = "ml_models_list",
    summary = "List the vision models of the organisation",
    description = "Registered ONNX detection models, sorted by name, paginated (`limit`/`offset`), filtered by `search` (name, description) and `label`. Any member.",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::vision::MlModel>),
        (status = 400, description = "Invalid `label` filter", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut select = ml_models::Entity::find()
        .filter(ml_models::Column::OrgId.eq(org.org.id))
        .filter(ml_models::Column::Task.eq(vision::VISION_TASK));
    match pagination::label_ids(
        &ctx.db,
        org.org.id,
        q.label.as_deref(),
        pnex_core::resources::KIND_ML_MODEL,
    )
    .await
    {
        Ok(None) => {}
        Ok(Some(ids)) => {
            select = select
                .filter(ml_models::Column::Id.is_in(pagination::parse_ids::<uuid::Uuid>(&ids)))
        }
        Err(resp) => return Ok(resp),
    }
    let rows = select
        .order_by_asc(ml_models::Column::Name)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let dtos: Vec<_> = rows
        .iter()
        .filter(|m| {
            pagination::rust_search_match(
                &q.search,
                &[m.name.as_str(), m.description.as_deref().unwrap_or("")],
            )
        })
        .map(vision::dto)
        .collect();
    let count = dtos.len() as i64;
    let (skip, take) = page.slice(dtos.len());
    let mut filters = Vec::new();
    if let Some(s) = q.search.as_deref().filter(|s| !s.is_empty()) {
        filters.push(("search".to_string(), s.to_string()));
    }
    if let Some(l) = q.label.as_deref().filter(|l| !l.is_empty()) {
        filters.push(("label".to_string(), l.to_string()));
    }
    format::json(pagination::envelope(
        "/api/v1/ml/models",
        &filters,
        page,
        count,
        dtos.into_iter().skip(skip).take(take).collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/ml/models/{id}",
    tag = "ml",
    operation_id = "ml_models_get",
    summary = "Get a vision model",
    description = "Model spec (family, input size, labels, thresholds) and last load/inference check. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Model id")),
    responses(
        (status = 200, body = pnex_core::vision::MlModel),
        (status = 404, description = "`ml-model-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(m) = find(&ctx.db, &org, id).await? else {
        return Err(not_found());
    };
    format::json(vision::dto(&m))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateInput {
    name: String,
    #[serde(default)]
    description: Option<String>,
    /// Media asset (kind `model`) holding the ONNX file.
    #[schema(value_type = String, format = "uuid")]
    asset_id: Uuid,
    #[serde(default)]
    asset_version: Option<i64>,
    #[serde(default)]
    spec: Option<ModelSpec>,
}

/// The asset must be an org media of kind `model`.
async fn check_asset(db: &DatabaseConnection, org: &OrgContext, asset_id: Uuid) -> Result<bool> {
    Ok(media_assets::Entity::find_by_id(asset_id)
        .filter(media_assets::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .is_some_and(|a| a.kind == "model"))
}

fn validate_name(name: &str) -> Option<(&'static str, String)> {
    let n = name.trim();
    if n.is_empty() {
        return Some(("name", pnex_core::err_codes::FIELD_REQUIRED.to_string()));
    }
    if n.chars().count() > 200 {
        return Some(("name", "max_length:200".to_string()));
    }
    None
}

#[utoipa::path(
    post,
    path = "/api/v1/ml/models",
    tag = "ml",
    operation_id = "ml_models_create",
    summary = "Register a vision model",
    description = "Registers an ONNX file of the media library (kind `model`) as a detection model. The file is loaded and test-run first: input size is taken from the file, a wrong label count is a field error, a model that does not load is refused. Owner, admin or member.",
    request_body = CreateInput,
    responses(
        (status = 201, body = pnex_core::vision::MlModel),
        (status = 400, description = "Field errors (`name`: `required`/`max_length:200`/`unique`, `asset_id`: `invalid`, spec fields)", body = crate::openapi::ErrorDetail),
        (status = 422, description = "`ml-model-invalid` (args.detail = loader diagnostic) or `ml-model-missing-bytes`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`server-busy`: inference pool saturated", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    axum::Json(input): axum::Json<CreateInput>,
) -> Result<Response> {
    let spec = input.spec.unwrap_or_default();
    let mut errs = Vec::new();
    if let Some(e) = validate_name(&input.name) {
        errs.push(e);
    }
    if let Err(more) = spec.check() {
        errs.extend(more);
    }
    if !check_asset(&ctx.db, &org, input.asset_id).await? {
        errs.push(("asset_id", "invalid".to_string()));
    }
    if !errs.is_empty() {
        return Ok(field_errors(errs));
    }
    let dup = ml_models::Entity::find()
        .filter(ml_models::Column::OrgId.eq(org.org.id))
        .filter(ml_models::Column::Name.eq(input.name.trim()))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if dup.is_some() {
        return Ok(field_errors(vec![("name", "unique".to_string())]));
    }
    let Some((bytes, _)) = vision::asset_bytes(&ctx, input.asset_id, input.asset_version).await?
    else {
        return Err(unprocessable(
            pnex_core::err_codes::ML_MODEL_MISSING_BYTES,
            "The model file is missing".into(),
        ));
    };
    let (spec, infer_ms) = match vision::prepare_spec(bytes, spec).await {
        Ok(ok) => ok,
        Err(p) => return Ok(spec_problem(p)),
    };
    let row = vision::insert_model(
        &ctx.db,
        org.org.id,
        &input.name,
        input.description,
        input.asset_id,
        input.asset_version,
        &spec,
        infer_ms,
    )
    .await
    .map_err(|_| Error::InternalServerError)?;
    Ok((StatusCode::CREATED, format::json(vision::dto(&row))).into_response())
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateInput {
    name: Option<String>,
    description: Option<String>,
    /// `null` unpins, a number pins.
    #[serde(default, deserialize_with = "crate::controllers::ml_models::some_null")]
    #[schema(value_type = Option<i64>)]
    asset_version: Option<Option<i64>>,
    spec: Option<ModelSpec>,
}

/// Distinguishes an absent field (`None`) from an explicit `null`
/// (`Some(None)`).
pub(crate) fn some_null<'de, D>(d: D) -> std::result::Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<i64>::deserialize(d)?))
}

#[utoipa::path(
    patch,
    path = "/api/v1/ml/models/{id}",
    tag = "ml",
    operation_id = "ml_models_update",
    summary = "Update a vision model",
    description = "Partial update. A new `spec` or `asset_version` (`null` unpins, a number pins a file version) is re-checked against the file before saving; a failure leaves the stored model untouched. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Model id")),
    request_body = UpdateInput,
    responses(
        (status = 200, body = pnex_core::vision::MlModel),
        (status = 400, description = "Field errors", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ml-model-not-found`", body = crate::openapi::ErrorDetail),
        (status = 422, description = "`ml-model-invalid` (args.detail = loader diagnostic) or `ml-model-missing-bytes`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`server-busy`: inference pool saturated", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    axum::Json(input): axum::Json<UpdateInput>,
) -> Result<Response> {
    let Some(m) = find(&ctx.db, &org, id).await? else {
        return Err(not_found());
    };
    let mut errs = Vec::new();
    if let Some(name) = &input.name {
        if let Some(e) = validate_name(name) {
            errs.push(e);
        }
    }
    if let Some(spec) = &input.spec {
        if let Err(more) = spec.check() {
            errs.extend(more);
        }
    }
    if !errs.is_empty() {
        return Ok(field_errors(errs));
    }
    // A new spec or file version is re-checked (D100) against the
    // resulting pair; a failure leaves the stored model untouched.
    let mut checked: Option<(ModelSpec, u64)> = None;
    if input.spec.is_some() || input.asset_version.is_some() {
        let version = input.asset_version.unwrap_or(m.asset_version);
        let spec = input.spec.clone().unwrap_or_else(|| vision::spec_of(&m));
        let Some((bytes, _)) = vision::asset_bytes(&ctx, m.asset_id, version).await? else {
            return Err(unprocessable(
                pnex_core::err_codes::ML_MODEL_MISSING_BYTES,
                "The model file is missing".into(),
            ));
        };
        match vision::prepare_spec(bytes, spec).await {
            Ok(ok) => checked = Some(ok),
            Err(p) => return Ok(spec_problem(p)),
        }
    }
    let mut upd: ml_models::ActiveModel = m.into();
    if let Some(name) = input.name {
        upd.name = Set(name.trim().to_string());
    }
    if let Some(d) = input.description {
        upd.description = Set(Some(d).filter(|d| !d.trim().is_empty()));
    }
    if let Some(v) = input.asset_version {
        upd.asset_version = Set(v);
    }
    if let Some((spec, infer_ms)) = checked {
        upd.check_status = Set(ModelCheckStatus::Valid.wire().into());
        upd.check_error = Set(None);
        upd.infer_ms = Set(Some(infer_ms as i64));
        upd.checked_at = Set(Some(chrono::Utc::now().into()));
        upd.family = Set(spec.family.wire().into());
        upd.input_width = Set(spec.input_width as i32);
        upd.input_height = Set(spec.input_height as i32);
        upd.labels = Set(serde_json::to_value(&spec.labels).unwrap_or_default());
        upd.score_threshold = Set(spec.score_threshold);
        upd.nms_iou = Set(spec.nms_iou);
    }
    upd.updated_at = Set(chrono::Utc::now().into());
    let saved = upd
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    vision::evict(saved.id);
    format::json(vision::dto(&saved))
}

#[utoipa::path(
    delete,
    path = "/api/v1/ml/models/{id}",
    tag = "ml",
    operation_id = "ml_models_delete",
    summary = "Delete a vision model",
    description = "Removes the model registration; the ONNX file stays in the media library. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Model id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "`ml-model-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(m) = find(&ctx.db, &org, id).await? else {
        return Err(not_found());
    };
    // The ONNX bytes stay in the media library (the user deletes the asset
    // there): one media can back several model specs.
    ml_models::Entity::delete_by_id(m.id)
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    vision::evict(m.id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[utoipa::path(
    post,
    path = "/api/v1/ml/models/{id}/test",
    tag = "ml",
    operation_id = "ml_models_test_image",
    summary = "Run a vision model on an image",
    description = "Runs the model on the uploaded JPEG/PNG (raw body, 16 MB max) and returns the detections. Nothing is stored. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Model id")),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "JPEG or PNG bytes"),
    responses(
        (status = 200, body = pnex_core::vision::DetectionResult),
        (status = 400, description = "Empty body: `field-invalid` (`image`: `required`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ml-model-not-found`", body = crate::openapi::ErrorDetail),
        (status = 422, description = "`ml-model-missing-bytes`, `ml-model-load-failed` or `ml-model-inference-failed`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`server-busy`: inference pool saturated", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn test_image(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    body: axum::body::Bytes,
) -> Result<Response> {
    let Some(m) = find(&ctx.db, &org, id).await? else {
        return Err(not_found());
    };
    if body.is_empty() {
        return Ok(field_errors(vec![(
            "image",
            pnex_core::err_codes::FIELD_REQUIRED.to_string(),
        )]));
    }
    let det = match vision::detector(&ctx, &m).await {
        Ok(Some(d)) => d,
        Ok(None) => {
            return Err(unprocessable(
                "ml-model-missing-bytes",
                "The model file is missing".into(),
            ))
        }
        Err(e) => {
            return Err(unprocessable(
                "ml-model-load-failed",
                format!("The model could not be loaded: {e}"),
            ))
        }
    };
    match vision::detect(det, body.to_vec()).await {
        Ok(res) => format::json(res),
        Err(vision::VisionError::Busy(s)) => Err(crate::services::compute_limits::busy_error(s)),
        Err(e) => Err(unprocessable(
            "ml-model-inference-failed",
            format!("Inference failed: {e}"),
        )),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/ml/models/{id}/check",
    tag = "ml",
    operation_id = "ml_models_check",
    summary = "Re-check a vision model",
    description = "Re-runs the load and test-inference check and stores its outcome in `check`. Owner, admin or member.",
    params(("id" = String, Path, format = "uuid", description = "Model id")),
    responses(
        (status = 200, body = pnex_core::vision::MlModel),
        (status = 404, description = "`ml-model-not-found`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`server-busy`: inference pool saturated", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn check(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(m) = find(&ctx.db, &org, id).await? else {
        return Err(not_found());
    };
    let saved = vision::check_model(&ctx, m).await?;
    format::json(vision::dto(&saved))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct InspectQuery {
    /// Media asset (kind `model`) holding the ONNX file.
    #[param(value_type = String, format = "uuid")]
    asset_id: Uuid,
    #[serde(default)]
    asset_version: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/api/v1/ml/models/inspect",
    tag = "ml",
    operation_id = "ml_models_inspect",
    summary = "Inspect an ONNX file",
    description = "What an ONNX file of the media library declares (input size, class count), to prefill the model form. Read-only. Any member.",
    params(InspectQuery),
    responses(
        (status = 200, body = pnex_core::vision::ModelInspection),
        (status = 400, description = "`field-invalid`: `asset_id` is not an org media of kind `model` (`not-found`)", body = crate::openapi::ErrorDetail),
        (status = 422, description = "`ml-model-invalid` (args.detail = loader diagnostic) or `ml-model-missing-bytes`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`server-busy`: inference pool saturated", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn inspect(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<InspectQuery>,
) -> Result<Response> {
    if !check_asset(&ctx.db, &org, q.asset_id).await? {
        return Ok(super::field_error(
            "asset_id",
            pnex_core::err_codes::FIELD_NOT_FOUND,
        ));
    }
    let Some((bytes, _)) = vision::asset_bytes(&ctx, q.asset_id, q.asset_version).await? else {
        return Err(unprocessable(
            pnex_core::err_codes::ML_MODEL_MISSING_BYTES,
            "The model file is missing".into(),
        ));
    };
    match vision::inspect_bytes(bytes).await {
        Ok(info) => format::json(info),
        Err(vision::VisionError::Busy(s)) => Err(crate::services::compute_limits::busy_error(s)),
        Err(vision::VisionError::Failed(detail)) => Ok(invalid_model(detail)),
    }
}

/// A frame older than this is reported stale by the live test.
const LIVE_STALE_MS: i64 = 3_000;
/// How long the live test waits for a first/fresh frame after waking the
/// camera.
const LIVE_WAIT_MS: u64 = 4_000;
/// Mean luma under which detection is unreliable.
const LIVE_DARK_LUMA: f32 = 40.0;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LiveQuery {
    /// Camera device id.
    device: i64,
}

#[utoipa::path(
    post,
    path = "/api/v1/ml/models/{id}/test-live",
    tag = "ml",
    operation_id = "ml_models_test_live",
    summary = "Run a vision model on a live camera frame",
    description = "Takes the latest frame of a connected camera (waking an on-demand camera for a few seconds), runs the model with a low score floor and returns the frame (base64 JPEG), every detection and warnings (`camera-frame-stale`, `image-too-dark`, `inference-slower-than-camera`). Nothing is stored. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Model id"), LiveQuery),
    responses(
        (status = 200, body = pnex_core::vision::LiveTestResult),
        (status = 400, description = "`field-invalid`: unknown camera `device` (`not-found`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ml-model-not-found`", body = crate::openapi::ErrorDetail),
        (status = 422, description = "`camera-offline`, `camera-no-frame`, `ml-model-missing-bytes`, `ml-model-invalid` or `ml-model-inference-failed`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`server-busy`: inference pool saturated", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn test_live(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<LiveQuery>,
) -> Result<Response> {
    use base64::Engine as _;
    let Some(m) = find(&ctx.db, &org, id).await? else {
        return Err(not_found());
    };
    let Some(cam) = device_cameras::Entity::find_by_id(q.device)
        .filter(device_cameras::Column::OrgId.eq(org.org.id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Ok(super::field_error(
            "device",
            pnex_core::err_codes::FIELD_NOT_FOUND,
        ));
    };
    let settings = camera::settings_of(&cam);
    let state = LiveCameraState {
        connected: crate::controllers::ws_device::is_connected(q.device).await,
        capture_mode: settings.capture_mode.wire().to_string(),
        framesize: settings.framesize.wire().to_string(),
        fps: settings.fps,
    };
    if !state.connected {
        return Err(unprocessable(
            pnex_core::err_codes::CAMERA_OFFLINE,
            "This camera is not connected".into(),
        ));
    }
    // Being a viewer for the duration of the call wakes an on_demand camera;
    // the D76 grace period bridges the gap to the next call.
    let _viewer = camera::viewer_join(&ctx.db, org.org.id, q.device).await;
    let (latest, mut rx) = camera::subscribe(org.org.id, q.device);
    let now = chrono::Utc::now().timestamp_millis();
    let frame = match latest.filter(|f| now - f.ts_ms <= LIVE_STALE_MS) {
        Some(f) => Some(f),
        None => tokio::time::timeout(std::time::Duration::from_millis(LIVE_WAIT_MS), rx.recv())
            .await
            .ok()
            .and_then(|r| r.ok()),
    };
    // Uplink on another pod and no relayed frame yet: last bus frame.
    let frame = match frame {
        Some(f) => Some(f),
        None => match crate::models::_entities::device_registries::Entity::find_by_id(q.device)
            .one(&ctx.db)
            .await
        {
            Ok(Some(dev)) => camera::latest_anywhere(org.org.id, q.device, &dev.device_id).await,
            _ => camera::latest(q.device),
        },
    };
    let Some(frame) = frame else {
        return Err(unprocessable(
            pnex_core::err_codes::CAMERA_NO_FRAME,
            "No image received from this camera yet".into(),
        ));
    };
    let det = match vision::detector(&ctx, &m).await {
        Ok(Some(d)) => d,
        Ok(None) => {
            return Err(unprocessable(
                pnex_core::err_codes::ML_MODEL_MISSING_BYTES,
                "The model file is missing".into(),
            ))
        }
        Err(detail) => return Ok(invalid_model(detail)),
    };
    let threshold = det.spec().score_threshold;
    let jpeg = frame.jpeg.to_vec();
    let analysed = {
        let det = det.clone();
        let jpeg = jpeg.clone();
        match vision::run_blocking(move || {
            let luma = pnex_vision::mean_luma(&jpeg);
            det.detect_with_floor(&jpeg, DETECT_FLOOR)
                .map(|r| (r, luma))
        })
        .await
        {
            Ok(r) => r,
            Err(vision::VisionError::Busy(s)) => {
                return Err(crate::services::compute_limits::busy_error(s));
            }
            Err(vision::VisionError::Failed(_)) => return Err(Error::InternalServerError),
        }
    };
    let (result, luma) = match analysed {
        Ok(ok) => ok,
        Err(e) => {
            return Err(unprocessable(
                pnex_core::err_codes::ML_MODEL_INFERENCE_FAILED,
                format!("Inference failed: {e}"),
            ))
        }
    };
    let age = chrono::Utc::now().timestamp_millis() - frame.ts_ms;
    let mut warnings = Vec::new();
    if age > LIVE_STALE_MS {
        warnings.push("camera-frame-stale".to_string());
    }
    if luma.is_some_and(|l| l < LIVE_DARK_LUMA) {
        warnings.push("image-too-dark".to_string());
    }
    if settings.fps > 0 && result.took_ms > 1000 / u64::from(settings.fps) {
        warnings.push("inference-slower-than-camera".to_string());
    }
    format::json(LiveTestResult {
        frame_b64: base64::engine::general_purpose::STANDARD.encode(&jpeg),
        frame_seq: frame.header.seq,
        frame_age_ms: age,
        result,
        threshold,
        camera: state,
        warnings,
    })
}

// ───────────────────────────── Internal ─────────────────────────────

#[derive(Debug, Deserialize)]
pub struct InternalQuery {
    org_id: i64,
}

fn flow_token_ok(ctx: &AppContext, headers: &HeaderMap) -> bool {
    let Some(expected) = FlowSettings::from_config(&ctx.config)
        .device_write
        .map(|(_, t)| t)
        .filter(|t| !t.is_empty())
    else {
        return false;
    };
    headers
        .get("x-pnex-flow-token")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| crate::auth::service_token::matches(v, &expected))
}

async fn internal_find(
    ctx: &AppContext,
    id: Uuid,
    org_id: i64,
) -> Result<Option<ml_models::Model>> {
    ml_models::Entity::find_by_id(id)
        .filter(ml_models::Column::OrgId.eq(org_id))
        .filter(ml_models::Column::Task.eq(vision::VISION_TASK))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)
}

async fn internal_spec(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<InternalQuery>,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    let Some(m) = internal_find(&ctx, id, q.org_id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let sha = vision::model_version(&ctx.db, &m)
        .await?
        .map(|v| v.sha256.unwrap_or_else(|| v.id.to_string()));
    format::json(serde_json::json!({ "model": vision::dto(&m), "sha256": sha }))
}

async fn internal_content(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<InternalQuery>,
) -> Result<Response> {
    if !flow_token_ok(&ctx, &headers) {
        return Ok(StatusCode::UNAUTHORIZED.into_response());
    }
    let Some(m) = internal_find(&ctx, id, q.org_id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let Some((bytes, _)) = vision::model_bytes(&ctx, &m).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    Ok((
        StatusCode::OK,
        [("content-type", "application/octet-stream")],
        bytes,
    )
        .into_response())
}
