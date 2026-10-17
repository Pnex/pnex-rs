//! `GET /api/v1/memory/keys` and `POST /api/v1/memory/values/query` — org
//! shared memory (flow `memory-write` node) for the dashboards: key picker
//! and live numeric values. Read-only, viewer included. An unreachable
//! memory store lists no key and answers the value query with a 503
//! `memory-unavailable` (cf. services/memory.rs).

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::prelude::*;
use serde::Deserialize;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::services::memory;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/memory")
        .add("/keys", get(keys))
        .add("/values/query", post(values))
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct KeysQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/memory/keys",
    tag = "memory",
    operation_id = "memory_keys",
    summary = "List the live shared-memory keys of the organisation",
    description = "Keys written by the `memory-write` flow node, sorted, with their last write time and numeric fields (dashboard key picker). Paginated (`limit`/`offset`). Read-only, any member. Degraded by design: an unreachable memory store yields an empty page, never a 5xx.",
    params(KeysQuery),
    responses((status = 200, body = pnex_core::Paginated<pnex_core::memory::MemoryKeyInfo>)),
    security(("bearer" = [], "org" = []))
)]
async fn keys(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Query(q): Query<KeysQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let all = memory::list_keys(&ctx.config, org.org.id).await;
    let (skip, take) = page.slice(all.len());
    let count = all.len() as i64;
    let rows: Vec<_> = all.into_iter().skip(skip).take(take).collect();
    format::json(pagination::envelope(
        "/api/v1/memory/keys",
        &[],
        page,
        count,
        rows,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/memory/values/query",
    tag = "memory",
    operation_id = "memory_values_query",
    summary = "Read numeric shared-memory values",
    description = "Resolves up to 64 references (`key` + optional object `field`) to their current numeric value, in request order. Read-only, any member.",
    request_body = pnex_core::memory::MemoryValuesRequest,
    responses(
        (status = 200, body = pnex_core::memory::MemoryValuesResponse),
        (status = 400, description = "`field-invalid`: more than 64 refs (`refs`: `max_length:64`)", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`memory-unavailable`: the memory store is unreachable or disabled", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn values(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Json(body): Json<pnex_core::memory::MemoryValuesRequest>,
) -> Result<Response> {
    if body.refs.len() > pnex_core::memory::MEMORY_VALUES_CAP {
        return Ok(super::field_error(
            "refs",
            &format!(
                "{}:{}",
                pnex_core::err_codes::FIELD_MAX_LENGTH,
                pnex_core::memory::MEMORY_VALUES_CAP
            ),
        ));
    }
    match memory::values(&ctx.config, org.org.id, &body.refs).await {
        Some(resp) => format::json(resp),
        None => Err(super::coded_error(
            StatusCode::SERVICE_UNAVAILABLE,
            pnex_core::err_codes::MEMORY_UNAVAILABLE,
            "The shared memory store is unavailable.",
            None,
        )),
    }
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(keys, values))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
