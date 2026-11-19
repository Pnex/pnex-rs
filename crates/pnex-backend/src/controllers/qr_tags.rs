//! QR tags API (parts-docs.md D962–D963). The org comes from the principal
//! (R1): a token of another org is unknown (404), never a hint that it
//! exists. Permissions `qr-tags:read` and `qr-tags:write` (R2).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::qr::{QrLinkInput, QrSheetInput, QrState};
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::qr_tags::{self, QrError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/qr-tags", get(list))
        .add("/qr-tags/sheets", post(sheet))
        .add("/qr-tags/objects/{id}", post(for_object))
        .add("/qr-tags/{token}", get(one))
        .add("/qr-tags/{token}/link", post(link))
        .add("/qr-tags/{token}/revoke", post(revoke))
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(list, sheet, for_object, one, link, revoke))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn qr_error(e: QrError) -> Result<Response> {
    Err(match e {
        QrError::Invalid { field, token } => return Ok(super::field_error(&field, &token)),
        QrError::NotFound => Error::CustomError(
            StatusCode::NOT_FOUND,
            ErrorDetail::new(err_codes::QR_TAG_NOT_FOUND, "QR tag not found."),
        ),
        QrError::NotBlank => Error::CustomError(
            StatusCode::CONFLICT,
            ErrorDetail::new(
                err_codes::QR_TAG_NOT_BLANK,
                "Only a blank QR tag can be linked; this one is linked or revoked.",
            ),
        ),
        QrError::Db(e) => {
            tracing::error!(error = %e, "qr tags database error");
            Error::InternalServerError
        }
    })
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    /// Only the tags of this object.
    #[param(value_type = Option<String>, format = "uuid")]
    object_id: Option<Uuid>,
    /// Only the tags in this state.
    state: Option<QrState>,
    /// Only these tokens, comma-separated (a sheet to print), 120 at most.
    tokens: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/qr-tags`
#[utoipa::path(
    get,
    path = "/api/v1/qr-tags",
    tag = "qr-tags",
    operation_id = "qr_tags_list",
    summary = "List the QR tags",
    description = "QR tags of the org, newest first, optionally of one object, in one state (`blank`, `linked`, `revoked`) or among given tokens, with the object a linked tag opens. Requires `qr-tags:read`. Paginated (`limit`/`offset`).",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::qr::QrTag>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let tokens: Option<Vec<String>> = q.tokens.as_deref().map(|t| {
        t.split(',')
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(str::to_string)
            .collect()
    });
    if tokens
        .as_ref()
        .is_some_and(|t| t.len() > pnex_core::qr::SHEET_MAX as usize)
    {
        return Ok(super::field_error(
            "tokens",
            &format!(
                "{}:{}",
                err_codes::FIELD_MAX_LENGTH,
                pnex_core::qr::SHEET_MAX
            ),
        ));
    }
    match qr_tags::list(
        &ctx.db,
        org.org.id,
        q.object_id,
        q.state,
        tokens,
        page.limit,
        page.offset,
    )
    .await
    {
        Ok((count, rows)) => format::json(pagination::envelope(
            "/api/v1/qr-tags",
            &[],
            page,
            count,
            rows,
        )),
        Err(e) => qr_error(e),
    }
}

/// `POST /api/v1/qr-tags/sheets`
#[utoipa::path(
    post,
    path = "/api/v1/qr-tags/sheets",
    tag = "qr-tags",
    operation_id = "qr_tags_sheets_create",
    summary = "Create a sheet of blank QR tags",
    description = "Creates `count` blank QR tags (1 to 120) to print ahead and link on site by a scan. Requires `qr-tags:write`.",
    request_body = QrSheetInput,
    responses(
        (status = 201, body = pnex_core::qr::QrSheet),
        (status = 400, description = "`field-invalid` (`errors.fields`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn sheet(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<QrSheetInput>,
) -> Result<Response> {
    match qr_tags::sheet(&ctx.db, org.org.id, input.count).await {
        Ok(tags) => {
            let sheet_id = tags
                .first()
                .and_then(|t| t.sheet_id.clone())
                .unwrap_or_default();
            Ok((
                StatusCode::CREATED,
                Json(pnex_core::qr::QrSheet { sheet_id, tags }),
            )
                .into_response())
        }
        Err(e) => qr_error(e),
    }
}

/// `POST /api/v1/qr-tags/objects/{id}`
#[utoipa::path(
    post,
    path = "/api/v1/qr-tags/objects/{id}",
    tag = "qr-tags",
    operation_id = "qr_tags_for_object",
    summary = "Get or make the QR tag of an object",
    description = "The live QR tag of an object of the org, made and linked now when it has none (201). Requires `qr-tags:write`.",
    params(("id" = String, Path, format = "uuid", description = "Object id (UUID)")),
    responses(
        (status = 200, body = pnex_core::qr::QrTag),
        (status = 201, body = pnex_core::qr::QrTag),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`qr-tag-not-found`: no such object in the org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn for_object(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match qr_tags::for_object(&ctx.db, org.org.id, org.auth.user.id, id).await {
        Ok((tag, true)) => Ok((StatusCode::CREATED, Json(tag)).into_response()),
        Ok((tag, false)) => format::json(tag),
        Err(e) => qr_error(e),
    }
}

/// `GET /api/v1/qr-tags/{token}`
#[utoipa::path(
    get,
    path = "/api/v1/qr-tags/{token}",
    tag = "qr-tags",
    operation_id = "qr_tags_get",
    summary = "Resolve a scanned QR tag",
    description = "The tag a scan read, in the caller's org only: the object a linked tag opens, a blank tag waiting to be linked, or a revoked one. A token of another org is unknown. Requires `qr-tags:read`.",
    params(("token" = String, Path, description = "Token printed in the QR")),
    responses(
        (status = 200, body = pnex_core::qr::QrTag),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`qr-tag-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(token): Path<String>,
) -> Result<Response> {
    match qr_tags::get(&ctx.db, org.org.id, &token).await {
        Ok(t) => format::json(t),
        Err(e) => qr_error(e),
    }
}

/// `POST /api/v1/qr-tags/{token}/link`
#[utoipa::path(
    post,
    path = "/api/v1/qr-tags/{token}/link",
    tag = "qr-tags",
    operation_id = "qr_tags_link",
    summary = "Link a blank QR tag to an object",
    description = "Links a blank tag, scanned on site, to an object of the org (any type): the tag then opens it. A linked or revoked tag is refused. Requires `qr-tags:write`.",
    params(("token" = String, Path, description = "Token printed in the QR")),
    request_body = QrLinkInput,
    responses(
        (status = 200, body = pnex_core::qr::QrTag),
        (status = 400, description = "`field-invalid` (`errors.fields`: `object_id` unknown)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`qr-tag-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`qr-tag-not-blank`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn link(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(token): Path<String>,
    Json(input): Json<QrLinkInput>,
) -> Result<Response> {
    let Ok(object) = Uuid::parse_str(input.object_id.trim()) else {
        return Ok(super::field_error(
            "object_id",
            err_codes::FIELD_INVALID_UUID,
        ));
    };
    match qr_tags::link(&ctx.db, org.org.id, org.auth.user.id, &token, object).await {
        Ok(t) => format::json(t),
        Err(e) => qr_error(e),
    }
}

/// `POST /api/v1/qr-tags/{token}/revoke`
#[utoipa::path(
    post,
    path = "/api/v1/qr-tags/{token}/revoke",
    tag = "qr-tags",
    operation_id = "qr_tags_revoke",
    summary = "Revoke a QR tag",
    description = "A lost or torn label stops opening its object; revoking twice changes nothing. Requires `qr-tags:write`.",
    params(("token" = String, Path, description = "Token printed in the QR")),
    responses(
        (status = 200, body = pnex_core::qr::QrTag),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`qr-tag-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn revoke(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(token): Path<String>,
) -> Result<Response> {
    match qr_tags::revoke(&ctx.db, org.org.id, &token).await {
        Ok(t) => format::json(t),
        Err(e) => qr_error(e),
    }
}
