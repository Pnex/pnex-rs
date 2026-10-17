//! Geo providers and proxy — `/api/v1/geo/*` (geo-layers.md §8, phase F).
//! Providers: CRUD owner/admin, list every member (secret = vault
//! reference). Proxy: every member, the browser never calls a provider
//! (L20). Org always from the principal (R1).

use crate::controllers::pagination::{self, PageParams};
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
        .add("/route", get(route))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(list, create, update, delete, test, basemaps, geocode, reverse, route),
        components(schemas(pnex_core::geo::RouteProfile))
    )]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
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
        ProviderError::Invalid { field, token } => Ok(super::field_error(&field, &token)),
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
        ProxyError::BadRequest(field) => Ok(super::field_error(field, err_codes::FIELD_INVALID)),
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
#[utoipa::path(
    get,
    path = "/api/v1/geo/providers",
    tag = "geo",
    operation_id = "geo_providers_list",
    summary = "List the geo providers of the organisation",
    description = "Every geo provider (basemap, geocoding, routing) configured for the org, with the capabilities each is the org default for. Paginated (`limit`/`offset`). The API key is only a vault reference, never its value. Any member.",
    params(PageQuery),
    responses((status = 200, body = pnex_core::Paginated<pnex_core::geo::GeoProvider>)),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let rows = providers::list(&ctx.db, org.org.id)
        .await
        .map_err(db_error)?;
    match providers::views(&ctx.db, org.org.id, &rows).await {
        Ok(views) => paged("/api/v1/geo/providers", &q, views),
        Err(e) => crate::controllers::secrets::store_error(e),
    }
}

/// Page of a list (`limit`/`offset`, D14).
#[derive(Deserialize, utoipa::IntoParams)]
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

/// `POST /api/v1/geo/providers` — owner/admin.
#[utoipa::path(
    post,
    path = "/api/v1/geo/providers",
    tag = "geo",
    operation_id = "geo_providers_create",
    summary = "Create a geo provider",
    description = "Registers a basemap, geocoding or routing provider. A typed `api_key` value is stored in the org vault (owner/admin); `default_for` makes it the org default for those capabilities. Owner or admin only.",
    request_body = pnex_core::geo::GeoProviderInput,
    responses(
        (status = 201, body = pnex_core::geo::GeoProvider),
        (status = 400, description = "`field-invalid`: `errors.fields` maps the field to a token", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`geo-provider-forbidden` or a vault write refusal", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`geo-provider-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    put,
    path = "/api/v1/geo/providers/{id}",
    tag = "geo",
    operation_id = "geo_providers_update",
    summary = "Replace a geo provider",
    description = "Full replacement of the provider settings; an absent `api_key` keeps the current key. Owner or admin only.",
    params(("id" = String, Path, format = "uuid", description = "Geo provider id")),
    request_body = pnex_core::geo::GeoProviderInput,
    responses(
        (status = 200, body = pnex_core::geo::GeoProvider),
        (status = 400, description = "`field-invalid`: `errors.fields` maps the field to a token", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`geo-provider-forbidden` or a vault write refusal", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`geo-provider-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`geo-provider-name-taken`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    delete,
    path = "/api/v1/geo/providers/{id}",
    tag = "geo",
    operation_id = "geo_providers_delete",
    summary = "Delete a geo provider",
    description = "Deletes the provider (and its default assignments). Owner or admin only.",
    params(("id" = String, Path, format = "uuid", description = "Geo provider id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`geo-provider-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`geo-provider-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
#[utoipa::path(
    post,
    path = "/api/v1/geo/providers/{id}/test",
    tag = "geo",
    operation_id = "geo_providers_test",
    summary = "Test a geo provider",
    description = "Sends one probe request to the provider from the server and returns its latency. No data is stored. Owner or admin only.",
    params(("id" = String, Path, format = "uuid", description = "Geo provider id")),
    responses(
        (status = 200, body = GeoTestResult),
        (status = 403, description = "`geo-provider-forbidden`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`geo-provider-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`geo-not-configured`: no default provider for this capability (args.capability)", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`geo-rate-limited`: provider rate limit reached; `Retry-After` header and args.retry_after_s", body = crate::openapi::ErrorDetail),
        (status = 502, description = "`geo-provider-failed`: the provider failed or answered an error", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
        Ok(ms) => format::json(GeoTestResult {
            ok: true,
            latency_ms: ms,
        }),
        Err(e) => proxy_error(e),
    }
}

/// Answer of a successful provider test.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct GeoTestResult {
    ok: bool,
    latency_ms: u64,
}

/// `GET /api/v1/geo/basemaps` — every member; style URLs carry the
/// basemap's browser key.
#[utoipa::path(
    get,
    path = "/api/v1/geo/basemaps",
    tag = "geo",
    operation_id = "geo_basemaps_list",
    summary = "List the basemaps the map can open on",
    description = "MapLibre style URLs of the org basemap providers, paginated (`limit`/`offset`); a basemap key is a browser key already embedded in `style_url`. Any member.",
    params(PageQuery),
    responses((status = 200, body = pnex_core::Paginated<pnex_core::geo::Basemap>)),
    security(("bearer" = [], "org" = []))
)]
async fn basemaps(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Query(q): Query<PageQuery>,
) -> Result<Response> {
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    match providers::basemaps(&ctx.db, &ring, org.org.id).await {
        Ok(maps) => paged("/api/v1/geo/basemaps", &q, maps),
        Err(e) => crate::controllers::secrets::store_error(e),
    }
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct GeocodeQuery {
    /// Free-text address or place (1–500 characters).
    q: String,
    /// Maximum hits (default 5, clamped to 1–20).
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    5
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ReverseQuery {
    lat: f64,
    lon: f64,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct RouteQuery {
    /// `lat,lon` pairs separated by `;`, 2 to 25 points.
    points: String,
    #[serde(default)]
    #[param(value_type = Option<RouteProfile>)]
    profile: RouteProfile,
}

/// Parses `lat,lon;lat,lon…`; `None` on any malformed pair.
fn parse_points(raw: &str) -> Option<Vec<(f64, f64)>> {
    raw.split(';')
        .map(|pair| {
            let (lat, lon) = pair.split_once(',')?;
            Some((lat.trim().parse().ok()?, lon.trim().parse().ok()?))
        })
        .collect()
}

/// Hits of a geocoding call (computed result, not a stored collection).
#[derive(serde::Serialize, utoipa::ToSchema)]
struct GeocodeHits {
    results: Vec<pnex_core::geo::GeocodeResult>,
}

fn proxied<T: serde::Serialize>(r: std::result::Result<T, ProxyError>) -> Result<Response> {
    match r {
        Ok(v) => format::json(v),
        Err(e) => proxy_error(e),
    }
}

fn proxied_hits(
    r: std::result::Result<Vec<pnex_core::geo::GeocodeResult>, ProxyError>,
) -> Result<Response> {
    proxied(r.map(|results| GeocodeHits { results }))
}

/// `GET /api/v1/geo/geocode?q=&limit=` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/geo/geocode",
    tag = "geo",
    operation_id = "geo_geocode",
    summary = "Geocode an address",
    description = "Forward geocoding through the org default geocoding provider (server-side proxy, cached and rate limited). Returns normalized hits in `results`. Any member.",
    params(GeocodeQuery),
    responses(
        (status = 200, body = GeocodeHits),
        (status = 400, description = "`field-invalid`: empty or too long `q`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`geo-not-configured`: no default provider for this capability (args.capability)", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`geo-rate-limited`: provider rate limit reached; `Retry-After` header and args.retry_after_s", body = crate::openapi::ErrorDetail),
        (status = 502, description = "`geo-provider-failed`: the provider failed or answered an error", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
    proxied_hits(proxy::geocode(&d, org.org.id, &q.q, q.limit.clamp(1, 20)).await)
}

/// `GET /api/v1/geo/reverse?lat=&lon=` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/geo/reverse",
    tag = "geo",
    operation_id = "geo_reverse",
    summary = "Reverse geocode a point",
    description = "Address of a WGS84 point through the org default reverse-geocoding provider (server-side proxy). Empty `results` means no address. Any member.",
    params(ReverseQuery),
    responses(
        (status = 200, body = GeocodeHits),
        (status = 400, description = "`field-invalid`: coordinates out of range", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`geo-not-configured`: no default provider for this capability (args.capability)", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`geo-rate-limited`: provider rate limit reached; `Retry-After` header and args.retry_after_s", body = crate::openapi::ErrorDetail),
        (status = 502, description = "`geo-provider-failed`: the provider failed or answered an error", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
    proxied_hits(proxy::reverse(&d, org.org.id, q.lat, q.lon).await)
}

/// `GET /api/v1/geo/route?points=lat,lon;lat,lon&profile=` — every member
/// (read-only computation).
#[utoipa::path(
    get,
    path = "/api/v1/geo/route",
    tag = "geo",
    operation_id = "geo_route",
    summary = "Compute a route",
    description = "Route through 2 to 25 points (`points=lat,lon;lat,lon…`) with the org default routing provider (server-side proxy). Read-only computation, nothing stored. Any member.",
    params(RouteQuery),
    responses(
        (status = 200, body = pnex_core::geo::Route),
        (status = 400, description = "`field-invalid`: malformed or out-of-range `points`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`geo-not-configured`: no default provider for this capability (args.capability)", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`geo-rate-limited`: provider rate limit reached; `Retry-After` header and args.retry_after_s", body = crate::openapi::ErrorDetail),
        (status = 502, description = "`geo-provider-failed`: the provider failed or answered an error", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn route(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Query(q): Query<RouteQuery>,
) -> Result<Response> {
    let Some(points) = parse_points(&q.points) else {
        return Ok(super::field_error("points", err_codes::FIELD_INVALID));
    };
    let ring = match crate::controllers::secrets::keyring(&ctx) {
        Ok(ring) => ring,
        Err(e) => return crate::controllers::secrets::store_error(e),
    };
    let d = Deps {
        db: &ctx.db,
        config: &ctx.config,
        ring: &ring,
    };
    proxied(proxy::route(&d, org.org.id, &points, q.profile).await)
}

#[cfg(test)]
mod tests {
    use super::parse_points;

    #[test]
    fn route_points_parse_pairs_and_reject_garbage() {
        assert_eq!(
            parse_points("45.75,4.83; 45.76,4.86"),
            Some(vec![(45.75, 4.83), (45.76, 4.86)])
        );
        assert_eq!(parse_points("45.75"), None);
        assert_eq!(parse_points("a,b;1,2"), None);
    }
}
