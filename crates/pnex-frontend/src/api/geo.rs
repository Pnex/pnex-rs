//! Geo providers of the org and the server-side geo proxy
//! (`/api/v1/geo/*`, geo-layers.md §8): basemaps of the map, provider CRUD
//! (secret = vault reference), forward / reverse geocoding.

use pnex_core::geo::{Basemap, GeoProvider, GeoProviderInput, GeocodeResult};
use uuid::Uuid;

use crate::api::client;
use crate::api::error::ApiError;
use crate::api::media::urlencode;

const PROVIDERS: &str = "/api/v1/geo/providers";

/// `GET …/basemaps` — basemaps the map can open on.
pub async fn basemaps() -> Result<Vec<Basemap>, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/geo/basemaps", None).await
}

/// `GET …/providers`.
pub async fn providers() -> Result<Vec<GeoProvider>, ApiError> {
    client::request(reqwest::Method::GET, PROVIDERS, None).await
}

/// `POST …/providers`.
pub async fn create_provider(input: &GeoProviderInput) -> Result<GeoProvider, ApiError> {
    let body = serde_json::to_value(input).map_err(|e| ApiError::new(e.to_string()))?;
    client::request(reqwest::Method::POST, PROVIDERS, Some(body)).await
}

/// `PUT …/providers/{id}` — `secret: None` keeps the current secret.
pub async fn update_provider(id: Uuid, input: &GeoProviderInput) -> Result<GeoProvider, ApiError> {
    let body = serde_json::to_value(input).map_err(|e| ApiError::new(e.to_string()))?;
    client::request(
        reqwest::Method::PUT,
        &format!("{PROVIDERS}/{id}"),
        Some(body),
    )
    .await
}

/// `DELETE …/providers/{id}` — 204.
pub async fn delete_provider(id: Uuid) -> Result<(), ApiError> {
    client::request_opt::<()>(reqwest::Method::DELETE, &format!("{PROVIDERS}/{id}"), None).await?;
    Ok(())
}

/// `POST …/providers/{id}/test` — one real call; latency in ms.
pub async fn test_provider(id: Uuid) -> Result<u64, ApiError> {
    let v: serde_json::Value = client::request(
        reqwest::Method::POST,
        &format!("{PROVIDERS}/{id}/test"),
        None,
    )
    .await?;
    Ok(v["latency_ms"].as_u64().unwrap_or_default())
}

/// `GET …/geocode?q=` through the org's default geocoder.
pub async fn geocode(query: &str) -> Result<Vec<GeocodeResult>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/geo/geocode?q={}", urlencode(query)),
        None,
    )
    .await
}

/// `GET …/reverse?lat=&lon=` through the org's default reverse geocoder.
pub async fn reverse(lat: f64, lon: f64) -> Result<Vec<GeocodeResult>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/geo/reverse?lat={lat}&lon={lon}"),
        None,
    )
    .await
}
