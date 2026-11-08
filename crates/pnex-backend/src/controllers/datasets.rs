//! Datasets API (data-pipelines.md D569): catalogue, CSV load, sample. Org from
//! the principal (R1), permissions applied by `OrgContext` (R2).

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::datasets::{
    DatasetIdentityInput, DatasetInput, DatasetTransactionKind, DatasetUploadComplete,
    DatasetUploadInput, UPLOAD_PART_MAX_BYTES,
};
use pnex_core::err_codes;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::datasets::{self, uploads, DatasetError};

const DEFAULT_SAMPLE_ROWS: usize = 50;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/datasets", get(list).post(create))
        .add("/datasets/{id}", get(one).delete(remove))
        .add("/datasets/{id}/identity", put(set_identity))
        .add("/datasets/{id}/freshness", put(set_freshness))
        .add("/datasets/{id}/transactions", get(transactions))
        // A raw body stream: no body limit layer applies, `load_csv` holds the bound.
        .add("/datasets/{id}/transactions/csv", post(load_csv))
        .add("/datasets/{id}/uploads", post(upload_start))
        .add("/datasets/{id}/uploads/{upload}", get(upload_get))
        .add(
            "/datasets/{id}/uploads/{upload}/parts/{part}",
            put(upload_part),
        )
        .add(
            "/datasets/{id}/uploads/{upload}/complete",
            post(upload_complete),
        )
        .add("/datasets/{id}/sample", get(sample))
        .add("/datasets/{id}/profile", get(profile))
        .add("/datasets/{id}/files", get(files))
        .add("/datasets/{id}/files/{index}", get(file))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(
            list,
            one,
            create,
            remove,
            set_identity,
            set_freshness,
            transactions,
            load_csv,
            upload_start,
            upload_get,
            upload_part,
            upload_complete,
            sample,
            profile,
            files,
            file
        ),
        components(schemas(DatasetTransactionKind, pnex_core::datasets::DatasetUploadState))
    )]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn coded(status: StatusCode, code: &str, text: &str, args: Option<serde_json::Value>) -> Error {
    super::coded_error(status, code, text, args)
}

/// D801: the content of a table in the caller's org needs a clearance for each of
/// its markings. `Some` is the answer to send instead of the content.
async fn governed(
    ctx: &AppContext,
    org: &OrgContext,
    id: Uuid,
    what: &str,
) -> Option<Result<Response>> {
    // In the org first: a refusal must not name the markings of another org's table.
    if let Err(e) = datasets::get(&ctx.db, org.org.id, id).await {
        return Some(dataset_error(e));
    }
    let origin = if org.auth.service.is_some() {
        "access-key"
    } else {
        "ui"
    };
    match crate::services::governance::check_dataset_read(
        &ctx.db,
        org.org.id,
        org.auth.user.id,
        id,
        what,
        origin,
    )
    .await
    {
        Ok(missing) if missing.is_empty() => None,
        Ok(missing) => Some(Err(super::coded_error(
            StatusCode::FORBIDDEN,
            err_codes::MARKING_REQUIRED,
            "A clearance is missing for a marking of this dataset.",
            Some(serde_json::json!({ "markings": missing })),
        ))),
        Err(e) => {
            tracing::error!(error = %e, "governance check failed");
            Some(Err(Error::InternalServerError))
        }
    }
}

fn dataset_error(e: DatasetError) -> Result<Response> {
    let args = e.args();
    Err(match e {
        DatasetError::Invalid { field, token } => return Ok(super::field_error(&field, &token)),
        DatasetError::NotFound => Error::CustomError(
            StatusCode::NOT_FOUND,
            ErrorDetail::new(err_codes::DATASET_NOT_FOUND, "Dataset not found."),
        ),
        DatasetError::NameTaken => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::DATASET_NAME_TAKEN,
                "A dataset of the org already has this name.",
            ),
        ),
        DatasetError::TransactionOpen => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::DATASET_TRANSACTION_OPEN,
                "Another load of this dataset is running.",
            ),
        ),
        DatasetError::TransactionAborted => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::DATASET_TRANSACTION_ABORTED,
                "The load stopped before it could commit; run it again.",
            ),
        ),
        DatasetError::Csv(_) => coded(
            StatusCode::BAD_REQUEST,
            err_codes::DATASET_CSV_INVALID,
            "The file is not a CSV this server can read.",
            args,
        ),
        DatasetError::SchemaMismatch { .. } => coded(
            StatusCode::CONFLICT,
            err_codes::DATASET_SCHEMA_MISMATCH,
            "The columns of the file are not those of the dataset.",
            args,
        ),
        DatasetError::QuotaExceeded => coded(
            StatusCode::CONFLICT,
            err_codes::DATASET_QUOTA,
            "The org's dataset storage is full.",
            args,
        ),
        DatasetError::SourceTooLarge => coded(
            StatusCode::PAYLOAD_TOO_LARGE,
            err_codes::DATASET_SOURCE_TOO_LARGE,
            "The file is over the size a load accepts.",
            args,
        ),
        DatasetError::UploadNotFound => Error::CustomError(
            StatusCode::NOT_FOUND,
            ErrorDetail::new(err_codes::DATASET_UPLOAD_NOT_FOUND, "Upload not found."),
        ),
        DatasetError::UploadClosed | DatasetError::UploadLost => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::DATASET_UPLOAD_CLOSED,
                "The upload no longer takes parts.",
            ),
        ),
        DatasetError::PartTooLarge => coded(
            StatusCode::PAYLOAD_TOO_LARGE,
            err_codes::DATASET_UPLOAD_PART_TOO_LARGE,
            "The part is over the size of a part.",
            Some(serde_json::json!({ "max": UPLOAD_PART_MAX_BYTES })),
        ),
        DatasetError::PartsMissing { missing, count } => coded(
            StatusCode::CONFLICT,
            err_codes::DATASET_UPLOAD_PARTS_MISSING,
            "The parts received are not those announced.",
            Some(serde_json::json!({ "missing": missing, "count": count })),
        ),
        DatasetError::TooManyUploads => coded(
            StatusCode::CONFLICT,
            err_codes::DATASET_UPLOAD_TOO_MANY,
            "Too many uploads are in progress in this org.",
            None,
        ),
        DatasetError::SourceInterrupted => Error::CustomError(
            StatusCode::BAD_REQUEST,
            ErrorDetail::new(
                err_codes::DATASET_SOURCE_INTERRUPTED,
                "The upload stopped before its end.",
            ),
        ),
        DatasetError::VersionPurged => Error::CustomError(
            StatusCode::GONE,
            ErrorDetail::new(
                err_codes::DATASET_VERSION_PURGED,
                "This version of the dataset has left its retention.",
            ),
        ),
        DatasetError::ErasurePending => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::DATASET_ERASURE_PENDING,
                "An erasure is being applied to this table.",
            ),
        ),
        DatasetError::Reserved => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::DATASET_RESERVED,
                "This table is filled by an import.",
            ),
        ),
        DatasetError::Storage(e) => {
            tracing::warn!("dataset storage: {e}");
            Error::CustomError(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorDetail::new(
                    err_codes::DATASET_STORAGE_UNAVAILABLE,
                    "The dataset storage is not reachable.",
                ),
            )
        }
        DatasetError::Db(e) => {
            tracing::error!(error = %e, "dataset database error");
            Error::InternalServerError
        }
    })
}

/// Page of a list (`limit`/`offset`, D14).
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PageQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/datasets`
#[utoipa::path(
    get,
    path = "/api/v1/datasets",
    tag = "datasets",
    operation_id = "datasets_list",
    summary = "List the datasets",
    description = "Datasets of the org by name, each with the rows and bytes of its current view. Requires `datasets:read`. Paginated (`limit`/`offset`).",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::datasets::Dataset>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    match datasets::list(&ctx.db, org.org.id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            "/api/v1/datasets",
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => dataset_error(e),
    }
}

/// `POST /api/v1/datasets`
#[utoipa::path(
    post,
    path = "/api/v1/datasets",
    tag = "datasets",
    operation_id = "datasets_create",
    summary = "Create a dataset",
    description = "Creates an empty dataset; load rows with `POST /datasets/{id}/transactions/csv`. Requires `datasets:write`.",
    request_body = pnex_core::datasets::DatasetInput,
    responses(
        (status = 201, body = pnex_core::datasets::Dataset),
        (status = 400, description = "`field-invalid`: `errors.fields.name` token `required` or `max_length:255`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`dataset-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<DatasetInput>,
) -> Result<Response> {
    match datasets::create(&ctx.db, org.org.id, &input.name).await {
        Ok(d) => Ok((StatusCode::CREATED, Json(d)).into_response()),
        Err(e) => dataset_error(e),
    }
}

/// `GET /api/v1/datasets/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/datasets/{id}",
    tag = "datasets",
    operation_id = "datasets_get",
    summary = "Get a dataset",
    description = "One dataset with its columns and the rows and bytes of its current view. Requires `datasets:read`.",
    params(("id" = String, Path, format = "uuid", description = "Dataset id (UUID)")),
    responses(
        (status = 200, body = pnex_core::datasets::Dataset),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match datasets::get(&ctx.db, org.org.id, id).await {
        Ok(d) => format::json(d),
        Err(e) => dataset_error(e),
    }
}

/// `DELETE /api/v1/datasets/{id}`
#[utoipa::path(
    delete,
    path = "/api/v1/datasets/{id}",
    tag = "datasets",
    operation_id = "datasets_delete",
    summary = "Delete a dataset",
    description = "Removes a dataset, its history and its files for good. Requires `datasets:delete`.",
    params(("id" = String, Path, format = "uuid", description = "Dataset id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`dataset-transaction-open`, `dataset-reserved`, `dataset-erasure-pending`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`dataset-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let result = match datasets::store_of(&ctx.config) {
        Ok(store) => datasets::delete(&ctx.db, &store, org.org.id, id).await,
        Err(e) => Err(e),
    };
    match result {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => dataset_error(e),
    }
}

/// `PUT /api/v1/datasets/{id}/freshness`
#[utoipa::path(
    put,
    path = "/api/v1/datasets/{id}/freshness",
    tag = "datasets",
    operation_id = "datasets_freshness_set",
    summary = "Set how long a dataset may go without a new version",
    description = "Past this many hours without a new version the dataset reads as stale and the org is warned once; `null` stops the check. Requires `datasets:write`.",
    params(("id" = String, Path, format = "uuid", description = "Dataset id (UUID)")),
    request_body = pnex_core::datasets::DatasetFreshnessInput,
    responses(
        (status = 200, body = pnex_core::datasets::Dataset),
        (status = 400, description = "`field-invalid` on `max_age_hours`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn set_freshness(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<pnex_core::datasets::DatasetFreshnessInput>,
) -> Result<Response> {
    match datasets::set_freshness(&ctx.db, org.org.id, id, input.max_age_hours).await {
        Ok(d) => format::json(d),
        Err(e) => dataset_error(e),
    }
}

/// `PUT /api/v1/datasets/{id}/identity`
#[utoipa::path(
    put,
    path = "/api/v1/datasets/{id}/identity",
    tag = "datasets",
    operation_id = "datasets_identity_set",
    summary = "Declare the identity columns of a dataset",
    description = "Replaces the columns through which erasing an object removes its rows from every readable version of this dataset. Erasures already done are not applied again. Requires `datasets:write`.",
    params(("id" = String, Path, format = "uuid", description = "Dataset id (UUID)")),
    request_body = pnex_core::datasets::DatasetIdentityInput,
    responses(
        (status = 200, body = pnex_core::datasets::Dataset),
        (status = 400, description = "`field-invalid`: `errors.fields.columns.<n>.column`, `.type_key` or `.property` token `invalid`, or `errors.fields.columns` token `max_items:32`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`dataset-reserved`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn set_identity(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<DatasetIdentityInput>,
) -> Result<Response> {
    match datasets::erasure::set_identity(&ctx.db, org.org.id, id, &input.columns).await {
        Ok(d) => format::json(d),
        Err(e) => dataset_error(e),
    }
}

/// `GET /api/v1/datasets/{id}/transactions`
#[utoipa::path(
    get,
    path = "/api/v1/datasets/{id}/transactions",
    tag = "datasets",
    operation_id = "datasets_transactions_list",
    summary = "List the transactions of a dataset",
    description = "Transactions of a dataset, newest first, aborted ones with their failure code. Requires `datasets:read`. Paginated (`limit`/`offset`).",
    params(
        ("id" = String, Path, format = "uuid", description = "Dataset id (UUID)"),
        PageQuery
    ),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::datasets::DatasetTransaction>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn transactions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    match datasets::transactions(&ctx.db, org.org.id, id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            &format!("/api/v1/datasets/{id}/transactions"),
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => dataset_error(e),
    }
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct LoadQuery {
    /// `snapshot` replaces the view, `append` adds to it. Default `snapshot`.
    kind: Option<DatasetTransactionKind>,
    /// One ASCII character. Default: picked from the first line.
    delimiter: Option<String>,
}

/// `POST /api/v1/datasets/{id}/transactions/csv`
#[utoipa::path(
    post,
    path = "/api/v1/datasets/{id}/transactions/csv",
    tag = "datasets",
    operation_id = "datasets_transactions_load_csv",
    summary = "Load a CSV file into a dataset",
    description = "Streams a CSV body into a new transaction, as text columns, without holding the file in memory. Readers see it once committed. Requires `datasets:write`.",
    params(
        ("id" = String, Path, format = "uuid", description = "Dataset id (UUID)"),
        LoadQuery
    ),
    request_body(content = String, content_type = "text/csv"),
    responses(
        (status = 201, body = pnex_core::datasets::DatasetTransaction),
        (status = 400, description = "`dataset-csv-invalid` (args `reason`, `line`, `column`, `expected`, `got`), `dataset-source-interrupted`, or `field-invalid` on `delimiter`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`dataset-reserved`, `dataset-transaction-open`, `dataset-transaction-aborted`, `dataset-schema-mismatch` (args `columns`), `dataset-quota` (args `max`)", body = crate::openapi::ErrorDetail),
        (status = 413, description = "`dataset-source-too-large` (args `max`)", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`dataset-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn load_csv(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<LoadQuery>,
    body: Body,
) -> Result<Response> {
    let delimiter = match q.delimiter.as_deref() {
        None => None,
        Some(d) if valid_delimiter(d) => d.bytes().next(),
        Some(_) => return Ok(super::field_error("delimiter", "invalid")),
    };
    let kind = q.kind.unwrap_or(DatasetTransactionKind::Snapshot);
    if let Err(e) = datasets::check_loadable(&ctx.db, org.org.id, id).await {
        return dataset_error(e);
    }
    let store = match datasets::store_of(&ctx.config) {
        Ok(store) => store,
        Err(e) => return dataset_error(e),
    };
    let stream = body.into_data_stream();
    match datasets::load_csv(&ctx.db, &store, org.org.id, id, kind, delimiter, stream).await {
        Ok(t) => Ok((StatusCode::CREATED, Json(t)).into_response()),
        Err(e) => dataset_error(e),
    }
}

fn valid_delimiter(d: &str) -> bool {
    d.len() == 1 && d.is_ascii() && d != "\"" && d != "\n" && d != "\r"
}

/// `POST /api/v1/datasets/{id}/uploads`
#[utoipa::path(
    post,
    path = "/api/v1/datasets/{id}/uploads",
    tag = "datasets",
    operation_id = "datasets_uploads_start",
    summary = "Open the upload of a file in parts",
    description = "For a file over what a request may carry behind a proxy (Cloudflare: 100 MB). Send the parts with `PUT .../parts/{part}`, close with `POST .../complete`, then read the upload until it is `done` or `failed`. An upload still open on this table is replaced. Requires `datasets:write`.",
    params(("id" = String, Path, format = "uuid", description = "Dataset id (UUID)")),
    request_body = pnex_core::datasets::DatasetUploadInput,
    responses(
        (status = 201, body = pnex_core::datasets::DatasetUpload),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`dataset-reserved`, `dataset-transaction-open`, `dataset-upload-too-many`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`dataset-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn upload_start(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<DatasetUploadInput>,
) -> Result<Response> {
    if input
        .delimiter
        .as_deref()
        .is_some_and(|d| !valid_delimiter(d))
    {
        return Ok(super::field_error("delimiter", "invalid"));
    }
    let kind = input.kind.unwrap_or(DatasetTransactionKind::Snapshot);
    let result = match datasets::store_of(&ctx.config) {
        Ok(store) => {
            uploads::start(
                &ctx.db,
                &store,
                org.org.id,
                id,
                kind,
                input.delimiter.as_deref(),
            )
            .await
        }
        Err(e) => Err(e),
    };
    match result {
        Ok(u) => Ok((StatusCode::CREATED, Json(u)).into_response()),
        Err(e) => dataset_error(e),
    }
}

/// `GET /api/v1/datasets/{id}/uploads/{upload}`
#[utoipa::path(
    get,
    path = "/api/v1/datasets/{id}/uploads/{upload}",
    tag = "datasets",
    operation_id = "datasets_uploads_get",
    summary = "State of an upload",
    description = "Parts received, then the conversion: `done` with its `transaction_id`, or `failed` with its `code` and `args`. Requires `datasets:read`.",
    params(
        ("id" = String, Path, format = "uuid", description = "Dataset id (UUID)"),
        ("upload" = String, Path, format = "uuid", description = "Upload id (UUID)")
    ),
    responses(
        (status = 200, body = pnex_core::datasets::DatasetUpload),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-upload-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn upload_get(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, upload)): Path<(Uuid, Uuid)>,
) -> Result<Response> {
    match uploads::get(&ctx.db, org.org.id, id, upload).await {
        Ok(u) => format::json(u),
        Err(e) => dataset_error(e),
    }
}

/// `PUT /api/v1/datasets/{id}/uploads/{upload}/parts/{part}`
#[utoipa::path(
    put,
    path = "/api/v1/datasets/{id}/uploads/{upload}/parts/{part}",
    tag = "datasets",
    operation_id = "datasets_uploads_part_put",
    summary = "Store one part of an upload",
    description = "The bytes of the file, cut anywhere, 64 MiB at most. Parts are numbered from 1 and may come in any order; sending one again replaces it. Requires `datasets:write`.",
    params(
        ("id" = String, Path, format = "uuid", description = "Dataset id (UUID)"),
        ("upload" = String, Path, format = "uuid", description = "Upload id (UUID)"),
        ("part" = i32, Path, description = "Part number, from 1 to 1000")
    ),
    request_body(content = String, content_type = "application/octet-stream"),
    responses(
        (status = 200, body = pnex_core::datasets::DatasetUpload),
        (status = 400, description = "`field-invalid` (`errors.fields`), `dataset-source-interrupted`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-upload-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`dataset-upload-closed`", body = crate::openapi::ErrorDetail),
        (status = 413, description = "`dataset-upload-part-too-large` (args `max`), `dataset-source-too-large` (args `max`)", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`dataset-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn upload_part(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, upload, part)): Path<(Uuid, Uuid, i32)>,
    body: Body,
) -> Result<Response> {
    let store = match datasets::store_of(&ctx.config) {
        Ok(store) => store,
        Err(e) => return dataset_error(e),
    };
    let stream = body.into_data_stream();
    match uploads::put_part(&ctx.db, &store, org.org.id, id, upload, part, stream).await {
        Ok(u) => format::json(u),
        Err(e) => dataset_error(e),
    }
}

/// `POST /api/v1/datasets/{id}/uploads/{upload}/complete`
#[utoipa::path(
    post,
    path = "/api/v1/datasets/{id}/uploads/{upload}/complete",
    tag = "datasets",
    operation_id = "datasets_uploads_complete",
    summary = "Close an upload and convert the file",
    description = "Says how many parts make the file; refused if any is missing. A server then converts it in the background: read the upload until it is `done` or `failed`. Requires `datasets:write`.",
    params(
        ("id" = String, Path, format = "uuid", description = "Dataset id (UUID)"),
        ("upload" = String, Path, format = "uuid", description = "Upload id (UUID)")
    ),
    request_body = pnex_core::datasets::DatasetUploadComplete,
    responses(
        (status = 202, body = pnex_core::datasets::DatasetUpload),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-upload-not-found`, `dataset-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`dataset-upload-closed`, `dataset-reserved`, `dataset-upload-parts-missing` (args `missing`, `count`)", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`dataset-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn upload_complete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, upload)): Path<(Uuid, Uuid)>,
    Json(input): Json<DatasetUploadComplete>,
) -> Result<Response> {
    match uploads::complete(&ctx.db, org.org.id, id, upload, input.parts).await {
        Ok(u) => Ok((StatusCode::ACCEPTED, Json(u)).into_response()),
        Err(e) => dataset_error(e),
    }
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct SampleQuery {
    /// Rows, 200 at most. Default 50.
    limit: Option<usize>,
    /// Reads the view as it was at this instant.
    #[param(format = "date-time", value_type = Option<String>)]
    as_of: Option<chrono::DateTime<chrono::Utc>>,
}

/// `GET /api/v1/datasets/{id}/sample`
#[utoipa::path(
    get,
    path = "/api/v1/datasets/{id}/sample",
    tag = "datasets",
    operation_id = "datasets_sample",
    summary = "Read the first rows of a dataset",
    description = "First rows of the current view, or of the view at `as_of`, values as text and cut to 1000 characters. Requires `datasets:read`.",
    params(
        ("id" = String, Path, format = "uuid", description = "Dataset id (UUID)"),
        SampleQuery
    ),
    responses(
        (status = 200, body = pnex_core::datasets::DatasetSample),
        (status = 403, description = "`access-denied`, or `marking-required` (args `markings`): a clearance is missing", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-not-found`", body = crate::openapi::ErrorDetail),
        (status = 410, description = "`dataset-version-purged`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`dataset-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn sample(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<SampleQuery>,
) -> Result<Response> {
    if let Some(refused) = governed(&ctx, &org, id, "sample").await {
        return refused;
    }
    let limit = q.limit.unwrap_or(DEFAULT_SAMPLE_ROWS);
    let result = match datasets::store_of(&ctx.config) {
        Ok(store) => datasets::sample(&ctx.db, &store, org.org.id, id, limit, q.as_of).await,
        Err(e) => Err(e),
    };
    match result {
        Ok(s) => format::json(s),
        Err(e) => dataset_error(e),
    }
}

/// `GET /api/v1/datasets/{id}/profile`
#[utoipa::path(
    get,
    path = "/api/v1/datasets/{id}/profile",
    tag = "datasets",
    operation_id = "datasets_profile",
    summary = "Read how full the columns of a dataset are",
    description = "Row count, and for each column its type and its empty values, from the file footers of the current view. No value is read. Requires `datasets:read`.",
    params(("id" = String, Path, format = "uuid", description = "Dataset id (UUID)")),
    responses(
        (status = 200, body = pnex_core::datasets::DatasetProfile),
        (status = 403, description = "`access-denied`, or `marking-required` (args `markings`): a clearance is missing", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-not-found`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`dataset-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn profile(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if let Some(refused) = governed(&ctx, &org, id, "profile").await {
        return refused;
    }
    let result = match datasets::store_of(&ctx.config) {
        Ok(store) => datasets::profile(&ctx.db, &store, org.org.id, id).await,
        Err(e) => Err(e),
    };
    match result {
        Ok(p) => format::json(p),
        Err(e) => dataset_error(e),
    }
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct FilesQuery {
    /// Reads the view as it was at this instant; pass the same one to every
    /// call so the files do not move between them.
    #[param(format = "date-time", value_type = Option<String>)]
    as_of: Option<chrono::DateTime<chrono::Utc>>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/datasets/{id}/files`
#[utoipa::path(
    get,
    path = "/api/v1/datasets/{id}/files",
    tag = "datasets",
    operation_id = "datasets_files_list",
    summary = "List the Parquet files of a dataset",
    description = "Files of the current view, or of the view at `as_of`, in commit order; each is read whole by `GET /datasets/{id}/files/{index}`. For training code that reads Parquet directly. Requires `datasets:read`. Paginated (`limit`/`offset`).",
    params(
        ("id" = String, Path, format = "uuid", description = "Dataset id (UUID)"),
        FilesQuery
    ),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::datasets::DatasetFile>),
        (status = 403, description = "`access-denied`, or `marking-required` (args `markings`): a clearance is missing", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-not-found`", body = crate::openapi::ErrorDetail),
        (status = 410, description = "`dataset-version-purged`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn files(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<FilesQuery>,
) -> Result<Response> {
    if let Some(refused) = governed(&ctx, &org, id, "files").await {
        return refused;
    }
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let all = match datasets::view_files(&ctx.db, org.org.id, id, q.as_of).await {
        Ok(f) => f,
        Err(e) => return dataset_error(e),
    };
    let count = all.len() as i64;
    let results: Vec<pnex_core::datasets::DatasetFile> = all
        .into_iter()
        .enumerate()
        .skip(page.offset.max(0) as usize)
        .take(page.limit.max(0) as usize)
        .map(|(i, f)| pnex_core::datasets::DatasetFile {
            index: i as i64,
            transaction_id: f.transaction.to_string(),
            rows: f.rows as i64,
            bytes: f.bytes as i64,
        })
        .collect();
    let filters: Vec<(String, String)> = q
        .as_of
        .map(|t| ("as_of".to_string(), t.to_rfc3339()))
        .into_iter()
        .collect();
    format::json(pagination::envelope(
        &format!("/api/v1/datasets/{id}/files"),
        &filters,
        page,
        count,
        results,
    ))
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct FileQuery {
    /// The `as_of` of the listing the index comes from.
    #[param(format = "date-time", value_type = Option<String>)]
    as_of: Option<chrono::DateTime<chrono::Utc>>,
}

/// `GET /api/v1/datasets/{id}/files/{index}`
#[utoipa::path(
    get,
    path = "/api/v1/datasets/{id}/files/{index}",
    tag = "datasets",
    operation_id = "datasets_files_get",
    summary = "Download a Parquet file of a dataset",
    description = "The Parquet file at `index` of the view listed by `GET /datasets/{id}/files` with the same `as_of`. Requires `datasets:read`.",
    params(
        ("id" = String, Path, format = "uuid", description = "Dataset id (UUID)"),
        ("index" = i64, Path, description = "Position of the file in the view, from 0"),
        FileQuery
    ),
    responses(
        (status = 200, content_type = "application/vnd.apache.parquet", body = Vec<u8>),
        (status = 403, description = "`access-denied`, or `marking-required` (args `markings`): a clearance is missing", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-not-found`: no such dataset or no file at this index", body = crate::openapi::ErrorDetail),
        (status = 410, description = "`dataset-version-purged`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`dataset-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn file(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, index)): Path<(Uuid, usize)>,
    Query(q): Query<FileQuery>,
) -> Result<Response> {
    if let Some(refused) = governed(&ctx, &org, id, "file").await {
        return refused;
    }
    let found = match datasets::view_files(&ctx.db, org.org.id, id, q.as_of).await {
        Ok(f) => f.into_iter().nth(index),
        Err(e) => return dataset_error(e),
    };
    let Some(found) = found else {
        return dataset_error(DatasetError::NotFound);
    };
    let store = match datasets::store_of(&ctx.config) {
        Ok(s) => s,
        Err(e) => return dataset_error(e),
    };
    let stream = crate::services::agent_artifacts::read_stream(store, found.key, found.bytes);
    let mut res = Body::from_stream(stream).into_response();
    let h = res.headers_mut();
    h.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/vnd.apache.parquet"),
    );
    h.insert(
        axum::http::header::CONTENT_LENGTH,
        axum::http::HeaderValue::from(found.bytes),
    );
    h.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    Ok(res)
}
