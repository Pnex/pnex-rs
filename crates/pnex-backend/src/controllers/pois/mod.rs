//! POI-first map (D35–D39): POI (`map_pins`), backend clustering (D37),
//! typed links (D39) and device GPS positions (D38), org scoping (D2).
//! **Additive** endpoints — never bump `pnex_api_contract::CONTRACT`.
//!
//! Contract: `docs/contracts/viz.http`; PRD: `docs/architecture/viz-bases.md`.
//! - lists = D14 envelope `{count, next, previous, results}`;
//! - errors are `ErrorDetail` (`field-invalid` with machine tokens);
//! - cross-org 404 hidden (POI/link not found or from another org);
//! - cluster = individual items (≤ `CLUSTER_INDIVIDUAL_MAX` points in the
//!   bbox) otherwise numbered clusters (centroid + sample);
//! - writes need their IAM permission (`pois:write`), logic in
//!   `services/pois.rs` (single point, `services/flow.rs` pattern).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::{device_placements, device_positions, map_pins};
use crate::services::pois as svc;
use pnex_core::err_codes;

// Submodules (pure code moves from the former single-file controller).
mod crud;
mod dto;
mod errors;
mod payloads;
mod placements;
mod positions;

// Private globs: bring every submodule's visible items into this module's
// namespace so `routes()` resolves its handlers and sibling submodules share
// helpers through `use super::*;`.
use crud::*;
use dto::*;
use errors::*;
use payloads::*;
use placements::*;
use positions::*;

// Disambiguate our `delete` handler from `loco_rs::prelude::delete`
// (an explicit binding shadows glob imports).
use crud::delete;

// ─────────────────────────── routes ───────────────────────────

pub fn routes() -> Routes {
    // Static segments (`cluster`, `placements`) are declared before
    // the parametric `{id}` (matchit: static wins — sites pattern).
    Routes::new()
        .prefix("/api/v1")
        .add("/pois/cluster", get(cluster))
        .add(
            "/pois/placements/{id}",
            patch(update_placement).delete(delete_placement),
        )
        .add("/pois", get(list).post(create))
        .add("/pois/{id}", get(detail).patch(update).delete(delete))
        .add("/pois/{id}/devices", post(attach_device))
        .add("/device-positions", get(list_positions))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        cluster,
        update_placement,
        delete_placement,
        list,
        create,
        detail,
        update,
        delete,
        attach_device,
        list_positions
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
