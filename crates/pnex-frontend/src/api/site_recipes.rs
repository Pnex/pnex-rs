//! Site navigation rules (agents.md D323–D326): web sites, rules (org
//! pipelines that navigate, PE5.5), plays on the server browser and the
//! navigation journal.

use pnex_core::pipelines::Pipeline;
use pnex_core::site_recipe::{site_host, Recipe, RunInput, SiteRecipe, SiteRecipeRun, WebSite};
use pnex_core::Paginated;

use crate::api::client;
use crate::api::error::ApiError;
use crate::api::media::urlencode;
use crate::api::pipelines;

/// `GET /api/v1/web-sites`, every page.
pub async fn sites() -> Result<Vec<WebSite>, ApiError> {
    client::request_all("/api/v1/web-sites").await
}

/// The rule view of a pipeline that is one.
fn view(p: Pipeline) -> Option<SiteRecipe> {
    let rule = p.rule?;
    Some(SiteRecipe {
        id: p.id,
        site: site_host(&rule.site).unwrap_or_else(|| rule.site.clone()),
        name: rule.name.clone(),
        yaml: p.document,
        version: p.version,
        recipe: Some(rule),
        created_at: p.created_at,
        updated_at: p.updated_at,
    })
}

/// Every rule of the org: its pipelines that are site rules.
pub async fn all() -> Result<Vec<SiteRecipe>, ApiError> {
    Ok(pipelines::list()
        .await?
        .into_iter()
        .filter_map(view)
        .collect())
}

/// The rules of one site (any spelling of its host).
pub async fn list(site: &str) -> Result<Vec<SiteRecipe>, ApiError> {
    let site = site_host(site).unwrap_or_else(|| site.trim().to_string());
    Ok(all()
        .await?
        .into_iter()
        .filter(|r| r.site == site)
        .collect())
}

/// Checks a rule's pipeline document: its rule, or why not.
pub async fn validate(document: &str) -> Result<Recipe, ApiError> {
    pipelines::validate(document).await?.rule.ok_or_else(|| {
        ApiError::local(
            pnex_core::err_codes::SITE_RECIPE_INVALID,
            "This pipeline is not a site rule.",
            None,
        )
    })
}

/// Saves a new rule. A pipeline name is unique in the org, a rule's per
/// site: the pipeline carries both.
pub async fn create(document: &str) -> Result<SiteRecipe, ApiError> {
    let rule = validate(document).await?;
    let site = site_host(&rule.site).unwrap_or_else(|| rule.site.clone());
    let p = pipelines::create(&format!("{} · {site}", rule.name), document).await?;
    view(p).ok_or_else(|| {
        ApiError::local(
            pnex_core::err_codes::SITE_RECIPE_INVALID,
            "Not a site rule.",
            None,
        )
    })
}

/// Saves a new version — 409 when saved in between.
pub async fn update(
    id: &str,
    document: &str,
    expected_version: i32,
) -> Result<SiteRecipe, ApiError> {
    let p = pipelines::update(id, document, expected_version).await?;
    view(p).ok_or_else(|| {
        ApiError::local(
            pnex_core::err_codes::SITE_RECIPE_INVALID,
            "Not a site rule.",
            None,
        )
    })
}

pub async fn delete(id: &str) -> Result<(), ApiError> {
    pipelines::delete(id).await
}

/// `POST /api/v1/navigations` — answers the run, `running`.
pub async fn play(input: &RunInput) -> Result<SiteRecipeRun, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/navigations",
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/navigations?site=…` — journal of a site.
pub async fn runs(site: &str, limit: i64) -> Result<Paginated<SiteRecipeRun>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/navigations?site={}&limit={limit}", urlencode(site)),
        None,
    )
    .await
}

/// `GET /api/v1/navigations/{id}` — steps, extracts, changes.
pub async fn run(id: &str) -> Result<SiteRecipeRun, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/navigations/{}", urlencode(id)),
        None,
    )
    .await
}

/// Path of a run file (`step-3.jpg`, `pdf-5.pdf`).
pub fn file_path(run_id: &str, file: &str) -> String {
    format!(
        "/api/v1/navigations/{}/files/{}",
        urlencode(run_id),
        urlencode(file)
    )
}

/// `GET /api/v1/navigations/{id}/live`: the last live view frame, empty
/// when there is none (204).
pub async fn live(run_id: &str) -> Result<Vec<u8>, ApiError> {
    client::request_bytes(
        reqwest::Method::GET,
        &format!("/api/v1/navigations/{}/live", urlencode(run_id)),
    )
    .await
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

/// The rule's versions, newest first.
pub async fn versions(
    id: &str,
) -> Result<Vec<pnex_core::site_recipe::SiteRecipeVersion>, ApiError> {
    Ok(pipelines::versions(id)
        .await?
        .into_iter()
        .map(|v| pnex_core::site_recipe::SiteRecipeVersion {
            version: v.version,
            yaml: v.document,
            author: v.author,
            created_at: v.created_at,
        })
        .collect())
}

/// Makes an old version the active one: saved again as a new version.
pub async fn restore(id: &str, version: i32) -> Result<SiteRecipe, ApiError> {
    let current = pipelines::get(id).await?.version;
    let old = versions(id)
        .await?
        .into_iter()
        .find(|v| v.version == version)
        .ok_or_else(|| {
            ApiError::local(
                pnex_core::err_codes::SITE_RECIPE_NOT_FOUND,
                "No such version.",
                None,
            )
        })?;
    update(id, &old.yaml, current).await
}

/// `GET /api/v1/pipelines/{id}/flows`: the deployed flows that play it.
pub async fn schedule(id: &str) -> Result<pnex_core::site_recipe::SiteRecipeSchedule, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/pipelines/{}/flows", urlencode(id)),
        None,
    )
    .await
}
