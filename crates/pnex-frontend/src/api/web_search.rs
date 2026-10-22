//! Web search providers of the org (web-presence.md D360).

use pnex_core::web_search::{WebSearchProvider, WebSearchProviderInput, WebSearchTestResult};

use crate::api::client;
use crate::api::error::ApiError;
use crate::api::media::urlencode;

/// `GET /api/v1/web-search/providers`, every page.
pub async fn providers() -> Result<Vec<WebSearchProvider>, ApiError> {
    client::request_all("/api/v1/web-search/providers").await
}

/// `POST /api/v1/web-search/providers` or `PUT …/{id}`.
pub async fn save(
    id: Option<&str>,
    input: &WebSearchProviderInput,
) -> Result<WebSearchProvider, ApiError> {
    let body = Some(serde_json::to_value(input).unwrap_or_default());
    match id {
        Some(id) => {
            client::request(
                reqwest::Method::PUT,
                &format!("/api/v1/web-search/providers/{}", urlencode(id)),
                body,
            )
            .await
        }
        None => client::request(reqwest::Method::POST, "/api/v1/web-search/providers", body).await,
    }
}

/// `DELETE /api/v1/web-search/providers/{id}`.
pub async fn delete(id: &str) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/web-search/providers/{}", urlencode(id)),
        None,
    )
    .await
    .map(|_| ())
}

/// `POST /api/v1/web-search/providers/{id}/test`.
pub async fn test(id: &str) -> Result<WebSearchTestResult, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/web-search/providers/{}/test", urlencode(id)),
        None,
    )
    .await
}
