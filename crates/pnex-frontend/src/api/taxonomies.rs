//! Topic taxonomies API (media-ingest.md D168): `/api/v1/taxonomies`.

use pnex_core::taxonomy::{Taxonomy, TaxonomyInput, TaxonomyVersion, TaxonomyVersionInput};

use crate::api::client;
use crate::api::error::ApiError;

fn body<T: serde::Serialize>(input: &T) -> Option<serde_json::Value> {
    Some(serde_json::to_value(input).unwrap_or_default())
}

/// `GET /api/v1/taxonomies` — with their current version.
pub async fn list() -> Result<Vec<Taxonomy>, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/taxonomies", None).await
}

/// `POST /api/v1/taxonomies` — created empty (version 0).
pub async fn create(input: &TaxonomyInput) -> Result<Taxonomy, ApiError> {
    client::request(reqwest::Method::POST, "/api/v1/taxonomies", body(input)).await
}

/// `DELETE /api/v1/taxonomies/{id}`.
pub async fn delete(id: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/taxonomies/{id}"),
        None,
    )
    .await
}

/// `GET /api/v1/taxonomies/{id}/versions` — newest first.
pub async fn versions(id: &str) -> Result<Vec<TaxonomyVersion>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/taxonomies/{id}/versions"),
        None,
    )
    .await
}

/// `POST /api/v1/taxonomies/{id}/versions` — 409 `taxonomy-version-conflict`
/// when `expected_version` is stale.
pub async fn add_version(
    id: &str,
    input: &TaxonomyVersionInput,
) -> Result<TaxonomyVersion, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/taxonomies/{id}/versions"),
        body(input),
    )
    .await
}
