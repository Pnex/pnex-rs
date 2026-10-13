//! API client of Pages (pages.md #21): list, create, rename, delete, named
//! versions, and the websocket URL of the live editor.

use pnex_core::pages::{
    CreateDoc, CreateDocComment, CreateDocVersion, DocComment, DocSummary, DocVersionState,
    DocVersionSummary, PatchDoc, PatchDocComment,
};
use pnex_core::Paginated;
use uuid::Uuid;

use crate::api::client;
use crate::api::error::ApiError;

pub async fn list() -> Result<Vec<DocSummary>, ApiError> {
    let page: Paginated<DocSummary> =
        client::request(reqwest::Method::GET, "/api/v1/docs?limit=200", None).await?;
    Ok(page.results)
}

pub async fn create(title: String) -> Result<DocSummary, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/docs",
        Some(serde_json::to_value(CreateDoc { title }).unwrap_or_default()),
    )
    .await
}

pub async fn detail(id: Uuid) -> Result<DocSummary, ApiError> {
    client::request(reqwest::Method::GET, &format!("/api/v1/docs/{id}"), None).await
}

pub async fn rename(id: Uuid, title: String) -> Result<DocSummary, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/docs/{id}"),
        Some(serde_json::to_value(PatchDoc { title }).unwrap_or_default()),
    )
    .await
}

pub async fn delete(id: Uuid) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/docs/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

#[derive(serde::Deserialize)]
struct VersionList {
    results: Vec<DocVersionSummary>,
}

pub async fn versions(id: Uuid) -> Result<Vec<DocVersionSummary>, ApiError> {
    let list: VersionList = client::request(
        reqwest::Method::GET,
        &format!("/api/v1/docs/{id}/versions"),
        None,
    )
    .await?;
    Ok(list.results)
}

pub async fn create_version(id: Uuid, label: String) -> Result<DocVersionSummary, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/docs/{id}/versions"),
        Some(serde_json::to_value(CreateDocVersion { label }).unwrap_or_default()),
    )
    .await
}

/// `POST /api/v1/docs/{id}/versions/{n}/approve` — a person approves a
/// frozen version (D210).
pub async fn approve_version(id: Uuid, n: i64) -> Result<DocVersionSummary, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/docs/{id}/versions/{n}/approve"),
        None,
    )
    .await
}

/// `GET /api/v1/docs/{id}/versions/{n}/pdf` — the version as a PDF, in
/// the UI language (D209).
pub async fn version_pdf(id: Uuid, n: i64) -> Result<Vec<u8>, ApiError> {
    let lang = crate::i18n::current_tag();
    client::request_bytes(
        reqwest::Method::GET,
        &format!("/api/v1/docs/{id}/versions/{n}/pdf?lang={lang}"),
    )
    .await
}

pub async fn version_state(id: Uuid, n: i64) -> Result<DocVersionState, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/docs/{id}/versions/{n}"),
        None,
    )
    .await
}

#[derive(serde::Deserialize)]
struct CommentList {
    results: Vec<DocComment>,
}

pub async fn comments(id: Uuid) -> Result<Vec<DocComment>, ApiError> {
    let list: CommentList = client::request(
        reqwest::Method::GET,
        &format!("/api/v1/docs/{id}/comments"),
        None,
    )
    .await?;
    Ok(list.results)
}

pub async fn create_comment(id: Uuid, input: CreateDocComment) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::POST,
        &format!("/api/v1/docs/{id}/comments"),
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
    .map(|_| ())
}

pub async fn update_comment(id: Uuid, cid: i64, input: PatchDocComment) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::PATCH,
        &format!("/api/v1/docs/{id}/comments/{cid}"),
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
    .map(|_| ())
}

pub async fn delete_comment(id: Uuid, cid: i64) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/docs/{id}/comments/{cid}"),
        None,
    )
    .await
    .map(|_| ())
}

/// `/ws/doc/{id}` URL with a fresh one-time ticket; `None` without a
/// session or when the server is unreachable.
pub async fn live_ws_url(id: Uuid) -> Option<String> {
    let ticket = crate::api::cameras::ws_ticket().await.ok()?;
    let base = crate::api::config::api_base();
    Some(format!(
        "{}/ws/doc/{id}?ticket={}",
        crate::api::cameras::ws_base(&base),
        crate::api::media::urlencode(&ticket)
    ))
}
