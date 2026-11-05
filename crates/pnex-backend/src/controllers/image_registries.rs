//! Image registries API (agent-environments.md D703). Org from the principal (R1),
//! permissions applied by `OrgContext` (R2), and no secret in any answer (R4, R16).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::image_registries::ImageRegistryInput;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::image_registries::{self, Author, ImageRegistryError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/image-registries", get(list).post(create))
        .add(
            "/image-registries/{id}",
            get(one).put(update).delete(remove),
        )
        .add("/image-registries/{id}/test", post(test))
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(list, one, create, update, remove, test))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn registry_error(e: ImageRegistryError) -> Result<Response> {
    Err(match e {
        ImageRegistryError::Invalid { field, token } => {
            return Ok(super::field_error(&field, &token))
        }
        ImageRegistryError::NotFound => Error::CustomError(
            StatusCode::NOT_FOUND,
            ErrorDetail::new(err_codes::IMAGE_REGISTRY_NOT_FOUND, "Registry not found."),
        ),
        ImageRegistryError::NameTaken => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::IMAGE_REGISTRY_NAME_TAKEN,
                "A registry of the org already has this name.",
            ),
        ),
        ImageRegistryError::InUse => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::IMAGE_REGISTRY_IN_USE,
                "An environment still names this registry.",
            ),
        ),
        ImageRegistryError::VersionConflict { current } => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::IMAGE_REGISTRY_VERSION_CONFLICT,
            "The registry changed since it was read; reload it.",
            Some(serde_json::json!({ "current": current })),
        ),
        ImageRegistryError::Store(e) => return crate::controllers::secrets::store_error(e),
        ImageRegistryError::Db(e) => {
            tracing::error!(error = %e, "image registry database error");
            Error::InternalServerError
        }
    })
}

fn author(org: &OrgContext) -> Author {
    Author {
        user_id: org.auth.user.id,
        can_write_secrets: org.can("secrets:write"),
    }
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PageQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/image-registries`
#[utoipa::path(
    get,
    path = "/api/v1/image-registries",
    tag = "image-registries",
    operation_id = "image_registries_list",
    summary = "List the image registries",
    description = "Saved accesses to OCI registries, never with their secrets. Requires `image-registries:read`. Paginated (`limit`/`offset`).",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::image_registries::ImageRegistry>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    match image_registries::list(&ctx.db, org.org.id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            "/api/v1/image-registries",
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => registry_error(e),
    }
}

/// `POST /api/v1/image-registries`
#[utoipa::path(
    post,
    path = "/api/v1/image-registries",
    tag = "image-registries",
    operation_id = "image_registries_create",
    summary = "Create an image registry",
    description = "Saves the access to an OCI registry and repository. The read secret and the write secret are distinct and go to the vault, each a typed value, which needs `secrets:write`, or a secret picked from it. Requires `image-registries:write`.",
    request_body = pnex_core::image_registries::ImageRegistryInput,
    responses(
        (status = 201, body = pnex_core::image_registries::ImageRegistry),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`image-registry-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<ImageRegistryInput>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match image_registries::create(&ctx.db, &ring, org.org.id, author(&org), &input).await {
        Ok(r) => Ok((StatusCode::CREATED, Json(r)).into_response()),
        Err(e) => registry_error(e),
    }
}

/// `GET /api/v1/image-registries/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/image-registries/{id}",
    tag = "image-registries",
    operation_id = "image_registries_get",
    summary = "Get an image registry",
    description = "One registry, never with its secrets. Requires `image-registries:read`.",
    params(("id" = String, Path, format = "uuid", description = "Registry id (UUID)")),
    responses(
        (status = 200, body = pnex_core::image_registries::ImageRegistry),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`image-registry-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match image_registries::get(&ctx.db, org.org.id, id).await {
        Ok(r) => format::json(r),
        Err(e) => registry_error(e),
    }
}

/// `PUT /api/v1/image-registries/{id}`
#[utoipa::path(
    put,
    path = "/api/v1/image-registries/{id}",
    tag = "image-registries",
    operation_id = "image_registries_update",
    summary = "Replace an image registry",
    description = "Replaces the registry, on the version that was read (`expected_version`). An absent secret keeps the current one, unless the host or the TLS setting changes: it must then be given again, so that it never goes where its author did not mean it to. Requires `image-registries:write`.",
    params(("id" = String, Path, format = "uuid", description = "Registry id (UUID)")),
    request_body = pnex_core::image_registries::ImageRegistryInput,
    responses(
        (status = 200, body = pnex_core::image_registries::ImageRegistry),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`image-registry-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`image-registry-name-taken` or `image-registry-version-conflict` (args `current`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<ImageRegistryInput>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match image_registries::update(&ctx.db, &ring, org.org.id, author(&org), id, &input).await {
        Ok(r) => format::json(r),
        Err(e) => registry_error(e),
    }
}

/// `DELETE /api/v1/image-registries/{id}`
#[utoipa::path(
    delete,
    path = "/api/v1/image-registries/{id}",
    tag = "image-registries",
    operation_id = "image_registries_delete",
    summary = "Delete an image registry",
    description = "Removes the registry and its dedicated secrets. Images stay in the registry. Requires `image-registries:delete`.",
    params(("id" = String, Path, format = "uuid", description = "Registry id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`image-registry-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`image-registry-in-use`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match image_registries::delete(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => registry_error(e),
    }
}

/// `POST /api/v1/image-registries/{id}/test`
#[utoipa::path(
    post,
    path = "/api/v1/image-registries/{id}/test",
    tag = "image-registries",
    operation_id = "image_registries_test",
    summary = "Test an image registry",
    description = "Tries the pull right on the repository with the read secret, and the push right with the write secret when one is set, by opening then cancelling an upload. No image moves. Returns machine codes, never the text of the outside server. Requires `image-registries:test`.",
    params(("id" = String, Path, format = "uuid", description = "Registry id (UUID)")),
    responses(
        (status = 200, body = pnex_core::image_registries::ImageRegistryTest),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`image-registry-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn test(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match image_registries::test(&ctx.db, &ring, org.org.id, id).await {
        Ok(t) => format::json(t),
        Err(e) => registry_error(e),
    }
}
