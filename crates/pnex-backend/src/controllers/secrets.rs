//! Org secrets vault API (secrets.md §5, D110–D117).
//!
//! - `GET /api/v1/secrets`: every role; viewers only get ids and names.
//!   Never a value, for anyone.
//! - `POST`, `PATCH /{id}`, `DELETE /{id}`: owner/admin (D117). Delete is
//!   refused while the secret is used (409 `secret-in-use`).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch};
use axum::Json;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::{org_secrets, users};
use crate::services::secrets::store::{self, StoreError, Writer};
use crate::services::secrets::Keyring;
use pnex_core::{err_codes, OrgSecret, OrgSecretInput};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/secrets")
        .add("/", get(list).post(create))
        .add("/{id}", patch(update).delete(remove))
}

fn detail(status: StatusCode, code: &str, msg: &str) -> Error {
    Error::CustomError(
        status,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

/// HTTP mapping of vault errors, shared with the consumers that embed a
/// secret field (notify channels, flows, WiFi, LLM providers).
pub(crate) fn store_error(e: StoreError) -> Result<Response> {
    match e {
        StoreError::NotFound => Err(detail(
            StatusCode::NOT_FOUND,
            err_codes::SECRET_NOT_FOUND,
            "Secret not found.",
        )),
        StoreError::InUse => Err(detail(
            StatusCode::CONFLICT,
            err_codes::SECRET_IN_USE,
            "This secret is still in use.",
        )),
        StoreError::NameTaken => Err(detail(
            StatusCode::CONFLICT,
            err_codes::SECRET_NAME_TAKEN,
            "A secret with this name already exists.",
        )),
        StoreError::WriteForbidden => Err(forbidden()),
        StoreError::DestinationLocked { field } => Err(detail(
            StatusCode::FORBIDDEN,
            err_codes::SECRET_DESTINATION_LOCKED,
            &format!(
                "Field `{field}` uses a vault secret: only an owner or admin can bind a secret to a field and its destination."
            ),
        )),
        StoreError::Invalid { field, token } => Ok(crate::controllers::field_error(field, &token)),
        StoreError::Keyring(err) => {
            tracing::error!(%err, "secrets keyring unavailable");
            Err(Error::InternalServerError)
        }
        StoreError::Crypto(err) => {
            tracing::error!(%err, "secret could not be decrypted");
            Err(Error::InternalServerError)
        }
        StoreError::Db(err) => {
            tracing::error!(%err, "secrets store database error");
            Err(Error::InternalServerError)
        }
    }
}

fn forbidden() -> Error {
    detail(
        StatusCode::FORBIDDEN,
        err_codes::SECRET_WRITE_FORBIDDEN,
        "Owner or admin role required to create, change or delete a secret.",
    )
}

pub(crate) fn keyring(ctx: &AppContext) -> std::result::Result<Keyring, StoreError> {
    Ok(Keyring::from_config(&ctx.config)?)
}

pub(crate) fn writer(org: &OrgContext) -> Writer {
    Writer {
        org_id: Some(org.org.id),
        user_id: Some(org.auth.user.id),
    }
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    /// Case-insensitive search on the name.
    search: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/secrets` — name ASC, `search` on the name.
#[utoipa::path(
    get,
    path = "/api/v1/secrets",
    tag = "secrets",
    operation_id = "secrets_list",
    summary = "List the org secrets",
    description = "Vault secrets of the org, ordered by name, paginated (`limit`/`offset`), optional name search. Never returns a value. Viewers only get id and name; other roles also get description, usages and last writer email.",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::OrgSecret>)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut query = org_secrets::Entity::find()
        .filter(org_secrets::Column::OrgId.eq(org.org.id))
        .order_by_asc(org_secrets::Column::Name);
    let mut filters = Vec::new();
    if let Some(pat) = pagination::sql_search_pattern(q.search.as_deref()) {
        query = query.filter(pagination::sql_contains(
            (org_secrets::Entity, org_secrets::Column::Name),
            &pat,
        ));
        filters.push((
            "search".to_string(),
            q.search.clone().unwrap_or_default().trim().to_string(),
        ));
    }
    let (count, rows) = pagination::sql_page(&ctx.db, query, page)
        .await
        .map_err(|_| Error::InternalServerError)?;

    let details = !org.is_viewer();
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let mut usages = if details {
        match store::usages_of(&ctx.db, &ids).await {
            Ok(u) => u,
            Err(e) => return store_error(e),
        }
    } else {
        Default::default()
    };
    let user_ids: Vec<i64> = rows.iter().filter_map(|r| r.updated_by).collect();
    let emails: std::collections::HashMap<i64, String> = if details && !user_ids.is_empty() {
        users::Entity::find()
            .filter(users::Column::Id.is_in(user_ids))
            .all(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .into_iter()
            .map(|u| (u.id, u.email))
            .collect()
    } else {
        Default::default()
    };
    let results: Vec<OrgSecret> = rows
        .into_iter()
        .map(|r| {
            let mut dto = dto(&r);
            if details {
                dto.usages = usages.remove(&r.id).unwrap_or_default();
                dto.updated_by = r.updated_by.and_then(|id| emails.get(&id).cloned());
            } else {
                dto.description = None;
            }
            dto
        })
        .collect();
    Ok(format::json(pagination::envelope(
        "/api/v1/secrets",
        &filters,
        page,
        count,
        results,
    ))
    .into_response())
}

fn dto(r: &org_secrets::Model) -> OrgSecret {
    OrgSecret {
        id: r.id,
        name: r.name.clone(),
        description: r.description.clone(),
        usages: Vec::new(),
        updated_by: None,
        created_at: r.created_at.to_rfc3339(),
        updated_at: r.updated_at.to_rfc3339(),
    }
}

/// `POST /api/v1/secrets` — value required.
#[utoipa::path(
    post,
    path = "/api/v1/secrets",
    tag = "secrets",
    operation_id = "secrets_create",
    summary = "Create a secret",
    description = "Stores a new encrypted secret in the org vault (`value` required). Owner or admin only. The answer never echoes the value.",
    request_body = pnex_core::OrgSecretInput,
    responses(
        (status = 201, body = pnex_core::OrgSecret),
        (status = 400, description = "`field-invalid`: `errors.fields` maps each invalid field to a token (`required`, `max_length:255`...)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Not owner/admin (`secret-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Name already used (`secret-name-taken`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<OrgSecretInput>,
) -> Result<Response> {
    if !org.can_manage_secrets() {
        return Err(forbidden());
    }
    let ring = match keyring(&ctx) {
        Ok(r) => r,
        Err(e) => return store_error(e),
    };
    let value = input.value.as_deref().unwrap_or_default();
    match store::create(
        &ctx.db,
        &ring,
        writer(&org),
        &input.name,
        input.description.as_deref(),
        value,
    )
    .await
    {
        Ok(row) => Ok((StatusCode::CREATED, format::json(dto(&row))).into_response()),
        Err(e) => store_error(e),
    }
}

/// `PATCH /api/v1/secrets/{id}` — rename, redescribe, optionally replace
/// the value (absent or empty = keep).
#[utoipa::path(
    patch,
    path = "/api/v1/secrets/{id}",
    tag = "secrets",
    operation_id = "secrets_update",
    summary = "Update a secret",
    description = "Renames and redescribes a secret; replaces its value only when `value` is present and non-empty. Owner or admin only.",
    params(("id" = String, Path, format = "uuid", description = "Secret id")),
    request_body = pnex_core::OrgSecretInput,
    responses(
        (status = 200, body = pnex_core::OrgSecret),
        (status = 400, description = "`field-invalid`: `errors.fields` maps each invalid field to a token (`required`, `max_length:255`...)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "Not owner/admin (`secret-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown secret (`secret-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Name already used (`secret-name-taken`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<OrgSecretInput>,
) -> Result<Response> {
    if !org.can_manage_secrets() {
        return Err(forbidden());
    }
    let ring = match keyring(&ctx) {
        Ok(r) => r,
        Err(e) => return store_error(e),
    };
    let value = input.value.as_deref().filter(|v| !v.is_empty());
    match store::update(
        &ctx.db,
        &ring,
        writer(&org),
        id,
        &input.name,
        input.description.as_deref(),
        value,
    )
    .await
    {
        Ok(row) => Ok(format::json(dto(&row)).into_response()),
        Err(e) => store_error(e),
    }
}

/// `DELETE /api/v1/secrets/{id}` — 409 while used.
#[utoipa::path(
    delete,
    path = "/api/v1/secrets/{id}",
    tag = "secrets",
    operation_id = "secrets_delete",
    summary = "Delete a secret",
    description = "Deletes a secret of the org vault. Owner or admin only. Refused while a consumer (notify channel, flow, WiFi, LLM provider, media stream, geo provider) still references it.",
    params(("id" = String, Path, format = "uuid", description = "Secret id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Not owner/admin (`secret-write-forbidden`)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown secret (`secret-not-found`)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "Secret still in use (`secret-in-use`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if !org.can_manage_secrets() {
        return Err(forbidden());
    }
    match store::delete(&ctx.db, Some(org.org.id), id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => store_error(e),
    }
}

/// `POST /api/v1/system/secrets/rekey` — platform admin (secrets.md S8):
/// rewrites every vault row under the write key of `PNEX_SECRETS_KEYS`.
/// Run only once every pod has restarted with the new keyring.
#[utoipa::path(
    post,
    path = "/api/v1/system/secrets/rekey",
    tag = "system",
    operation_id = "system_secrets_rekey",
    summary = "Re-encrypt the secrets vault under the current write key",
    description = "Platform admin only. Rewrites every vault row under the write key of the server keyring (`PNEX_SECRETS_KEYS`) after a master key rotation; rows under a key missing from the keyring are left untouched. Run only once every server instance has restarted with the new keyring.",
    responses(
        (status = 200, body = pnex_core::SecretsRekeyReport),
        (status = 403, description = "Not a platform admin", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = []))
)]
pub async fn platform_rekey(
    _admin: crate::auth::PlatformAdmin,
    State(ctx): State<AppContext>,
) -> Result<Response> {
    use crate::services::secrets::rekey;
    let ring = match keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return store_error(e),
    };
    let report = match rekey::rekey_singleton(&ctx.db, &ring).await {
        Ok(report) => report,
        Err(e) => return store_error(e),
    };
    let remaining = match rekey::stats(&ctx.db, &ring).await {
        Ok(stats) => stats.stale,
        Err(e) => return store_error(e),
    };
    format::json(pnex_core::SecretsRekeyReport {
        rewritten: report.rewritten,
        unreadable: report.unreadable,
        skipped: report.skipped,
        remaining,
    })
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(list, create, update, remove))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
