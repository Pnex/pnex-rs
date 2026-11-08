//! Read-only SQL API (analysis.md D825–D828). Org from the principal (R1), the
//! permission applied by `OrgContext` (R2), what a query reads fixed by the server
//! under the caller's rights and markings; a query is its author's only.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use pnex_core::analysis::AnalysisQueryInput;
use pnex_core::err_codes;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::services::analysis::{self, AnalysisError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/analysis/queries", post(submit))
        .add("/analysis/queries/{id}", get(one))
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(submit, one))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn analysis_error(e: AnalysisError) -> Result<Response> {
    let (status, code, args) = match e {
        AnalysisError::Invalid { field, token } => return Ok(super::field_error(field, &token)),
        AnalysisError::Busy => (StatusCode::CONFLICT, err_codes::ANALYSIS_BUSY, None),
        AnalysisError::NotFound => (StatusCode::NOT_FOUND, err_codes::ANALYSIS_NOT_FOUND, None),
        AnalysisError::Coded(code, args) => {
            let status = if code == err_codes::ANALYSIS_TOO_MANY_OBJECTS {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            };
            (status, code, Some(args))
        }
        AnalysisError::Db(e) => {
            tracing::error!(error = %e, "analysis database error");
            return Err(Error::InternalServerError);
        }
    };
    Err(super::coded_error(
        status,
        code,
        code.replace('-', " "),
        args,
    ))
}

/// `POST /api/v1/analysis/queries`
#[utoipa::path(
    post,
    path = "/api/v1/analysis/queries",
    tag = "analysis",
    operation_id = "analysis_queries_create",
    summary = "Send a read-only SQL query",
    description = "Queues one read-only SELECT on the tables of the org and on `objects.<type>` (the objects of a type as the caller sees them), run by the pipeline worker before any pipeline: 60 s at most, 1,000 rows returned. A marked table the caller is not cleared for fails the query with `marking-required`. Read the result with `GET /analysis/queries/{id}`. One query at a time per person. Requires `analysis:run`.",
    request_body = pnex_core::analysis::AnalysisQueryInput,
    responses(
        (status = 202, body = pnex_core::analysis::AnalysisQuery),
        (status = 400, description = "`field-invalid` on `sql`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`analysis-busy`", body = crate::openapi::ErrorDetail),
        (status = 422, description = "`analysis-too-many-objects` (args `type`, `max`)", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`analysis-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn submit(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<AnalysisQueryInput>,
) -> Result<Response> {
    let store = match crate::services::datasets::store_of(&ctx.config) {
        Ok(s) => s,
        Err(_) => {
            return analysis_error(AnalysisError::Coded(
                err_codes::ANALYSIS_UNAVAILABLE,
                serde_json::Value::Null,
            ))
        }
    };
    let hide = super::ontology::redaction(&ctx, &org)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let sent = analysis::submit(
        &ctx.db,
        &store,
        org.org.id,
        org.auth.user.id,
        hide,
        &input.sql,
    )
    .await;
    match sent {
        Ok(q) => Ok((StatusCode::ACCEPTED, Json(q)).into_response()),
        Err(e) => analysis_error(e),
    }
}

/// `GET /api/v1/analysis/queries/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/analysis/queries/{id}",
    tag = "analysis",
    operation_id = "analysis_queries_get",
    summary = "Read a query and its rows",
    description = "The state of one of the caller's queries; once `done`, its columns and first rows as text (`truncated` when there were more), once `failed`, its code: `analysis-sql-invalid` (args `reason`), `analysis-table-unknown` (args `table`), `marking-required` (args `table`), `analysis-timeout`, `analysis-result-too-large`, `pipeline-memory-exceeded`. Kept a day. Requires `analysis:run`.",
    params(("id" = String, Path, format = "uuid", description = "Query id (UUID)")),
    responses(
        (status = 200, body = pnex_core::analysis::AnalysisQuery),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`analysis-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match analysis::get(&ctx.db, org.org.id, org.auth.user.id, id).await {
        Ok(q) => format::json(q),
        Err(e) => analysis_error(e),
    }
}
