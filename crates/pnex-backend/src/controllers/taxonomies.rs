//! Topic taxonomies API (media-ingest.md D168, §6): taxonomies and their
//! append-only versions. Org from the principal (R1), `can_write` at the
//! head of every write handler (R2).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::taxonomy::{TaxonomyInput, TaxonomyVersionInput};
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::media_ingest::taxonomies::{self, TaxonomyError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/taxonomies", get(list).post(create))
        .add("/taxonomies/{id}", get(one).patch(update).delete(remove))
        .add("/taxonomies/{id}/versions", get(versions).post(add_version))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(list, one, create, update, remove, versions, add_version))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
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

/// Service error → HTTP answer (field errors as `field-invalid`).
pub(crate) fn taxonomy_error(e: TaxonomyError) -> Result<Response> {
    match e {
        TaxonomyError::Invalid { field, token } => Ok(super::field_error(&field, &token)),
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

/// Page of a list (`limit`/`offset`, D14).
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PageQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// Slices a small in-memory list into the D14 envelope.
fn paged<T: serde::Serialize>(path: &str, q: &PageQuery, items: Vec<T>) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let count = items.len() as i64;
    let (skip, take) = page.slice(items.len());
    let results: Vec<T> = items.into_iter().skip(skip).take(take).collect();
    format::json(pagination::envelope(path, &[], page, count, results))
}

/// `GET /api/v1/taxonomies` — every member; current version included.
#[utoipa::path(
    get,
    path = "/api/v1/taxonomies",
    tag = "taxonomies",
    operation_id = "taxonomies_list",
    summary = "List the topic taxonomies",
    description = "Taxonomies of the org, sorted by name, each with its current version (topics) when it has one. Paginated (`limit`/`offset`).",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::taxonomy::Taxonomy>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    match taxonomies::list(&ctx.db, org.org.id).await {
        Ok(rows) => paged("/api/v1/taxonomies", &q, rows),
        Err(e) => taxonomy_error(e.into()),
    }
}

/// `GET /api/v1/taxonomies/{id}` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/taxonomies/{id}",
    tag = "taxonomies",
    operation_id = "taxonomies_get",
    summary = "Get a topic taxonomy",
    description = "One taxonomy with its current version.",
    params(("id" = String, Path, format = "uuid", description = "Taxonomy id (UUID)")),
    responses(
        (status = 200, body = pnex_core::taxonomy::Taxonomy),
        (status = 404, description = "`taxonomy-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    post,
    path = "/api/v1/taxonomies",
    tag = "taxonomies",
    operation_id = "taxonomies_create",
    summary = "Create a topic taxonomy",
    description = "Creates an empty taxonomy (version 0, no topics); add topics with `POST /taxonomies/{id}/versions`. Requires owner, admin or member role.",
    request_body = pnex_core::taxonomy::TaxonomyInput,
    responses(
        (status = 201, body = pnex_core::taxonomy::Taxonomy),
        (status = 400, description = "`field-invalid`: `errors.fields` tokens `required`, `max_length:N`, `invalid`; topic fields as `topics.<i>.<name>`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`taxonomy-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`taxonomy-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    patch,
    path = "/api/v1/taxonomies/{id}",
    tag = "taxonomies",
    operation_id = "taxonomies_update",
    summary = "Rename or describe a topic taxonomy",
    description = "Updates name and/or description (absent fields unchanged); topics are changed by appending a version. Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Taxonomy id (UUID)")),
    request_body = pnex_core::taxonomy::TaxonomyInput,
    responses(
        (status = 200, body = pnex_core::taxonomy::Taxonomy),
        (status = 400, description = "`field-invalid`: `errors.fields` tokens `required`, `max_length:N`, `invalid`; topic fields as `topics.<i>.<name>`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`taxonomy-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`taxonomy-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`taxonomy-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    delete,
    path = "/api/v1/taxonomies/{id}",
    tag = "taxonomies",
    operation_id = "taxonomies_delete",
    summary = "Delete a topic taxonomy",
    description = "Deletes the taxonomy and its versions. Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Taxonomy id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`taxonomy-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`taxonomy-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    get,
    path = "/api/v1/taxonomies/{id}/versions",
    tag = "taxonomies",
    operation_id = "taxonomies_versions_list",
    summary = "List the versions of a taxonomy",
    description = "Append-only version history, newest first, paginated (`limit`/`offset`).",
    params(("id" = String, Path, format = "uuid", description = "Taxonomy id (UUID)"), PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::taxonomy::TaxonomyVersion>),
        (status = 404, description = "`taxonomy-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    match taxonomies::versions(&ctx.db, org.org.id, id).await {
        Ok(v) => paged(&format!("/api/v1/taxonomies/{id}/versions"), &q, v),
        Err(e) => taxonomy_error(e),
    }
}

/// `POST /api/v1/taxonomies/{id}/versions` — `can_write`: appends
/// `current_version + 1` on top of `expected_version` (409 otherwise).
#[utoipa::path(
    post,
    path = "/api/v1/taxonomies/{id}/versions",
    tag = "taxonomies",
    operation_id = "taxonomies_versions_create",
    summary = "Append a version to a taxonomy",
    description = "Writes a new topic list as version `current_version + 1`, only if `expected_version` equals the current version (optimistic concurrency). Series derived from earlier versions stay untouched. Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Taxonomy id (UUID)")),
    request_body = pnex_core::taxonomy::TaxonomyVersionInput,
    responses(
        (status = 201, body = pnex_core::taxonomy::TaxonomyVersion),
        (status = 400, description = "`field-invalid`: `errors.fields` tokens `required`, `max_length:N`, `invalid`; topic fields as `topics.<i>.<name>`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`taxonomy-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`taxonomy-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`taxonomy-version-conflict` (`args.current` = current version)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
