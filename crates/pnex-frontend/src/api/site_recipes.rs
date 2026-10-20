//! Site navigation rules API (site-recipes.md D323–D326): web sites,
//! recipes, test runs on the server browser and the run journal.

use pnex_core::site_recipe::{
    Recipe, RunInput, SiteRecipe, SiteRecipeDraft, SiteRecipeInput, SiteRecipeRun, WebSite,
};
use pnex_core::Paginated;

use crate::api::client;
use crate::api::error::ApiError;
use crate::api::media::urlencode;

/// `GET /api/v1/web-sites`, every page.
pub async fn sites() -> Result<Vec<WebSite>, ApiError> {
    client::request_all("/api/v1/web-sites").await
}

/// `GET /api/v1/site-recipes?site=…`, every page.
pub async fn list(site: &str) -> Result<Vec<SiteRecipe>, ApiError> {
    client::request_all(&format!("/api/v1/site-recipes?site={}", urlencode(site))).await
}

/// `GET /api/v1/site-recipes`, every recipe of the org.
pub async fn all() -> Result<Vec<SiteRecipe>, ApiError> {
    client::request_all("/api/v1/site-recipes").await
}

/// `POST /api/v1/site-recipes/validate` — 400 = `site-recipe-invalid`.
pub async fn validate(yaml: &str) -> Result<Recipe, ApiError> {
    let body = SiteRecipeDraft {
        yaml: yaml.to_string(),
    };
    client::request(
        reqwest::Method::POST,
        "/api/v1/site-recipes/validate",
        Some(serde_json::to_value(body).unwrap_or_default()),
    )
    .await
}

/// `POST /api/v1/site-recipes`.
pub async fn create(yaml: &str) -> Result<SiteRecipe, ApiError> {
    let body = SiteRecipeInput {
        yaml: yaml.to_string(),
        expected_version: None,
    };
    client::request(
        reqwest::Method::POST,
        "/api/v1/site-recipes",
        Some(serde_json::to_value(body).unwrap_or_default()),
    )
    .await
}

/// `PUT /api/v1/site-recipes/{id}` — 409 when saved in between.
pub async fn update(id: &str, yaml: &str, expected_version: i32) -> Result<SiteRecipe, ApiError> {
    let body = SiteRecipeInput {
        yaml: yaml.to_string(),
        expected_version: Some(expected_version),
    };
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/site-recipes/{}", urlencode(id)),
        Some(serde_json::to_value(body).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/site-recipes/{id}`.
pub async fn delete(id: &str) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/site-recipes/{}", urlencode(id)),
        None,
    )
    .await
    .map(|_| ())
}

/// `POST /api/v1/site-recipes/runs` — answers the run, `running`.
pub async fn play(input: &RunInput) -> Result<SiteRecipeRun, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/site-recipes/runs",
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/site-recipes/runs?site=…` — journal of a site.
pub async fn runs(site: &str, limit: i64) -> Result<Paginated<SiteRecipeRun>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!(
            "/api/v1/site-recipes/runs?site={}&limit={limit}",
            urlencode(site)
        ),
        None,
    )
    .await
}

/// `GET /api/v1/site-recipes/runs/{id}` — steps, extracts, changes.
pub async fn run(id: &str) -> Result<SiteRecipeRun, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/site-recipes/runs/{}", urlencode(id)),
        None,
    )
    .await
}

/// Path of a run file (`step-3.jpg`, `pdf-5.pdf`).
pub fn file_path(run_id: &str, file: &str) -> String {
    format!(
        "/api/v1/site-recipes/runs/{}/files/{}",
        urlencode(run_id),
        urlencode(file)
    )
}

// ───────────────────────────── browser endpoints ─────────────────────────────

use pnex_core::site_recipe::{BrowserEndpoint, BrowserEndpointInput, BrowserTestResult};

/// `GET /api/v1/browser-endpoints`, every page.
pub async fn endpoints() -> Result<Vec<BrowserEndpoint>, ApiError> {
    client::request_all("/api/v1/browser-endpoints").await
}

/// `POST /api/v1/browser-endpoints` or `PUT …/{id}`.
pub async fn endpoint_save(
    id: Option<&str>,
    input: &BrowserEndpointInput,
) -> Result<BrowserEndpoint, ApiError> {
    let body = Some(serde_json::to_value(input).unwrap_or_default());
    match id {
        Some(id) => {
            client::request(
                reqwest::Method::PUT,
                &format!("/api/v1/browser-endpoints/{}", urlencode(id)),
                body,
            )
            .await
        }
        None => client::request(reqwest::Method::POST, "/api/v1/browser-endpoints", body).await,
    }
}

/// `DELETE /api/v1/browser-endpoints/{id}`.
pub async fn endpoint_delete(id: &str) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/browser-endpoints/{}", urlencode(id)),
        None,
    )
    .await
    .map(|_| ())
}

/// `POST /api/v1/browser-endpoints/{id}/test`.
pub async fn endpoint_test(id: &str) -> Result<BrowserTestResult, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/browser-endpoints/{}/test", urlencode(id)),
        None,
    )
    .await
}

/// `GET /api/v1/site-recipes/{id}/versions`, every page (newest first).
pub async fn versions(
    id: &str,
) -> Result<Vec<pnex_core::site_recipe::SiteRecipeVersion>, ApiError> {
    client::request_all(&format!("/api/v1/site-recipes/{}/versions", urlencode(id))).await
}

/// `POST /api/v1/site-recipes/{id}/versions/{version}/restore`.
pub async fn restore(id: &str, version: i32) -> Result<SiteRecipe, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!(
            "/api/v1/site-recipes/{}/versions/{version}/restore",
            urlencode(id)
        ),
        None,
    )
    .await
}

/// `GET /api/v1/site-recipes/{id}/schedule`.
pub async fn schedule(id: &str) -> Result<pnex_core::site_recipe::SiteRecipeSchedule, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/site-recipes/{}/schedule", urlencode(id)),
        None,
    )
    .await
}
