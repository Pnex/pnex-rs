//! ETL flows (D18) — versioned CRUD + runtime deployment, org scoping
//! (D2). Source of truth = the database; `flows.json` is only an artifact
//! projected at each deploy.
//!
//! Versioning contract:
//! - **Save ≠ deploy**: `POST`/`PATCH` create versions
//!   (append-only) **without touching the runtime**; deployment is an
//!   explicit action (`POST /{id}/deploy`) that publishes a version;
//! - optimistic concurrency: `PATCH` carries `expected_version_number`, a
//!   stale save is rejected **409** (PRD requirement — deliberate deviation
//!   from the 400 convention of the rest of the repo);
//! - rollback = redeploy of an earlier version (`/rollback`);
//! - the projected artifact contains **all** the `deployed` flows of
//!   the instance (the runtime runs a multi-tab flows.json) — the deploy
//!   therefore reprojects the whole set, not only the published flow.
//!
//! Errors: `{"<field>": "<token>"}` for field-by-field 400s;
//! graph violations as 400 `{"violations": [...]}`; 409/503 via
//! `Error::CustomError` (Loco body `{"error": code, "description": msg}`,
//! `orgs.rs::conflict` pattern).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use serde::Deserialize;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::{flow_versions, flows};
use crate::services::flow::FlowSettings;
use crate::services::flow_supervisor;
use crate::services::openobserve;
use pnex_core::{FlowArtifactMeta, FlowGraph};

// Submodules (pure code moves from the former single-file controller).
mod artifact;
mod crud;
mod dto;
mod error;
mod lifecycle;

// Private globs: bring every submodule's visible items into this module's
// namespace so `routes()` resolves its handlers and sibling submodules share
// helpers through `use super::*;`.
use artifact::*;
use crud::*;
use dto::*;
use error::*;
use lifecycle::*;

// Disambiguate our `delete` handler from `loco_rs::prelude::delete`
// (an explicit binding shadows glob imports).
use crud::delete;

// Historical crate-level paths, preserved for external consumers.
pub(crate) use artifact::{deployed_flows_with_versions, project_org, reproject_and_signal};
pub(crate) use lifecycle::{
    flows_impacted_by_pin, flows_writing_pin, pin_write_owners_by_device, stop_flows_reading_pin,
};

// ─────────────────────────── Routes ───────────────────────────

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/flows")
        .add("", get(list).post(create))
        .add("/{id}", get(detail).patch(update).delete(delete))
        .add("/{id}/versions", get(versions))
        .add("/{id}/versions/{version_number}", get(version_detail))
        .add("/{id}/deploy", post(deploy))
        .add("/{id}/rollback", post(rollback))
        .add("/{id}/runtime", get(runtime))
        .add("/{id}/debug", get(debug))
        .add("/{id}/node-status", get(node_status))
        .add("/{id}/stop", post(stop))
        .add("/{id}/start", post(start))
        .add("/{id}/restart", post(restart))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        list,
        create,
        detail,
        update,
        delete,
        versions,
        version_detail,
        deploy,
        rollback,
        runtime,
        debug,
        node_status,
        stop,
        start,
        restart
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
