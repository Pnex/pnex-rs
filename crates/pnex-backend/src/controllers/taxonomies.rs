//! Topic taxonomies API (media-ingest.md D168, §6): taxonomies and their
//! append-only versions. Org from the principal (R1), `can_write` at the
//! head of every write handler (R2).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::taxonomy::{TaxonomyInput, TaxonomyVersionInput};
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::services::media_ingest::taxonomies::{self, TaxonomyError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/taxonomies", get(list).post(create))
        .add("/taxonomies/{id}", get(one).patch(update).delete(remove))
        .add("/taxonomies/{id}/versions", get(versions).post(add_version))
}

fn require_write(org: &OrgContext) -> Result<()> {
    if org.can_write() {
        return Ok(());
    }
    Err(Error::CustomError(
        StatusCode::FORBIDDEN,
        ErrorDetail::new(
            err_codes::TAXONOMY_WRITE_FORBIDDEN,
            "Viewer role: taxonomies are read-only.",
        ),
    ))
}

/// Service error → HTTP answer (field errors as `{field: token}`).
pub(crate) fn taxonomy_error(e: TaxonomyError) -> Result<Response> {
    match e {
        TaxonomyError::Invalid { field, token } => Ok((
            StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({ field: token })),
        )
            .into_response()),
        TaxonomyError::NotFound => Err(Error::CustomError(
            StatusCode::NOT_FOUND,
            ErrorDetail::new(err_codes::TAXONOMY_NOT_FOUND, "Taxonomy not found."),
        )),
        TaxonomyError::NameTaken => Err(Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::TAXONOMY_NAME_TAKEN,
                "A taxonomy of the org already has this name.",
            ),
        )),
        TaxonomyError::VersionConflict { current } => Err(super::coded_error(
            StatusCode::CONFLICT,
            err_codes::TAXONOMY_VERSION_CONFLICT,
            "The taxonomy changed since it was read; reload it.",
            Some(serde_json::json!({ "current": current.to_string() })),
        )),
        TaxonomyError::Db(e) => {
            tracing::error!(error = %e, "taxonomy database error");
            Err(Error::InternalServerError)
        }
    }
}

/// `GET /api/v1/taxonomies` — every member; current version included.
async fn list(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    match taxonomies::list(&ctx.db, org.org.id).await {
        Ok(rows) => format::json(rows),
        Err(e) => taxonomy_error(e.into()),
    }
}

/// `GET /api/v1/taxonomies/{id}` — every member.
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match taxonomies::get(&ctx.db, org.org.id, id).await {
        Ok(t) => format::json(t),
        Err(e) => taxonomy_error(e),
    }
}

/// `POST /api/v1/taxonomies` — `can_write`; created empty (version 0).
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<TaxonomyInput>,
) -> Result<Response> {
    require_write(&org)?;
    match taxonomies::create(&ctx.db, org.org.id, Some(org.auth.user.id), &input).await {
        Ok(row) => Ok((
            StatusCode::CREATED,
            format::json(taxonomies::view(&row, None)),
        )
            .into_response()),
        Err(e) => taxonomy_error(e),
    }
}

/// `PATCH /api/v1/taxonomies/{id}` — `can_write`: name, description.
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<TaxonomyInput>,
) -> Result<Response> {
    require_write(&org)?;
    if let Err(e) = taxonomies::update(&ctx.db, org.org.id, id, &input).await {
        return taxonomy_error(e);
    }
    one(State(ctx), org, Path(id)).await
}

/// `DELETE /api/v1/taxonomies/{id}` — `can_write`.
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_write(&org)?;
    match taxonomies::delete(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => taxonomy_error(e),
    }
}

/// `GET /api/v1/taxonomies/{id}/versions` — every member, newest first.
async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match taxonomies::versions(&ctx.db, org.org.id, id).await {
        Ok(v) => format::json(v),
        Err(e) => taxonomy_error(e),
    }
}

/// `POST /api/v1/taxonomies/{id}/versions` — `can_write`: appends
/// `current_version + 1` on top of `expected_version` (409 otherwise).
async fn add_version(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<TaxonomyVersionInput>,
) -> Result<Response> {
    require_write(&org)?;
    match taxonomies::add_version(&ctx.db, org.org.id, id, Some(org.auth.user.id), &input).await {
        Ok(v) => Ok((
            StatusCode::CREATED,
            format::json(taxonomies::version_view(&v)),
        )
            .into_response()),
        Err(e) => taxonomy_error(e),
    }
}
