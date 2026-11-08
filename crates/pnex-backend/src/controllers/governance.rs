//! Data governance API (governance.md D798–D803): markings, clearances, and the
//! markings of a table. Org from the principal (R1), permissions applied by
//! `OrgContext` (R2), every change journaled (D803).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::governance::{ClearanceInput, Declassify, MarkingInput, MarkingsAdd};
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::datasets;
use crate::services::governance::{self, GovernanceError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/governance/markings", get(markings).post(create_marking))
        .add("/governance/markings/{id}", delete(delete_marking))
        .add("/governance/clearances", get(clearances).post(grant))
        .add("/governance/clearances/{id}/revoke", post(revoke))
        .add("/datasets/{id}/markings", post(mark_dataset))
        .add("/datasets/{id}/declassify", post(declassify_dataset))
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        markings,
        create_marking,
        delete_marking,
        clearances,
        grant,
        revoke,
        mark_dataset,
        declassify_dataset
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn governance_error(e: GovernanceError) -> Result<Response> {
    let (status, code, text) = match e {
        GovernanceError::Invalid { field, token } => return Ok(super::field_error(&field, &token)),
        GovernanceError::MarkingNotFound => (
            StatusCode::NOT_FOUND,
            err_codes::MARKING_NOT_FOUND,
            "Marking not found.",
        ),
        GovernanceError::KeyTaken => (
            StatusCode::CONFLICT,
            err_codes::MARKING_KEY_TAKEN,
            "A marking of the org already has this key.",
        ),
        GovernanceError::InUse => (
            StatusCode::CONFLICT,
            err_codes::MARKING_IN_USE,
            "Data still carries this marking.",
        ),
        GovernanceError::ClearanceNotFound => (
            StatusCode::NOT_FOUND,
            err_codes::CLEARANCE_NOT_FOUND,
            "Clearance not found.",
        ),
        GovernanceError::Db(e) => {
            tracing::error!(error = %e, "governance database error");
            return Err(Error::InternalServerError);
        }
    };
    Err(Error::CustomError(status, ErrorDetail::new(code, text)))
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PageQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/governance/markings`
#[utoipa::path(
    get,
    path = "/api/v1/governance/markings",
    tag = "governance",
    operation_id = "governance_markings_list",
    summary = "List the markings",
    description = "The compartments of the org, with how many tables carry each. Never the data. Requires `governance:read`. Paginated (`limit`/`offset`).",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::governance::Marking>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn markings(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    match governance::markings(&ctx.db, org.org.id).await {
        Ok(all) => {
            let count = all.len() as i64;
            let rows = all
                .into_iter()
                .skip(page.offset as usize)
                .take(page.limit as usize)
                .collect();
            format::json(pagination::envelope(
                "/api/v1/governance/markings",
                &[],
                page,
                count,
                rows,
            ))
        }
        Err(e) => governance_error(e.into()),
    }
}

/// `POST /api/v1/governance/markings`
#[utoipa::path(
    post,
    path = "/api/v1/governance/markings",
    tag = "governance",
    operation_id = "governance_markings_create",
    summary = "Create a marking",
    description = "Adds a compartment to the org: data that carries it is read only by people cleared for it. Requires `governance:write`.",
    request_body = pnex_core::governance::MarkingInput,
    responses(
        (status = 201, body = pnex_core::governance::Marking),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`marking-key-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create_marking(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<MarkingInput>,
) -> Result<Response> {
    match governance::create_marking(&ctx.db, org.org.id, org.auth.user.id, &input).await {
        Ok(m) => Ok((StatusCode::CREATED, Json(m)).into_response()),
        Err(e) => governance_error(e),
    }
}

/// `DELETE /api/v1/governance/markings/{id}`
#[utoipa::path(
    delete,
    path = "/api/v1/governance/markings/{id}",
    tag = "governance",
    operation_id = "governance_markings_delete",
    summary = "Delete a marking",
    description = "Removes a compartment no data carries any more, and its clearances. Requires `governance:delete`.",
    params(("id" = String, Path, format = "uuid", description = "Marking id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`marking-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`marking-in-use`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete_marking(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match governance::delete_marking(&ctx.db, org.org.id, org.auth.user.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => governance_error(e),
    }
}

/// `GET /api/v1/governance/clearances`
#[utoipa::path(
    get,
    path = "/api/v1/governance/clearances",
    tag = "governance",
    operation_id = "governance_clearances_list",
    summary = "List the clearances",
    description = "Who is cleared for which marking, for which purpose, until when; active ones first. Requires `governance:write`. Paginated (`limit`/`offset`).",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::governance::Clearance>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn clearances(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    match governance::clearances(&ctx.db, org.org.id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            "/api/v1/governance/clearances",
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => governance_error(e.into()),
    }
}

/// `POST /api/v1/governance/clearances`
#[utoipa::path(
    post,
    path = "/api/v1/governance/clearances",
    tag = "governance",
    operation_id = "governance_clearances_create",
    summary = "Clear a member for a marking",
    description = "Lets a member of the org read data that carries a marking, for a declared purpose, for 1 to 365 days. Never a service account. Requires `governance:write`.",
    request_body = pnex_core::governance::ClearanceInput,
    responses(
        (status = 201, body = pnex_core::governance::Clearance),
        (status = 400, description = "`field-invalid` (`errors.fields`; `user_id`: `not_member`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`marking-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn grant(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<ClearanceInput>,
) -> Result<Response> {
    match governance::grant(&ctx.db, org.org.id, org.auth.user.id, &input).await {
        Ok(c) => Ok((StatusCode::CREATED, Json(c)).into_response()),
        Err(e) => governance_error(e),
    }
}

/// `POST /api/v1/governance/clearances/{id}/revoke`
#[utoipa::path(
    post,
    path = "/api/v1/governance/clearances/{id}/revoke",
    tag = "governance",
    operation_id = "governance_clearances_revoke",
    summary = "Revoke a clearance",
    description = "Ends a clearance at once; it stays in the list as revoked. Requires `governance:write`.",
    params(("id" = String, Path, format = "uuid", description = "Clearance id (UUID)")),
    responses(
        (status = 204, description = "Revoked"),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`clearance-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn revoke(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match governance::revoke(&ctx.db, org.org.id, org.auth.user.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => governance_error(e),
    }
}

/// The table in the caller's org, or the answer to send.
async fn dataset_in_org(ctx: &AppContext, org: &OrgContext, id: Uuid) -> Result<(), Response> {
    match datasets::get(&ctx.db, org.org.id, id).await {
        Ok(_) => Ok(()),
        Err(_) => Err(Error::CustomError(
            StatusCode::NOT_FOUND,
            ErrorDetail::new(err_codes::DATASET_NOT_FOUND, "Dataset not found."),
        )
        .into_response()),
    }
}

/// The table after a change of its markings.
async fn dataset_answer(ctx: &AppContext, org: &OrgContext, id: Uuid) -> Result<Response> {
    match datasets::get(&ctx.db, org.org.id, id).await {
        Ok(d) => format::json(d),
        Err(_) => Err(Error::InternalServerError),
    }
}

/// `POST /api/v1/datasets/{id}/markings`
#[utoipa::path(
    post,
    path = "/api/v1/datasets/{id}/markings",
    tag = "governance",
    operation_id = "datasets_markings_add",
    summary = "Mark a dataset",
    description = "Adds markings to a dataset: reading its rows then needs a clearance for each, and what pipelines compute from it inherits them. Removing one is a declassification. Requires `governance:write`.",
    params(("id" = String, Path, format = "uuid", description = "Dataset id (UUID)")),
    request_body = pnex_core::governance::MarkingsAdd,
    responses(
        (status = 200, body = pnex_core::datasets::Dataset),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-not-found` or `marking-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn mark_dataset(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<MarkingsAdd>,
) -> Result<Response> {
    if let Err(r) = dataset_in_org(&ctx, &org, id).await {
        return Ok(r);
    }
    let added = governance::add_dataset_markings(
        &ctx.db,
        org.org.id,
        org.auth.user.id,
        id,
        &input.markings,
    )
    .await;
    match added {
        Ok(_) => dataset_answer(&ctx, &org, id).await,
        Err(e) => governance_error(e),
    }
}

/// `POST /api/v1/datasets/{id}/declassify`
#[utoipa::path(
    post,
    path = "/api/v1/datasets/{id}/declassify",
    tag = "governance",
    operation_id = "datasets_declassify",
    summary = "Declassify a dataset",
    description = "Removes one marking from a dataset, with a justification kept in the journal. A later run whose input still carries the marking puts it back. Requires `governance:write`.",
    params(("id" = String, Path, format = "uuid", description = "Dataset id (UUID)")),
    request_body = pnex_core::governance::Declassify,
    responses(
        (status = 200, body = pnex_core::datasets::Dataset),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-not-found` or `marking-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn declassify_dataset(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<Declassify>,
) -> Result<Response> {
    if let Err(r) = dataset_in_org(&ctx, &org, id).await {
        return Ok(r);
    }
    match governance::declassify_dataset(&ctx.db, org.org.id, org.auth.user.id, id, &input).await {
        Ok(_) => dataset_answer(&ctx, &org, id).await,
        Err(e) => governance_error(e),
    }
}
