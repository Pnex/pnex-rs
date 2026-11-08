//! Rules API (analysis.md D829–D830). Org from the principal (R1), permissions
//! applied by `OrgContext` (R2); a rule runs with the rights of who saved it last,
//! a replay with the rights of who asks.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::rules::RuleInput;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::rules::{self, RuleError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/rules", get(list).post(create))
        .add("/rules/{id}", get(one).put(update).delete(remove))
        .add("/rules/{id}/replay", post(replay))
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(list, create, one, update, remove, replay))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn rule_error(e: RuleError) -> Result<Response> {
    Err(match e {
        RuleError::Invalid { field, token } => return Ok(super::field_error(&field, &token)),
        RuleError::NotFound => Error::CustomError(
            StatusCode::NOT_FOUND,
            ErrorDetail::new(err_codes::RULE_NOT_FOUND, "Rule not found."),
        ),
        RuleError::NameTaken => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::RULE_NAME_TAKEN,
                "A rule of the org already has this name.",
            ),
        ),
        RuleError::VersionConflict { current } => super::coded_error(
            StatusCode::CONFLICT,
            err_codes::RULE_VERSION_CONFLICT,
            "The rule changed since it was read; reload it.",
            Some(serde_json::json!({ "current": current })),
        ),
        RuleError::Db(e) => {
            tracing::error!(error = %e, "rules database error");
            Error::InternalServerError
        }
    })
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PageQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/rules`
#[utoipa::path(
    get,
    path = "/api/v1/rules",
    tag = "rules",
    operation_id = "rules_list",
    summary = "List the rules",
    description = "The rules of the org by name, with whether each held at the last pass. Requires `rules:read`. Paginated (`limit`/`offset`).",
    params(PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::rules::Rule>),
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
    match rules::list(&ctx.db, org.org.id, page.limit, page.offset).await {
        Ok((count, rows)) => format::json(pagination::envelope(
            "/api/v1/rules",
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => rule_error(e.into()),
    }
}

/// `POST /api/v1/rules`
#[utoipa::path(
    post,
    path = "/api/v1/rules",
    tag = "rules",
    operation_id = "rules_create",
    summary = "Create a rule",
    description = "A condition on the objects of a type (filters of the ontology query) or on a series (threshold held for some minutes), evaluated every five minutes with the rights of who saved it, notifying in-app or by a channel of the org when it turns true. Never writes anything else. Requires `rules:write`.",
    request_body = pnex_core::rules::RuleInput,
    responses(
        (status = 201, body = pnex_core::rules::Rule),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`rule-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<RuleInput>,
) -> Result<Response> {
    match rules::create(&ctx.db, org.org.id, org.auth.user.id, &input).await {
        Ok(r) => Ok((StatusCode::CREATED, Json(r)).into_response()),
        Err(e) => rule_error(e),
    }
}

/// `GET /api/v1/rules/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/rules/{id}",
    tag = "rules",
    operation_id = "rules_get",
    summary = "Get a rule",
    description = "One rule of the org. Requires `rules:read`.",
    params(("id" = String, Path, format = "uuid", description = "Rule id (UUID)")),
    responses(
        (status = 200, body = pnex_core::rules::Rule),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`rule-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match rules::get(&ctx.db, org.org.id, id).await {
        Ok(r) => format::json(r),
        Err(e) => rule_error(e),
    }
}

/// `PUT /api/v1/rules/{id}`
#[utoipa::path(
    put,
    path = "/api/v1/rules/{id}",
    tag = "rules",
    operation_id = "rules_update",
    summary = "Replace a rule",
    description = "Replaces the rule on the version read (`expected_version`); it then runs with the rights of who saved it. A changed condition starts again from what is true now. Requires `rules:write`.",
    params(("id" = String, Path, format = "uuid", description = "Rule id (UUID)")),
    request_body = pnex_core::rules::RuleInput,
    responses(
        (status = 200, body = pnex_core::rules::Rule),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`rule-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`rule-name-taken` or `rule-version-conflict` (args `current`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<RuleInput>,
) -> Result<Response> {
    match rules::update(&ctx.db, org.org.id, id, org.auth.user.id, &input).await {
        Ok(r) => format::json(r),
        Err(e) => rule_error(e),
    }
}

/// `DELETE /api/v1/rules/{id}`
#[utoipa::path(
    delete,
    path = "/api/v1/rules/{id}",
    tag = "rules",
    operation_id = "rules_delete",
    summary = "Delete a rule",
    description = "Removes the rule; nothing it sent is withdrawn. Requires `rules:delete`.",
    params(("id" = String, Path, format = "uuid", description = "Rule id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`rule-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match rules::delete(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => rule_error(e),
    }
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
struct ReplayInput {
    /// A window preset back from now: `1h`, `6h`, `24h`, `7d` or `30d`.
    window: String,
}

/// `POST /api/v1/rules/{id}/replay`
#[utoipa::path(
    post,
    path = "/api/v1/rules/{id}/replay",
    tag = "rules",
    operation_id = "rules_replay_preview",
    summary = "Replay a rule on the past",
    description = "When the rule would have fired over a window back from now, from the history of the objects and the series, with the rights of the caller. Sends nothing, stores nothing. `complete` is false when the history store did not answer. Requires `rules:read`.",
    params(("id" = String, Path, format = "uuid", description = "Rule id (UUID)")),
    request_body = ReplayInput,
    responses(
        (status = 200, body = pnex_core::rules::RuleReplay),
        (status = 400, description = "`field-invalid` on `window`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`rule-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn replay(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<ReplayInput>,
) -> Result<Response> {
    let rule = match rules::get(&ctx.db, org.org.id, id).await {
        Ok(r) => r,
        Err(e) => return rule_error(e),
    };
    let hide = super::ontology::redaction(&ctx, &org)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let client = super::ontology::o2(&ctx);
    match rules::replay(
        &ctx.db,
        client.as_ref(),
        org.org.id,
        &rule,
        &hide,
        &input.window,
    )
    .await
    {
        Ok(r) => format::json(r),
        Err(e) => rule_error(e),
    }
}
