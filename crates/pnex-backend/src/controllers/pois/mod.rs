//! Carte POI-first (D35–D39) : POI (`map_pins`), clustering backend (D37),
//! liens typés (D39) et positions GPS devices (D38), scoping org (D2).
//! Endpoints **additifs** — ne jamais bumper `pnex_api_contract::CONTRACT`.
//!
//! Contrat : `docs/contracts/viz.http` ; PRD : `docs/architecture/viz-bases.md`.
//! - listes = enveloppe D14 `{count, next, previous, results}` ;
//! - 400 per-field `{"<field>": msg}` (flows school);
//! - 404 masqué cross-org (POI/lien introuvable ou d'une autre org) ;
//! - cluster = items individuels (≤ `CLUSTER_INDIVIDUAL_MAX` points dans la
//!   bbox) sinon clusters numérotés (centroïde + échantillon) ;
//! - écritures `can_write()` (owner|admin), logique portée par
//!   `services/pois.rs` (point unique, école `services/flow.rs`).

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
    // Les segments statiques (`cluster`, `placements`) sont déclarés avant
    // le paramétrique `{id}` (matchit : statique prioritaire — école sites).
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
