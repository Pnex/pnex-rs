//! Server-side proxy to the org's geo engines (geo-layers.md L20–L22): the
//! browser, flows and the assistant never call a provider. Secrets are
//! revealed in memory here only; responses are cached in Valkey and every
//! provider has its own rate limit (Nominatim public policy: 1 req/s).

use std::time::Duration;

use loco_rs::config::Config;
use pnex_core::geo::{self, GeoCapability, GeoProviderKind, GeocodeResult, Route};
use redis::AsyncCommands;
use sea_orm::ConnectionTrait;
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};

use super::providers;
use crate::models::_entities::geo_providers;
use crate::services::rate_limit::{Decision, RateLimiter};
use crate::services::secrets::store::{self, StoreError};
use crate::services::secrets::Keyring;

/// Cache TTL of geocoding answers a provider lets us keep (L21).
const GEOCODE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
/// Cache TTL of routes, and of anything when `store_allowed` is false (L22).
const SHORT_TTL: Duration = Duration::from_secs(10 * 60);
/// Default rate when the provider row sets none.
const DEFAULT_RATE_PER_S: f32 = 1.0;

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("no default geo provider for {0:?}")]
    NotConfigured(GeoCapability),
    #[error("geo provider rate limited")]
    RateLimited(Duration),
    #[error("geo provider failed: {0}")]
    Upstream(String),
    #[error("invalid request: {0}")]
    BadRequest(&'static str),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<sea_orm::DbErr> for ProxyError {
    fn from(e: sea_orm::DbErr) -> Self {
        Self::Store(StoreError::Db(e))
    }
}

impl From<geo::GeoError> for ProxyError {
    fn from(e: geo::GeoError) -> Self {
        match e {
            geo::GeoError::BadBaseUrl => Self::Upstream("invalid provider URL".into()),
            geo::GeoError::Provider(m) => Self::Upstream(m),
            geo::GeoError::Malformed("route needs two points") => {
                Self::BadRequest("route needs two points")
            }
            geo::GeoError::Malformed(what) => {
                Self::Upstream(format!("unexpected response: {what}"))
            }
        }
    }
}

/// Process-wide HTTP client: egress guard (R8), no redirect (an org URL
/// could bounce the platform onto an internal host).
fn http() -> &'static reqwest::Client {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP.get_or_init(|| {
        pnex_core::egress::guarded(reqwest::Client::builder())
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("PNEX/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client")
    })
}

/// Shared deps of a proxied call.
pub struct Deps<'a, C: ConnectionTrait> {
    pub db: &'a C,
    pub config: &'a Config,
    pub ring: &'a Keyring,
}

async fn provider_for<C: ConnectionTrait>(
    d: &Deps<'_, C>,
    org_id: i64,
    cap: GeoCapability,
) -> Result<(geo_providers::Model, GeoProviderKind), ProxyError> {
    let row = providers::default_for(d.db, org_id, cap)
        .await?
        .ok_or(ProxyError::NotConfigured(cap))?;
    match GeoProviderKind::parse(&row.kind) {
        Some(kind) if kind.supported().contains(&cap) => Ok((row, kind)),
        _ => Err(ProxyError::NotConfigured(cap)),
    }
}

/// Cache key: provider + its config version + normalized request (L21).
fn cache_key(row: &geo_providers::Model, cap: GeoCapability, request: &str) -> String {
    let digest = Sha256::digest(request.as_bytes());
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "pnex:geo:{}:{}:{}:{hex}",
        row.id,
        row.updated_at.timestamp_millis(),
        cap.as_str()
    )
}

async fn cache_get<T: DeserializeOwned>(config: &Config, key: &str) -> Option<T> {
    let mut conn = crate::services::shared_valkey::conn(config).await?;
    let raw: Option<String> = conn.get(key).await.ok()?;
    serde_json::from_str(&raw?).ok()
}

async fn cache_set<T: Serialize>(config: &Config, key: &str, value: &T, ttl: Duration) {
    let Some(mut conn) = crate::services::shared_valkey::conn(config).await else {
        return;
    };
    if let Ok(raw) = serde_json::to_string(value) {
        let _: Result<(), _> = conn.set_ex(key, raw, ttl.as_secs()).await;
    }
}

async fn throttle(config: &Config, row: &geo_providers::Model) -> Result<(), ProxyError> {
    static LIMITER: tokio::sync::OnceCell<RateLimiter> = tokio::sync::OnceCell::const_new();
    let limiter = LIMITER
        .get_or_init(|| async {
            RateLimiter::new(crate::services::shared_valkey::conn(config).await)
        })
        .await;
    let rate = row.rate_limit_per_s.unwrap_or(DEFAULT_RATE_PER_S);
    // Window sized so that sub-1/s rates still allow one call per window.
    let (limit, window) = if rate >= 1.0 {
        (rate.floor() as u32, Duration::from_secs(1))
    } else {
        (1, Duration::from_secs_f32(1.0 / rate))
    };
    match limiter.hit(&format!("geo:{}", row.id), limit, window).await {
        Decision::Allow => Ok(()),
        Decision::Deny { retry_after } => Err(ProxyError::RateLimited(retry_after)),
    }
}

/// Sends the request with the API key (revealed from the vault) added as
/// the provider's key parameter, and the provider's timeout.
async fn send<C: ConnectionTrait>(
    d: &Deps<'_, C>,
    row: &geo_providers::Model,
    mut url: reqwest::Url,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, ProxyError> {
    if let Some(id) = row.secret_id {
        let key = store::reveal(d.db, d.ring, Some(row.org_id), id).await?;
        url.query_pairs_mut().append_pair(&row.key_param, &key);
    }
    let req = match body {
        Some(b) => http().post(url).json(&b),
        None => http().get(url),
    }
    .timeout(Duration::from_millis(row.timeout_ms.max(500) as u64));
    let resp = req.send().await.map_err(|e| {
        // Never echo the URL: the key is in its query string.
        let e = e.without_url();
        ProxyError::Upstream(if e.is_timeout() {
            "timeout".into()
        } else {
            e.to_string()
        })
    })?;
    let status = resp.status();
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|_| ProxyError::Upstream(format!("HTTP {}", status.as_u16())))?;
    // Valhalla answers its own errors as JSON with a 4xx: let the parser
    // surface the message.
    if status.is_server_error() {
        return Err(ProxyError::Upstream(format!("HTTP {}", status.as_u16())));
    }
    Ok(json)
}

fn params_of(row: &geo_providers::Model) -> Vec<(String, String)> {
    providers::strings_of(&row.params)
        .into_iter()
        .filter(|(k, _)| k != providers::STYLE_URL_DARK)
        .collect()
}

fn geocode_ttl(row: &geo_providers::Model) -> Duration {
    if row.store_allowed {
        GEOCODE_TTL
    } else {
        SHORT_TTL
    }
}

/// Forward geocoding through the org's default `geocode` provider.
pub async fn geocode<C: ConnectionTrait>(
    d: &Deps<'_, C>,
    org_id: i64,
    query: &str,
    limit: u32,
) -> Result<Vec<GeocodeResult>, ProxyError> {
    let query = query.trim();
    if query.is_empty() || query.chars().count() > 500 {
        return Err(ProxyError::BadRequest("query"));
    }
    let cap = GeoCapability::Geocode;
    let (row, kind) = provider_for(d, org_id, cap).await?;
    let key = cache_key(&row, cap, &format!("{}|{limit}", query.to_lowercase()));
    if let Some(hit) = cache_get(d.config, &key).await {
        return Ok(hit);
    }
    throttle(d.config, &row).await?;
    let hits = match kind {
        GeoProviderKind::Nominatim => {
            let url = geo::nominatim_search_url(&row.base_url, query, limit, &params_of(&row))?;
            geo::parse_nominatim(&send(d, &row, url, None).await?)?
        }
        GeoProviderKind::Photon => {
            let url = geo::photon_search_url(&row.base_url, query, limit, &params_of(&row))?;
            geo::parse_photon(&send(d, &row, url, None).await?)?
        }
        _ => return Err(ProxyError::NotConfigured(cap)),
    };
    cache_set(d.config, &key, &hits, geocode_ttl(&row)).await;
    Ok(hits)
}

/// Reverse geocoding through the org's default `reverse` provider.
pub async fn reverse<C: ConnectionTrait>(
    d: &Deps<'_, C>,
    org_id: i64,
    lat: f64,
    lon: f64,
) -> Result<Vec<GeocodeResult>, ProxyError> {
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return Err(ProxyError::BadRequest("coordinates"));
    }
    let cap = GeoCapability::Reverse;
    let (row, kind) = provider_for(d, org_id, cap).await?;
    // ~1 m grid: neighbouring clicks share a cache entry.
    let key = cache_key(&row, cap, &format!("{lat:.5},{lon:.5}"));
    if let Some(hit) = cache_get(d.config, &key).await {
        return Ok(hit);
    }
    throttle(d.config, &row).await?;
    let hits = match kind {
        GeoProviderKind::Nominatim => {
            let url = geo::nominatim_reverse_url(&row.base_url, lat, lon, &params_of(&row))?;
            geo::parse_nominatim(&send(d, &row, url, None).await?)?
        }
        GeoProviderKind::Photon => {
            let url = geo::photon_reverse_url(&row.base_url, lat, lon, &params_of(&row))?;
            geo::parse_photon(&send(d, &row, url, None).await?)?
        }
        _ => return Err(ProxyError::NotConfigured(cap)),
    };
    cache_set(d.config, &key, &hits, geocode_ttl(&row)).await;
    Ok(hits)
}

/// Route between `points` (`(lat, lon)`) through the default `route`
/// provider.
pub async fn route<C: ConnectionTrait>(
    d: &Deps<'_, C>,
    org_id: i64,
    points: &[(f64, f64)],
    profile: geo::RouteProfile,
) -> Result<Route, ProxyError> {
    if points.len() > 25
        || points
            .iter()
            .any(|(lat, lon)| !(-90.0..=90.0).contains(lat) || !(-180.0..=180.0).contains(lon))
    {
        return Err(ProxyError::BadRequest("points"));
    }
    let cap = GeoCapability::Route;
    let (row, kind) = provider_for(d, org_id, cap).await?;
    let key = cache_key(&row, cap, &format!("{points:?}|{profile:?}"));
    if let Some(hit) = cache_get(d.config, &key).await {
        return Ok(hit);
    }
    throttle(d.config, &row).await?;
    let route = match kind {
        GeoProviderKind::Valhalla => {
            let params = providers::strings_of(&row.params);
            let lang = params.get("language").map(String::as_str);
            let (url, body) = geo::valhalla_route_request(&row.base_url, points, profile, lang)?;
            geo::parse_valhalla_route(&send(d, &row, url, Some(body)).await?)?
        }
        GeoProviderKind::GraphHopper => {
            let params = providers::strings_of(&row.params);
            let lang = params.get("language").map(String::as_str);
            let url = geo::graphhopper_route_url(&row.base_url, points, profile, lang)?;
            geo::parse_graphhopper_route(&send(d, &row, url, None).await?)?
        }
        _ => return Err(ProxyError::NotConfigured(cap)),
    };
    cache_set(d.config, &key, &route, SHORT_TTL).await;
    Ok(route)
}

/// "Test" button (L23): one real call with a known input, never cached.
/// Returns the latency in ms.
pub async fn test<C: ConnectionTrait>(
    d: &Deps<'_, C>,
    row: &geo_providers::Model,
) -> Result<u64, ProxyError> {
    let started = std::time::Instant::now();
    let kind = GeoProviderKind::parse(&row.kind).ok_or(ProxyError::BadRequest("kind"))?;
    match kind {
        GeoProviderKind::Basemap => {
            let url = reqwest::Url::parse(&row.base_url)
                .map_err(|_| ProxyError::Upstream("invalid provider URL".into()))?;
            let style = send(d, row, url, None).await?;
            if style["version"].as_u64() != Some(8) {
                return Err(ProxyError::Upstream("not a MapLibre style v8".into()));
            }
        }
        GeoProviderKind::Nominatim => {
            let url = geo::nominatim_reverse_url(&row.base_url, 48.8584, 2.2945, &params_of(row))?;
            geo::parse_nominatim(&send(d, row, url, None).await?)?;
        }
        GeoProviderKind::Photon => {
            let url = geo::photon_reverse_url(&row.base_url, 48.8584, 2.2945, &params_of(row))?;
            geo::parse_photon(&send(d, row, url, None).await?)?;
        }
        GeoProviderKind::GraphHopper => {
            let points = [(48.8584, 2.2945), (48.8606, 2.3376)];
            let url =
                geo::graphhopper_route_url(&row.base_url, &points, geo::RouteProfile::Auto, None)?;
            geo::parse_graphhopper_route(&send(d, row, url, None).await?)?;
        }
        GeoProviderKind::Valhalla => {
            let points = [(48.8584, 2.2945), (48.8606, 2.3376)];
            let (url, body) =
                geo::valhalla_route_request(&row.base_url, &points, geo::RouteProfile::Auto, None)?;
            geo::parse_valhalla_route(&send(d, row, url, Some(body)).await?)?;
        }
    }
    Ok(started.elapsed().as_millis() as u64)
}
