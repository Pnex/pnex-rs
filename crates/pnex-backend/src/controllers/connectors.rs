//! Connectors API (data-pipelines.md D352): saved access to an outside database. Org
//! from the principal (R1), permissions applied by `OrgContext` (R2), and no password
//! in any answer (R4, R16).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::connectors::ConnectorInput;
use pnex_core::err_codes;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::connectors::{self, Author, ConnectorError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/connectors", get(list).post(create))
        .add("/connectors/{id}", get(one).put(update).delete(remove))
        .add("/connectors/{id}/test", post(test))
        .add("/connectors/{id}/secret", post(secret))
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(list, one, create, update, remove, test, secret))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

pub(crate) fn connector_error(e: ConnectorError) -> Result<Response> {
    Err(match e {
        ConnectorError::Invalid { field, token } => return Ok(super::field_error(&field, &token)),
        ConnectorError::NotFound => Error::CustomError(
            StatusCode::NOT_FOUND,
            ErrorDetail::new(err_codes::CONNECTOR_NOT_FOUND, "Connector not found."),
        ),
        ConnectorError::NameTaken => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::CONNECTOR_NAME_TAKEN,
                "A connector of the org already has this name.",
            ),
        ),
        ConnectorError::VersionConflict { current } => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::CONNECTOR_VERSION_CONFLICT,
            "The connector changed since it was read; reload it.",
            Some(serde_json::json!({ "current": current })),
        ),
        ConnectorError::Store(e) => return crate::controllers::secrets::store_error(e),
        ConnectorError::Db(e) => {
            tracing::error!(error = %e, "connector database error");
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

/// `GET /api/v1/connectors`
#[utoipa::path(
    get,
    path = "/api/v1/connectors",
    tag = "connectors",
    operation_id = "connectors_list",
    summary = "List the connectors",
    description = "Saved accesses to outside databases, by name, never with their password. Requires `connectors:read`. Paginated (`limit`/`offset`).",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::connectors::Connector>),
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
    match connectors::list(&ctx.db, org.org.id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            "/api/v1/connectors",
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => connector_error(e),
    }
}

/// `POST /api/v1/connectors`
#[utoipa::path(
    post,
    path = "/api/v1/connectors",
    tag = "connectors",
    operation_id = "connectors_create",
    summary = "Create a connector",
    description = "Saves the access to an outside PostgreSQL database. The password goes to the vault, either a typed value, which needs `secrets:write`, or a secret picked from it. Requires `connectors:write`.",
    request_body = pnex_core::connectors::ConnectorInput,
    responses(
        (status = 201, body = pnex_core::connectors::Connector),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`connector-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<ConnectorInput>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match connectors::create(&ctx.db, &ring, org.org.id, author(&org), &input).await {
        Ok(c) => Ok((StatusCode::CREATED, Json(c)).into_response()),
        Err(e) => connector_error(e),
    }
}

/// `GET /api/v1/connectors/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/connectors/{id}",
    tag = "connectors",
    operation_id = "connectors_get",
    summary = "Get a connector",
    description = "One connector, never with its password. Requires `connectors:read`.",
    params(("id" = String, Path, format = "uuid", description = "Connector id (UUID)")),
    responses(
        (status = 200, body = pnex_core::connectors::Connector),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`connector-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match connectors::get(&ctx.db, org.org.id, id).await {
        Ok(c) => format::json(c),
        Err(e) => connector_error(e),
    }
}

/// `PUT /api/v1/connectors/{id}`
#[utoipa::path(
    put,
    path = "/api/v1/connectors/{id}",
    tag = "connectors",
    operation_id = "connectors_update",
    summary = "Replace a connector",
    description = "Replaces the connector, on the version that was read (`expected_version`). An absent `password` keeps the current one, unless the host or port changes: the password must then be given again, so that it never goes to a host its author did not mean it for. Requires `connectors:write`.",
    params(("id" = String, Path, format = "uuid", description = "Connector id (UUID)")),
    request_body = pnex_core::connectors::ConnectorInput,
    responses(
        (status = 200, body = pnex_core::connectors::Connector),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`connector-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`connector-name-taken` or `connector-version-conflict` (args `current`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<ConnectorInput>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match connectors::update(&ctx.db, &ring, org.org.id, author(&org), id, &input).await {
        Ok(c) => format::json(c),
        Err(e) => connector_error(e),
    }
}

/// `DELETE /api/v1/connectors/{id}`
#[utoipa::path(
    delete,
    path = "/api/v1/connectors/{id}",
    tag = "connectors",
    operation_id = "connectors_delete",
    summary = "Delete a connector",
    description = "Removes the connector and its dedicated secret. An import still naming it then fails with `connector-not-found`. Requires `connectors:delete`.",
    params(("id" = String, Path, format = "uuid", description = "Connector id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`connector-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match connectors::delete(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => connector_error(e),
    }
}

/// `POST /api/v1/connectors/{id}/test`
#[utoipa::path(
    post,
    path = "/api/v1/connectors/{id}/test",
    tag = "connectors",
    operation_id = "connectors_test",
    summary = "Test a connector",
    description = "Tries to connect. Returns whether it worked and a machine code, never data nor the text of the outside server. Requires `connectors:test`.",
    params(("id" = String, Path, format = "uuid", description = "Connector id (UUID)")),
    responses(
        (status = 200, body = pnex_core::connectors::ConnectorTest),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`connector-not-found`", body = crate::openapi::ErrorDetail)
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
    match connectors::test(&ctx.db, &ring, org.org.id, id).await {
        Ok(t) => format::json(t),
        Err(e) => connector_error(e),
    }
}

/// `POST /api/v1/connectors/{id}/secret`
#[utoipa::path(
    post,
    path = "/api/v1/connectors/{id}/secret",
    tag = "connectors",
    operation_id = "connectors_secret",
    summary = "Generate a connector secret",
    description = "Gives the connector a new random secret, kept in the vault and returned this once; the previous one stops working at once. For a webhook, the sender signs its calls with it. Requires `connectors:write`.",
    params(("id" = String, Path, format = "uuid", description = "Connector id (UUID)")),
    responses(
        (status = 200, body = pnex_core::connectors::ConnectorSecret),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`connector-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn secret(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match connectors::regenerate_secret(&ctx.db, &ring, org.org.id, author(&org), id).await {
        Ok(secret) => format::json(pnex_core::connectors::ConnectorSecret { secret }),
        Err(e) => connector_error(e),
    }
}
