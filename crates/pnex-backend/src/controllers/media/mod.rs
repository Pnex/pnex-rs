//! Média (D21) — bibliothèque d'assets versionnée par org, scoping org (D2).
//!
//! Premier chemin body-bytes du repo : l'upload est un **raw bytes
//! octet-stream** (pas de multipart — reqwest front `request_upload`),
//! métadonnées en query params. Le `kind` est auto-détecté (sniff GPano/
//! extensions — `services::media_sniff`) ; priorité : kind client > sniff >
//! `photo`. La limite d'upload est posée par handler (`DefaultBodyLimit` sur
//! les POST) — au-delà de `settings.media.max_bytes`, réponse 413 JSON
//! propre via `content_length()` ; la limite axum (max+1 Mo) reste le filet.
//!
//! Versioning append-only nettoyable (école D18 flows) :
//! - `POST /{id}/versions` crée version n+1, jamais d'écrasement ;
//! - `POST /{id}/versions/{n}/restore` re-positionne la version courante ;
//! - `DELETE /{id}/versions/{n}` purge la version **et** son blob (refus 409
//!   `{"error":"last_version"}` si seule restante) ;
//! - `DELETE /{id}` purge l'asset et tous ses blobs.
//!
//! Écriture : **DB d'abord, storage ensuite** — un blob orphelin (DB commit
//! puis put en échec) est acceptable et purgeable ; l'inverse ne le serait
//! pas (ligne sans blob).
//!
//! Errors: 400 per-field `{"<field>": msg}` (devices/flows school), 403 via
//! `can_write()`, 404 masquant le cross-org (école `device_of_org`), 409 via
//! `Error::CustomError` (patron orgs.rs::conflict).

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::prelude::*;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::{media_assets, media_versions};
use crate::services::media::{sha256_hex, MediaSettings};
use crate::services::media_sniff;
use pnex_firmware_builder::sanitize_segment;

// Submodules (pure code moves from the former single-file controller).
mod content;
mod crud;
mod dto;
mod helpers;
mod store;
mod versions;

// Private globs: bring every submodule's visible items into this module's
// namespace so `routes()` resolves its handlers and sibling submodules share
// helpers through `use super::*;`.
use content::*;
use crud::*;
use dto::*;
use helpers::*;
use versions::*;

// Disambiguate our `delete` handler from `loco_rs::prelude::delete`
// (an explicit binding shadows glob imports).
use crud::delete;

// Historical crate-level paths, preserved for external consumers.
pub(crate) use store::{write_version, IncomingVersion};

// ─────────────────────────── Routes ───────────────────────────

pub fn routes() -> Routes {
    use crate::services::compute_limits;
    // Limite d'upload : env (défaut 256 Mo) + 1 Mo de marge — le 413 JSON
    // propre part du handler (settings.media.max_bytes, vérifié sur
    // content_length) ; au-delà, la limite axum coupe (filet).
    let max = std::env::var("PNEX_MEDIA_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(crate::services::media::DEFAULT_MAX_BYTES)
        + 1024 * 1024;
    Routes::new()
        .prefix("/api/v1/media")
        .add(
            "",
            get(list)
                .post(upload)
                .layer(DefaultBodyLimit::max(max))
                .layer(axum::middleware::from_fn(compute_limits::upload_gate)),
        )
        .add("/{id}", get(detail).patch(update).delete(delete))
        .add("/{id}/content", get(content))
        .add(
            "/{id}/versions",
            get(versions)
                .post(add_version)
                .layer(DefaultBodyLimit::max(max))
                .layer(axum::middleware::from_fn(compute_limits::upload_gate)),
        )
        .add("/{id}/versions/{n}", get(version_of).delete(delete_version))
        .add("/{id}/versions/{n}/content", get(version_content))
        .add("/{id}/versions/{n}/restore", post(restore))
}
