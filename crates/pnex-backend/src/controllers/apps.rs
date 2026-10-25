//! Applications API (external-apps.md D433) under `/api/v1/apps`. Org
//! from the principal (R1). Reading needs `apps:read`; declaring, changing
//! and deleting need `apps:write` / `apps:delete` (Admin). `GET /apps/me`
//! answers the application a service account signs in for.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use pnex_core::apps::{AppInput, AppView};
use pnex_core::err_codes;
use uuid::Uuid;

use super::coded_error;
use super::ontology::{paged, PageQuery};
use crate::auth::OrgContext;
use crate::services::apps::{self, AppError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/apps")
        .add("/", get(list).post(create))
        .add("/me", get(me))
        .add("/{id}", get(one).put(update).delete(remove))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(list, me, one, create, update, remove))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn app_error(e: AppError) -> Result<Response> {
    match e {
        AppError::Fields(fields) => Ok(super::fields_error(
            StatusCode::BAD_REQUEST,
            fields
                .into_iter()
                .map(|(k, v)| (k, serde_json::Value::String(v)))
                .collect(),
        )),
        AppError::NotFound => Err(coded_error(
            StatusCode::NOT_FOUND,
            err_codes::APP_NOT_FOUND,
            "Application not found.",
            None,
        )),
        AppError::VersionConflict { current } => Err(coded_error(
            StatusCode::CONFLICT,
            err_codes::APP_VERSION_CONFLICT,
            "The application changed since it was read; reload it.",
            Some(serde_json::json!({ "current": current.to_string() })),
        )),
        AppError::Db(e) => Err(e.into()),
    }
}

const INPUT_400: &str = "`field-invalid`: `key` (`required`, `invalid`, `unique`), `name` (`required`, `max_length:120`), `description` (`max_length:2000`), `service_account_id` (`invalid-uuid`, `not-found`, `unique`), `scope.types` / `scope.link_types` (`invalid`, `not-found`), `scope.media_kinds` / `scope.media_max_bytes` (`invalid`), `expected_version` (`required`, on a replace)";

#[utoipa::path(
    get,
    path = "/api/v1/apps",
    tag = "apps",
    operation_id = "apps_list",
    summary = "List the applications",
    description = "Applications declared in the org (external programs built on the ontology), sorted by name, paginated. Any org member.",
    params(PageQuery),
    responses((status = 200, body = pnex_core::Paginated<AppView>)),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    match apps::list(&ctx.db, org.org.id).await {
        Ok(rows) => paged("/api/v1/apps", &[], &q, Ok(rows)),
        Err(e) => app_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/apps/me",
    tag = "apps",
    operation_id = "apps_me",
    summary = "Get the calling application",
    description = "The application whose service account signs this request: its key and its scope. 404 `app-not-found` for a person or a service account no application uses. Any org member, service accounts included.",
    responses(
        (status = 200, body = AppView),
        (status = 404, description = "`app-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn me(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    let Some(sp) = org.auth.service.as_ref() else {
        return app_error(AppError::NotFound);
    };
    match apps::of_service_account(&ctx.db, org.org.id, sp.id).await {
        Ok(Some(a)) => format::json(a),
        Ok(None) => app_error(AppError::NotFound),
        Err(e) => app_error(e),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/apps/{id}",
    tag = "apps",
    operation_id = "apps_get",
    summary = "Get an application",
    description = "An application of the org with its scope. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Application id")),
    responses(
        (status = 200, body = AppView),
        (status = 404, description = "`app-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match apps::get(&ctx.db, org.org.id, id).await {
        Ok(a) => format::json(a),
        Err(e) => app_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/apps",
    tag = "apps",
    operation_id = "apps_create",
    summary = "Declare an application",
    description = "Declares an external application of the org: a key, the service account it signs in with (one application per account) and its scope (object types, link types, media kinds and size it copies). The change feed keeps to that scope when the application calls. Owner or admin.",
    request_body = AppInput,
    responses(
        (status = 201, body = AppView),
        (status = 400, description = INPUT_400, body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<AppInput>,
) -> Result<Response> {
    match apps::create(&ctx.db, org.org.id, org.auth.user.id, &input).await {
        Ok(a) => Ok((StatusCode::CREATED, format::json(a)).into_response()),
        Err(e) => app_error(e),
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/apps/{id}",
    tag = "apps",
    operation_id = "apps_update",
    summary = "Replace an application",
    description = "Replaces an application on the version it was read on (`expected_version`, 409 `app-version-conflict` otherwise). Owner or admin.",
    params(("id" = String, Path, format = "uuid", description = "Application id")),
    request_body = AppInput,
    responses(
        (status = 200, body = AppView),
        (status = 400, description = INPUT_400, body = crate::openapi::ErrorDetail),
        (status = 404, description = "`app-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`app-version-conflict`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<AppInput>,
) -> Result<Response> {
    match apps::update(&ctx.db, org.org.id, id, &input).await {
        Ok(a) => format::json(a),
        Err(e) => app_error(e),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/apps/{id}",
    tag = "apps",
    operation_id = "apps_delete",
    summary = "Delete an application",
    description = "Removes the declaration; its service account and keys stay (delete them on the Access page). Owner or admin.",
    params(("id" = String, Path, format = "uuid", description = "Application id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "`app-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match apps::delete(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => app_error(e),
    }
}
