//! Applications of the org (external-apps.md D433): `/api/v1/apps`.

use std::collections::BTreeMap;

use pnex_core::apps::{AppBuildInput, AppInput, AppPublishInput, AppScreenView, AppView};
use pnex_core::ontology::api::QueryResult;
use serde_json::Value;

use crate::api::client;
use crate::api::error::ApiError;
use crate::api::media::urlencode;

/// `GET /api/v1/apps`.
pub async fn list() -> Result<Vec<AppView>, ApiError> {
    client::request_all("/api/v1/apps").await
}

/// `POST /api/v1/apps`, or `PUT /api/v1/apps/{id}` on its version.
pub async fn save(id: Option<&str>, input: &AppInput) -> Result<AppView, ApiError> {
    let body = serde_json::to_value(input).ok();
    match id {
        None => client::request(reqwest::Method::POST, "/api/v1/apps", body).await,
        Some(id) => {
            client::request(reqwest::Method::PUT, &format!("/api/v1/apps/{id}"), body).await
        }
    }
}

/// `DELETE /api/v1/apps/{id}` — its service account stays.
pub async fn delete(id: &str) -> Result<(), ApiError> {
    client::request_opt::<Value>(reqwest::Method::DELETE, &format!("/api/v1/apps/{id}"), None)
        .await
        .map(|_| ())
}

/// `GET /api/v1/apps/{id}`.
pub async fn get(id: &str) -> Result<AppView, ApiError> {
    client::request(reqwest::Method::GET, &format!("/api/v1/apps/{id}"), None).await
}

/// `PUT /api/v1/apps/{id}/build`: its screens, variables and object sets.
pub async fn build(id: &str, input: &AppBuildInput) -> Result<AppView, ApiError> {
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/apps/{id}/build"),
        serde_json::to_value(input).ok(),
    )
    .await
}

/// `POST /api/v1/apps/{id}/publish`.
pub async fn publish(id: &str, expected_version: i32) -> Result<AppView, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/apps/{id}/publish"),
        serde_json::to_value(AppPublishInput { expected_version }).ok(),
    )
    .await
}

/// `GET /api/v1/apps/{id}/screens/{key}`: the published version.
pub async fn screen(id: &str, key: &str) -> Result<AppScreenView, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/apps/{id}/screens/{}", urlencode(key)),
        None,
    )
    .await
}

/// `POST /api/v1/apps/{id}/object-sets/{name}/aggregate/query`, reader's rights.
pub async fn aggregate_set(
    id: &str,
    name: &str,
    req: &pnex_core::ontology::aggregate::SetAggregateQuery,
) -> Result<pnex_core::ontology::aggregate::AggregateResult, ApiError> {
    let url = format!(
        "/api/v1/apps/{id}/object-sets/{}/aggregate/query",
        urlencode(name)
    );
    client::request(reqwest::Method::POST, &url, serde_json::to_value(req).ok()).await
}

/// `GET /api/v1/apps/{id}/object-sets/{name}` with the reader's rights.
pub async fn run_set(
    id: &str,
    name: &str,
    params: &BTreeMap<String, String>,
    limit: u64,
    offset: u64,
    sort: Option<(String, bool)>,
) -> Result<QueryResult, ApiError> {
    let mut url = format!(
        "/api/v1/apps/{id}/object-sets/{}?limit={limit}&offset={offset}",
        urlencode(name)
    );
    for (k, v) in params {
        url.push_str(&format!("&{}={}", urlencode(k), urlencode(v)));
    }
    if let Some((key, descending)) = sort {
        url.push_str(&format!("&sort={}&desc={descending}", urlencode(&key)));
    }
    client::request(reqwest::Method::GET, &url, None).await
}
