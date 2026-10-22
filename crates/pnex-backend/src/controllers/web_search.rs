//! Web search providers of the org (web-presence.md D360): CRUD with the
//! key in the vault, one default, a Test button. Org from the principal
//! (R1), `web-search:*` checked by the IAM extractor (R2).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::web_search::{WebSearchProvider, WebSearchProviderInput, WebSearchTestResult};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::web_search::{self as providers, ProviderError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/web-search/providers", get(list).post(create))
        .add(
            "/web-search/providers/{id}",
            axum::routing::put(update).delete(remove),
        )
        .add("/web-search/providers/{id}/test", post(test))
}

fn provider_error(e: ProviderError) -> Result<Response> {
    match e {
        ProviderError::Invalid { field, token } => Ok(super::field_error(field, &token)),
        ProviderError::NotFound => Err(super::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::WEB_SEARCH_PROVIDER_NOT_FOUND,
            "Web search provider not found.",
            None,
        )),
        ProviderError::NameTaken => Err(super::coded_error(
            StatusCode::CONFLICT,
            err_codes::WEB_SEARCH_PROVIDER_NAME_TAKEN,
            "A web search provider with this name already exists.",
            None,
        )),
        ProviderError::Store(e) => crate::controllers::secrets::store_error(e),
    }
}

fn author(org: &OrgContext) -> providers::Author {
    providers::Author {
        user_id: Some(org.auth.user.id),
        can_write_secrets: org.can("secrets:write"),
    }
}

async fn view(
    ctx: &AppContext,
    org_id: i64,
    row: &providers::Row,
    status: StatusCode,
) -> Result<Response> {
    match providers::views(&ctx.db, org_id, std::slice::from_ref(row)).await {
        Ok(mut v) if !v.is_empty() => Ok((status, format::json(v.remove(0))).into_response()),
        Ok(_) => Err(Error::InternalServerError),
        Err(e) => crate::controllers::secrets::store_error(e),
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PageQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/web-search/providers",
    tag = "web-search",
    operation_id = "web_search_providers_list",
    summary = "List web search providers",
    description = "Search providers the organization's web_search flow nodes query, with the default. The key is a vault reference, never its value. The platform provides none. Any member.",
    params(PageQuery),
    responses((status = 200, body = pnex_core::Paginated<WebSearchProvider>)),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = match providers::list(&ctx.db, org.org.id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "web search providers database error");
            return Err(Error::InternalServerError);
        }
    };
    let views = match providers::views(&ctx.db, org.org.id, &rows).await {
        Ok(v) => v,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    let count = views.len() as i64;
    let (skip, take) = page.slice(views.len());
    let results: Vec<WebSearchProvider> = views.into_iter().skip(skip).take(take).collect();
    format::json(pagination::envelope(
        "/api/v1/web-search/providers",
        &[],
        page,
        count,
        results,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/web-search/providers",
    tag = "web-search",
    operation_id = "web_search_providers_create",
    summary = "Create a web search provider",
    description = "Registers a provider: `serper` (key required, default address) or `searxng` (address of the instance, JSON output enabled, key optional, sent as a bearer token). The address must pass the egress policy. A typed `api_key` value is stored in the org vault. The first provider, or one with `is_default`, becomes the org default. Owner or admin only.",
    request_body = WebSearchProviderInput,
    responses(
        (status = 201, body = WebSearchProvider),
        (status = 400, description = "`field-invalid`: `errors.fields` maps the field to a token", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`web-search-provider-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<WebSearchProviderInput>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(r) => r,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match providers::create(&ctx.db, &ring, org.org.id, author(&org), &input).await {
        Ok(row) => view(&ctx, org.org.id, &row, StatusCode::CREATED).await,
        Err(e) => provider_error(e),
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/web-search/providers/{id}",
    tag = "web-search",
    operation_id = "web_search_providers_update",
    summary = "Replace a web search provider",
    description = "Full replacement; an absent `api_key` keeps the current one. Owner or admin only.",
    params(("id" = String, Path, format = "uuid", description = "Web search provider id")),
    request_body = WebSearchProviderInput,
    responses(
        (status = 200, body = WebSearchProvider),
        (status = 400, description = "`field-invalid`: `errors.fields` maps the field to a token", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`web-search-provider-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`web-search-provider-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<WebSearchProviderInput>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(r) => r,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match providers::update(&ctx.db, &ring, org.org.id, author(&org), id, &input).await {
        Ok(row) => view(&ctx, org.org.id, &row, StatusCode::OK).await,
        Err(e) => provider_error(e),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/web-search/providers/{id}",
    tag = "web-search",
    operation_id = "web_search_providers_delete",
    summary = "Delete a web search provider",
    description = "Deletes the provider and its dedicated key. Flow nodes that named it fail with `web-search-no-provider` until edited. Owner or admin only.",
    params(("id" = String, Path, format = "uuid", description = "Web search provider id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "`web-search-provider-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match providers::delete(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => provider_error(e),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/web-search/providers/{id}/test",
    tag = "web-search",
    operation_id = "web_search_providers_test",
    summary = "Test a web search provider",
    description = "From the server: sends one real query (it may be billed by the provider) and answers whether it worked, the number of results and the latency; nothing is stored. Owner or admin only.",
    params(("id" = String, Path, format = "uuid", description = "Web search provider id")),
    responses(
        (status = 200, body = WebSearchTestResult),
        (status = 404, description = "`web-search-provider-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn test(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let row = match providers::find(&ctx.db, org.org.id, id).await {
        Ok(r) => r,
        Err(e) => return provider_error(e),
    };
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(r) => r,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    format::json(providers::test(&ctx.db, &ring, org.org.id, &row).await)
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(list, create, update, remove, test))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
