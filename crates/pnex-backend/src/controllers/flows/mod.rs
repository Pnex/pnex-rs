//! Flows ETL (D18) — CRUD versionné + déploiement du runtime, scoping org
//! (D2). Source de vérité = la base ; le `flows.json` n'est qu'un artefact
//! projeté à chaque deploy.
//!
//! Contrat versioning :
//! - **Enregistrer ≠ déployer** : `POST`/`PATCH` créent des versions
//!   (append-only) **sans toucher au runtime** ; le déploiement est une
//!   action explicite (`POST /{id}/deploy`) qui publie une version ;
//! - concurrence optimiste : `PATCH` porte `expected_version_number`, un
//!   enregistrement périmé est rejeté **409** (exigence PRD — écart assumé
//!   vs la convention 400 du reste du repo) ;
//! - rollback = redéploiement d'une version antérieure (`/rollback`) ;
//! - l'artefact projeté contient **tous** les flows `deployed` de
//!   l'instance (le runtime exécute un flows.json multi-tabs) — le deploy
//!   reprojette donc l'ensemble, pas seulement le flow publié.
//!
//! Erreurs : forme `{"detail": ...}` (comme devices) pour les 400 champ-par-
//! champ ; violations de graphe en 400 `{"violations": [...]}` ; 409/503 via
//! `Error::CustomError` (corps Loco `{"error": code, "description": msg}`,
//! patron `orgs.rs::conflict`).

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
