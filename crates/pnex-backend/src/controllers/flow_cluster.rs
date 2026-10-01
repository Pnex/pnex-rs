//! Internal routes of the flow cluster (D106) — pod ↔ pod, never exposed
//! by the ingress. Auth: shared secret in `x-pnex-cluster-token` compared
//! to `settings.flow.cluster.token` — empty ⇒ 401 (fail-closed).
//!
//! - `POST /internal/flow-cluster/apply` — an org fragment for a worker of
//!   this process (`ApplyOrg`); errors answer an `ApplyError` body.
//! - `GET /internal/flow-cluster/flows/{id}/runtime` — runtime status.
//! - `GET /internal/flow-cluster/flows/{id}/debug[?limit=n]` — debug feed
//!   (`limit`) or node statuses + last values (no `limit`).

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use loco_rs::prelude::*;
use serde::Deserialize;

use crate::services::flow_cluster::transport::{
    local, CLUSTER_TOKEN_HEADER, CLUSTER_WORKER_HEADER,
};
use crate::services::flow_cluster::WorkerHandle;
use crate::services::flow_cluster::{ApplyError, ApplyOrg, ClusterSettings};

pub fn routes() -> Routes {
    Routes::new()
        .add("/internal/flow-cluster/apply", post(apply))
        .add("/internal/flow-cluster/flows/{id}/runtime", get(runtime))
        .add("/internal/flow-cluster/flows/{id}/debug", get(debug))
}

fn token_ok(ctx: &AppContext, headers: &HeaderMap) -> bool {
    let expected = ClusterSettings::from_config(&ctx.config).token;
    headers
        .get(CLUSTER_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| crate::auth::service_token::matches(v, &expected))
}

/// The local worker targeted by the caller (`x-pnex-cluster-worker`).
fn target(headers: &HeaderMap) -> Option<WorkerHandle> {
    headers
        .get(CLUSTER_WORKER_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(local)
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
}

fn apply_error(e: ApplyError) -> Response {
    let status = match &e {
        ApplyError::NotOwner => StatusCode::CONFLICT,
        ApplyError::Check { .. } => StatusCode::BAD_REQUEST,
        ApplyError::Unreachable { .. } | ApplyError::Runtime { .. } => {
            StatusCode::SERVICE_UNAVAILABLE
        }
    };
    (status, axum::Json(e)).into_response()
}

async fn apply(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<ApplyOrg>,
) -> Result<Response> {
    if !token_ok(&ctx, &headers) {
        return Ok(unauthorized());
    }
    // The org owner is re-checked against the database by the worker.
    let Some(node) = target(&headers) else {
        return Ok(apply_error(ApplyError::NotOwner));
    };
    Ok(match node.apply_org(req).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => apply_error(e),
    })
}

async fn runtime(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !token_ok(&ctx, &headers) {
        return Ok(unauthorized());
    }
    let status = target(&headers)
        .map(|n| n.runtime_status(id))
        .unwrap_or_default();
    format::json(status)
}

#[derive(Deserialize)]
struct DebugQuery {
    limit: Option<usize>,
}

async fn debug(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(q): Query<DebugQuery>,
) -> Result<Response> {
    if !token_ok(&ctx, &headers) {
        return Ok(unauthorized());
    }
    let entries = target(&headers)
        .map(|n| n.debug_entries(id, q.limit.map(|l| l.min(1000))))
        .unwrap_or_default();
    format::json(entries)
}
