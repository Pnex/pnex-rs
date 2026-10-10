//! Geo providers and proxy — `/api/v1/geo/*` (geo-layers.md §8, phase F).
//! Providers: CRUD owner/admin, list every member (secret = vault
//! reference). Proxy: every member, the browser never calls a provider
//! (L20). Org always from the principal (R1).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::geo::{GeoProviderInput, RouteProfile};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::services::geo::providers::{self, Author, ProviderError};
use crate::services::geo::proxy::{self, Deps, ProxyError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/geo")
        .add("/providers", get(list).post(create))
        .add("/providers/{id}", put(update).delete(delete))
        .add("/providers/{id}/test", post(test))
        .add("/basemaps", get(basemaps))
        .add("/geocode", get(geocode))
        .add("/reverse", get(reverse))
        .add("/route", post(route))
}

fn custom(status: StatusCode, code: &str, msg: &str, args: Option<serde_json::Value>) -> Error {
    Error::CustomError(
        status,
        loco_rs::controller::ErrorDetail {
            error: Some(code.to_string()),
            description: Some(msg.to_string()),
            errors: args.map(|a| serde_json::json!({ "args": a })),
        },
    )
}

fn provider_error(e: ProviderError) -> Result<Response> {
    match e {
        ProviderError::Invalid { field, token } => Ok((
            StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({ field: token })),
        )
            .into_response()),
        ProviderError::NotFound => Err(custom(
            StatusCode::NOT_FOUND,
            err_codes::GEO_PROVIDER_NOT_FOUND,
            "Geo provider not found.",
            None,
        )),
        ProviderError::NameTaken => Err(custom(
            StatusCode::CONFLICT,
            err_codes::GEO_PROVIDER_NAME_TAKEN,
            "A geo provider with this name already exists.",
            None,
        )),
        ProviderError::Store(e) => crate::controllers::secrets::store_error(e),
    }
}

fn proxy_error(e: ProxyError) -> Result<Response> {
    match e {
        ProxyError::NotConfigured(cap) => Err(custom(
            StatusCode::CONFLICT,
            err_codes::GEO_NOT_CONFIGURED,
            "No default geo provider is configured for this use.",
            Some(serde_json::json!({ "capability": cap.as_str() })),
        )),
        ProxyError::RateLimited(after) => {
            let secs = after.as_secs().max(1);
            let mut resp = custom(
                StatusCode::TOO_MANY_REQUESTS,
                err_codes::GEO_RATE_LIMITED,
                "Geo provider rate limit reached.",
                Some(serde_json::json!({ "retry_after_s": secs.to_string() })),
            )
            .into_response();
            if let Ok(v) = secs.to_string().parse() {
                resp.headers_mut()
                    .insert(axum::http::header::RETRY_AFTER, v);
            }
            Ok(resp)
        }
        ProxyError::Upstream(detail) => Err(custom(
            StatusCode::BAD_GATEWAY,
            err_codes::GEO_PROVIDER_FAILED,
            &detail,
            None,
        )),
        ProxyError::BadRequest(field) => Ok((
            StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({ field: err_codes::FIELD_INVALID })),
        )
            .into_response()),
        ProxyError::Store(e) => crate::controllers::secrets::store_error(e),
    }
}

fn db_error(e: sea_orm::DbErr) -> Error {
    tracing::error!(error = %e, "geo providers database error");
    Error::InternalServerError
}

fn require_manage(org: &OrgContext) -> Result<()> {
    if org.can_administer() {
        Ok(())
    } else {
        Err(custom(
            StatusCode::FORBIDDEN,
            err_codes::GEO_PROVIDER_FORBIDDEN,
            "Owner or admin role required to manage geo providers.",
            None,
        ))
    }
}

fn author(org: &OrgContext) -> Author {
    Author {
        user_id: Some(org.auth.user.id),
        can_write_secrets: org.can_manage_secrets(),
    }
}

async fn one(
    ctx: &AppContext,
    org_id: i64,
    row: &crate::models::_entities::geo_providers::Model,
    status: StatusCode,
) -> Result<Response> {
    match providers::views(&ctx.db, org_id, std::slice::from_ref(row)).await {
        Ok(mut views) if !views.is_empty() => {
            Ok((status, format::json(views.remove(0))).into_response())
        }
        Ok(_) => Err(Error::InternalServerError),
        Err(e) => crate::controllers::secrets::store_error(e),
    }
}

/// `GET /api/v1/geo/providers` — every member.
async fn list(org: OrgContext, State(ctx): State<AppContext>) -> Result<Response> {
    let rows = providers::list(&ctx.db, org.org.id)
        .await
        .map_err(db_error)?;
    match providers::views(&ctx.db, org.org.id, &rows).await {
        Ok(views) => format::json(views),
        Err(e) => crate::controllers::secrets::store_error(e),
    }
}

/// `POST /api/v1/geo/providers` — owner/admin.
async fn create(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Json(input): Json<GeoProviderInput>,
) -> Result<Response> {
    require_manage(&org)?;
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match providers::create(&ctx.db, &ring, org.org.id, author(&org), &input).await {
        Ok(row) => one(&ctx, org.org.id, &row, StatusCode::CREATED).await,
        Err(e) => provider_error(e),
    }
}

/// `PUT /api/v1/geo/providers/{id}` — owner/admin.
async fn update(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Path(id): Path<Uuid>,
    Json(input): Json<GeoProviderInput>,
) -> Result<Response> {
    require_manage(&org)?;
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match providers::update(&ctx.db, &ring, org.org.id, author(&org), id, &input).await {
        Ok(row) => one(&ctx, org.org.id, &row, StatusCode::OK).await,
        Err(e) => provider_error(e),
    }
}

/// `DELETE /api/v1/geo/providers/{id}` — owner/admin.
async fn delete(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_manage(&org)?;
    match providers::delete(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => provider_error(e),
    }
}

/// `POST /api/v1/geo/providers/{id}/test` — owner/admin (L23, L28).
async fn test(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_manage(&org)?;
    let row = match providers::find(&ctx.db, org.org.id, id).await {
        Ok(row) => row,
        Err(e) => return provider_error(e),
    };
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    let deps = Deps {
        db: &ctx.db,
        config: &ctx.config,
        ring: &ring,
    };
    match proxy::test(&deps, &row).await {
        Ok(ms) => format::json(serde_json::json!({ "ok": true, "latency_ms": ms })),
        Err(e) => proxy_error(e),
    }
}

/// `GET /api/v1/geo/basemaps` — every member; style URLs carry the
/// basemap's browser key.
async fn basemaps(org: OrgContext, State(ctx): State<AppContext>) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match providers::basemaps(&ctx.db, &ring, org.org.id).await {
        Ok(maps) => format::json(maps),
        Err(e) => crate::controllers::secrets::store_error(e),
    }
}

#[derive(Deserialize)]
struct GeocodeQuery {
    q: String,
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    5
}

#[derive(Deserialize)]
struct ReverseQuery {
    lat: f64,
    lon: f64,
}

#[derive(Deserialize)]
struct RouteBody {
    /// `[lat, lon]` pairs, at least two.
    points: Vec<[f64; 2]>,
    #[serde(default)]
    profile: RouteProfile,
}

fn proxied<T: serde::Serialize>(r: std::result::Result<T, ProxyError>) -> Result<Response> {
    match r {
        Ok(v) => format::json(v),
        Err(e) => proxy_error(e),
    }
}

/// `GET /api/v1/geo/geocode?q=&limit=` — every member.
async fn geocode(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Query(q): Query<GeocodeQuery>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    let d = Deps {
        db: &ctx.db,
        config: &ctx.config,
        ring: &ring,
    };
    proxied(proxy::geocode(&d, org.org.id, &q.q, q.limit.clamp(1, 20)).await)
}

/// `GET /api/v1/geo/reverse?lat=&lon=` — every member.
async fn reverse(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Query(q): Query<ReverseQuery>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    let d = Deps {
        db: &ctx.db,
        config: &ctx.config,
        ring: &ring,
    };
    proxied(proxy::reverse(&d, org.org.id, q.lat, q.lon).await)
}

/// `POST /api/v1/geo/route` — every member (read-only computation).
async fn route(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Json(body): Json<RouteBody>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    let d = Deps {
        db: &ctx.db,
        config: &ctx.config,
        ring: &ring,
    };
    let points: Vec<(f64, f64)> = body.points.iter().map(|p| (p[0], p[1])).collect();
    proxied(proxy::route(&d, org.org.id, &points, body.profile).await)
}
