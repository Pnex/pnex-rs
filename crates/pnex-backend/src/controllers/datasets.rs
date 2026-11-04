//! Datasets API (data-pipelines.md D569): catalogue, CSV load, sample. Org from
//! the principal (R1), permissions applied by `OrgContext` (R2).

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::datasets::{DatasetIdentityInput, DatasetInput, DatasetTransactionKind};
use pnex_core::err_codes;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::datasets::csv::CsvError;
use crate::services::datasets::{self, DatasetError};

const DEFAULT_SAMPLE_ROWS: usize = 50;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/datasets", get(list).post(create))
        .add("/datasets/{id}", get(one).delete(remove))
        .add("/datasets/{id}/identity", put(set_identity))
        .add("/datasets/{id}/transactions", get(transactions))
        // A raw body stream: no body limit layer applies, `load_csv` holds the bound.
        .add("/datasets/{id}/transactions/csv", post(load_csv))
        .add("/datasets/{id}/sample", get(sample))
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
            transactions,
            load_csv,
            sample
        ),
        components(schemas(DatasetTransactionKind))
    )]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn coded(status: StatusCode, code: &str, text: &str, args: Option<serde_json::Value>) -> Error {
    super::coded_error(status, code, text, args)
}

fn dataset_error(e: DatasetError) -> Result<Response> {
    let csv = |args| {
        coded(
            StatusCode::BAD_REQUEST,
            err_codes::DATASET_CSV_INVALID,
            "The file is not a CSV this server can read.",
            Some(args),
        )
    };
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
        DatasetError::Csv(CsvError::Empty) => csv(serde_json::json!({ "reason": "empty" })),
        DatasetError::Csv(CsvError::RecordTooLarge { line }) => {
            csv(serde_json::json!({ "reason": "record_too_large", "line": line }))
        }
        DatasetError::Csv(CsvError::TooManyColumns { line }) => {
            csv(serde_json::json!({ "reason": "too_many_columns", "line": line }))
        }
        DatasetError::Csv(CsvError::ColumnNameTooLong) => {
            csv(serde_json::json!({ "reason": "column_name_too_long" }))
        }
        DatasetError::Csv(CsvError::DuplicateColumn { name }) => {
            csv(serde_json::json!({ "reason": "duplicate_column", "column": name }))
        }
        DatasetError::Csv(CsvError::FieldCount {
            line,
            expected,
            got,
        }) => csv(serde_json::json!({
            "reason": "field_count", "line": line, "expected": expected, "got": got
        })),
        DatasetError::SchemaMismatch { columns } => coded(
            StatusCode::CONFLICT,
            err_codes::DATASET_SCHEMA_MISMATCH,
            "The columns of the file are not those of the dataset.",
            Some(serde_json::json!({ "columns": columns })),
        ),
        DatasetError::QuotaExceeded => coded(
            StatusCode::CONFLICT,
            err_codes::DATASET_QUOTA,
            "The org's dataset storage is full.",
            Some(serde_json::json!({ "max": datasets::QUOTA_BYTES })),
        ),
        DatasetError::SourceTooLarge => coded(
            StatusCode::PAYLOAD_TOO_LARGE,
            err_codes::DATASET_SOURCE_TOO_LARGE,
            "The file is over the size a load accepts.",
            Some(serde_json::json!({ "max": datasets::SOURCE_MAX_BYTES })),
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
        Some(d) if d.len() == 1 && d.is_ascii() && d != "\"" && d != "\n" && d != "\r" => {
            d.bytes().next()
        }
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
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
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
