//! Media (D21): versioned asset library per org, org-scoped (D2).
//!
//! Uploads are **raw `application/octet-stream` bodies** (no multipart,
//! frontend `request_upload`), metadata in query params. The `kind` is
//! sniffed (GPano / extensions, `services::media_sniff`); priority: client
//! kind > sniff. The upload limit is set per handler (`DefaultBodyLimit` on
//! the POSTs): above `settings.media.max_bytes` the handler answers a coded
//! 413 (`media-file-too-large`); the axum limit (max + 1 MB) is the safety net.
//!
//! Append-only, prunable versioning (flows school, D18):
//! - `POST /{id}/versions` creates version n+1, never overwrites;
//! - `POST /{id}/versions/{n}/restore` moves the current version;
//! - `DELETE /{id}/versions/{n}` purges the version **and** its blob (409
//!   `media-last-version` when it is the only one left);
//! - `DELETE /{id}` purges the asset and all its blobs.
//!
//! Writes: **storage first, DB next**: an orphan blob is acceptable and
//! purgeable, a row without its blob is not.
//!
//! Errors (D239, all `ErrorDetail`): 400 `field-invalid` with machine tokens
//! in `errors.fields`, 403 `access-denied` from the IAM extractor, 404
//! masking other orgs, 409 coded conflicts.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
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
mod analyses;
mod content;
mod crud;
mod dto;
mod helpers;
mod review;
mod search;
mod store;
mod versions;

// Private globs: bring every submodule's visible items into this module's
// namespace so `routes()` resolves its handlers and sibling submodules share
// helpers through `use super::*;`.
use analyses::*;
use content::*;
use crud::*;
use dto::*;
use helpers::*;
use review::*;
use search::*;
use versions::*;

// Disambiguate our `delete` handler from `loco_rs::prelude::delete`
// (an explicit binding shadows glob imports).
use crud::delete;

// Historical crate-level paths, preserved for external consumers.
pub(crate) use helpers::format_unsupported;
pub(crate) use store::{write_version, IncomingVersion};

// ─────────────────────────── Routes ───────────────────────────

pub fn routes() -> Routes {
    use crate::services::compute_limits;
    // Upload limit: env (default 2 GiB) + 1 MB slack; the coded 413 comes
    // from the handler (settings.media.max_bytes), the axum limit is the net.
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
        .add("/search", get(search))
        .add("/chunks/{chunk_id}", get(chunk))
        .add("/{id}/index", get(index_state).post(reindex))
        .add("/{id}/analyses", get(analyses).post(start_analyses))
        .add("/{id}/transcript", get(transcript))
        .add("/{id}/detections", get(detections))
        .add(
            "/{id}/attributions",
            get(attributions).post(create_attribution),
        )
        .add(
            "/{id}/attributions/{aid}",
            patch(update_attribution).delete(delete_attribution),
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
        .add("/{id}/versions/diff", get(versions_diff))
        .add("/{id}/versions/{n}", get(version_of).delete(delete_version))
        .add("/{id}/versions/{n}/content", get(version_content))
        .add("/{id}/versions/{n}/restore", post(restore))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        list,
        upload,
        search,
        chunk,
        index_state,
        reindex,
        analyses,
        start_analyses,
        transcript,
        detections,
        attributions,
        create_attribution,
        update_attribution,
        delete_attribution,
        detail,
        update,
        delete,
        content,
        versions,
        add_version,
        version_of,
        delete_version,
        version_content,
        versions_diff,
        restore
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
