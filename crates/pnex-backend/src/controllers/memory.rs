//! `GET /api/v1/memory/keys` and `POST /api/v1/memory/values` — org shared
//! memory (flow `memory-write` node) for the dashboards: key picker and
//! live numeric values. Read-only, viewer included; degraded by design
//! (never a 500) — cf. services/memory.rs.

use axum::extract::State;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::prelude::*;

use crate::auth::OrgContext;
use crate::services::memory;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/memory")
        .add("/keys", get(keys))
        .add("/values", post(values))
}

async fn keys(org: OrgContext, State(ctx): State<AppContext>) -> Result<Response> {
    format::json(memory::list_keys(&ctx.config, org.org.id).await)
}

async fn values(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Json(body): Json<pnex_core::memory::MemoryValuesRequest>,
) -> Result<Response> {
    if body.refs.len() > pnex_core::memory::MEMORY_VALUES_CAP {
        return Err(Error::BadRequest(format!(
            "refs: at most {} per request",
            pnex_core::memory::MEMORY_VALUES_CAP
        )));
    }
    format::json(memory::values(&ctx.config, org.org.id, &body.refs).await)
}
