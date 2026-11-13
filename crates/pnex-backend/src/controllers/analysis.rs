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
        .add("/analysis/named", get(named))
        .add("/analysis/named/{pack}/{key}", post(run_named))
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(submit, one, named, run_named))]
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
    description = "Queues one read-only SELECT on the tables of the org and on `objects.<type>` (the objects of a type as the caller sees them), run by the pipeline worker before any pipeline: 60 s at most, 1,000 rows returned. A marked table the caller is not cleared for fails the query with `marking-required`. Read the result with `GET /analysis/queries/{id}`. One query at a time per person; with `max_age_secs` (300 at most) the caller's own running or recent run of the same text is answered instead. Requires `analysis:run`.",
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
        input.max_age_secs,
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

/// The named queries of the packs the org installed.
async fn installed(
    db: &sea_orm::DatabaseConnection,
    org: i64,
) -> Result<Vec<pnex_core::named_query::NamedQueryView>> {
    use sea_orm::ConnectionTrait;
    let keys: Vec<String> = db
        .query_all_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT key FROM packs WHERE org_id = $1",
            vec![org.into()],
        ))
        .await?
        .iter()
        .filter_map(|r| r.try_get::<String>("", "key").ok())
        .collect();
    Ok(crate::services::ontology::packs::shipped()
        .into_iter()
        .filter(|p| keys.contains(&p.key))
        .flat_map(|p| {
            let pack = p.key.clone();
            p.queries
                .into_iter()
                .map(move |query| pnex_core::named_query::NamedQueryView {
                    pack: pack.clone(),
                    query,
                })
        })
        .collect())
}

/// `GET /api/v1/analysis/named`
#[utoipa::path(
    get,
    path = "/api/v1/analysis/named",
    tag = "analysis",
    operation_id = "analysis_named_list",
    summary = "List the named queries of the installed packs",
    description = "Each read-only query a pack the org installed declares (pipeline-engine.md D948): its key, its typed parameters and its SQL. Run one with `POST /analysis/named/{pack}/{key}`. Requires `analysis:run`.",
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::named_query::NamedQueryView>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn named(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    let results = installed(&ctx.db, org.org.id).await?;
    format::json(pnex_core::Paginated {
        count: results.len() as i64,
        next: None,
        previous: None,
        results,
    })
}

/// `POST /api/v1/analysis/named/{pack}/{key}`
#[utoipa::path(
    post,
    path = "/api/v1/analysis/named/{pack}/{key}",
    tag = "analysis",
    operation_id = "analysis_named_run",
    summary = "Run a named query of a pack",
    description = "Checks each parameter against its type and queues the pack's query as an analysis query of the caller (same rights, limits and reading as `POST /analysis/queries`): read its rows with `GET /analysis/queries/{id}`. A value never becomes SQL: it is written as a literal of its type. Requires `analysis:run`.",
    params(
        ("pack" = String, Path, description = "Key of an installed pack"),
        ("key" = String, Path, description = "Key of one of its queries")
    ),
    request_body = pnex_core::named_query::NamedQueryRun,
    responses(
        (status = 202, body = pnex_core::analysis::AnalysisQuery),
        (status = 400, description = "`field-invalid` or `field-required` on `params.<name>`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`named-query-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`analysis-busy`", body = crate::openapi::ErrorDetail),
        (status = 422, description = "`analysis-too-many-objects` (args `type`, `max`)", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`analysis-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn run_named(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((pack, key)): Path<(String, String)>,
    Json(input): Json<pnex_core::named_query::NamedQueryRun>,
) -> Result<Response> {
    let Some(q) = installed(&ctx.db, org.org.id)
        .await?
        .into_iter()
        .find(|v| v.pack == pack && v.query.key == key)
    else {
        return Err(super::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::NAMED_QUERY_NOT_FOUND,
            "named query not found",
            None,
        ));
    };
    let sql = match q.query.filled(&input.params) {
        Ok(s) => s,
        Err((name, token)) => {
            let token = match token.as_str() {
                "required" => err_codes::FIELD_REQUIRED,
                _ => err_codes::FIELD_INVALID,
            };
            return Ok(super::field_error(&format!("params.{name}"), token));
        }
    };
    submit(
        State(ctx),
        org,
        Json(AnalysisQueryInput {
            sql,
            max_age_secs: input.max_age_secs,
        }),
    )
    .await
}
