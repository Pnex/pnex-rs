//! Client API of the cross-cutting organisation layer (D42): labels,
//! containment, edges, folders, cross-kind search. Mirror types of the
//! `/api/v1/resources` controller.

#![allow(dead_code)]

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::api::client;
use crate::api::error::ApiError;

/// Labels of a resource (own).
pub type LabelSet = BTreeMap<String, Option<String>>;

/// D42 edge (cross links with placement).
#[derive(Debug, Clone, Deserialize)]
pub struct Edge {
    pub id: i64,
    pub relation: String,
    pub source_kind: String,
    pub source_id: String,
    pub target_kind: String,
    pub target_id: String,
    pub placement: Option<serde_json::Value>,
    pub created_at: String,
}

/// Dossier (kind `folder`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Folder {
    pub id: i64,
    pub name: String,
    pub emoji: Option<String>,
    created_at: String,
    updated_at: String,
}

/// Cross-kind search (effective labels).
#[derive(Debug, Clone, Deserialize)]
pub struct SearchHit {
    pub kind: String,
    pub id: String,
}

// ─────────────────────────── labels ───────────────────────────

/// Max ids accepted by one `labels/query` call (server bound).
pub const LABELS_BATCH_MAX: usize = 200;

/// `POST /api/v1/resources/labels/query`: own labels of a page of rows
/// (`kind` + stringified PKs, at most [`LABELS_BATCH_MAX`]). Ids without
/// labels are absent from the returned map.
pub async fn labels_batch(
    kind: &str,
    ids: Vec<String>,
) -> Result<std::collections::HashMap<String, LabelSet>, ApiError> {
    #[derive(Deserialize)]
    struct Dto {
        #[serde(default)]
        labels: std::collections::HashMap<String, LabelSet>,
    }
    let dto: Dto = client::request(
        reqwest::Method::POST,
        "/api/v1/resources/labels/query",
        Some(serde_json::json!({ "kind": kind, "ids": ids })),
    )
    .await?;
    Ok(dto.labels)
}

/// A label name already used in the org, with its known values.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LabelCatalogEntry {
    pub name: String,
    #[serde(default)]
    pub values: Vec<String>,
    pub count: i64,
}

/// `GET /api/v1/resources/labels/catalog`: picker suggestions (all pages).
pub async fn labels_catalog() -> Result<Vec<LabelCatalogEntry>, ApiError> {
    client::request_all("/api/v1/resources/labels/catalog").await
}

/// `GET /api/v1/resources/{kind}/{id}/labels`.
pub async fn get_labels(kind: &str, id: &str) -> Result<LabelSet, ApiError> {
    #[derive(Deserialize)]
    struct Dto {
        labels: LabelSet,
    }
    let dto: Dto = client::request(
        reqwest::Method::GET,
        &format!("/api/v1/resources/{kind}/{id}/labels"),
        None,
    )
    .await?;
    Ok(dto.labels)
}

/// `PUT /api/v1/resources/{kind}/{id}/labels` — replaces the whole doc.
pub async fn put_labels(kind: &str, id: &str, labels: &LabelSet) -> Result<LabelSet, ApiError> {
    #[derive(Deserialize)]
    struct Dto {
        labels: LabelSet,
    }
    let dto: Dto = client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/resources/{kind}/{id}/labels"),
        Some(serde_json::json!({ "labels": labels })),
    )
    .await?;
    Ok(dto.labels)
}

// ─────────────────────────── containment ───────────────────────────

/// Tree node (parent/path/children).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RefNode {
    pub kind: String,
    pub id: String,
    /// Display name (folders only).
    pub name: Option<String>,
    pub emoji: Option<String>,
    pub sort_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Containment {
    pub parent: Option<RefNode>,
    /// Path leaf → root (parent excluded).
    pub path: Vec<RefNode>,
    pub children: Vec<RefNode>,
}

/// `GET …/containment`.
pub async fn get_containment(kind: &str, id: &str) -> Result<Containment, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/resources/{kind}/{id}/containment"),
        None,
    )
    .await
}

/// `PUT …/containment` — (re)parents; `None` = detaches.
pub async fn set_parent(
    kind: &str,
    id: &str,
    parent: Option<(&str, &str)>,
) -> Result<Containment, ApiError> {
    let body = match parent {
        Some((pk, pi)) => serde_json::json!({"parent": {"kind": pk, "id": pi}}),
        None => serde_json::json!({"parent": null}),
    };
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/resources/{kind}/{id}/containment"),
        Some(body),
    )
    .await
}

// ─────────────────────────── edges ───────────────────────────

/// `GET /api/v1/resources/edges?relation=&source_kind=&source_id=…` (all
/// pages).
pub async fn list_edges(
    relation: Option<&str>,
    source: Option<(&str, &str)>,
    target: Option<(&str, &str)>,
) -> Result<Vec<Edge>, ApiError> {
    let mut params: Vec<String> = Vec::new();
    if let Some(r) = relation {
        params.push(format!("relation={r}"));
    }
    if let Some((k, i)) = source {
        params.push(format!("source_kind={k}&source_id={i}"));
    }
    if let Some((k, i)) = target {
        params.push(format!("target_kind={k}&target_id={i}"));
    }
    client::request_all(&format!("/api/v1/resources/edges?{}", params.join("&"))).await
}

/// `POST /api/v1/resources/edges` — 201 or 409 (duplicate).
pub async fn create_edge(
    relation: &str,
    source: (&str, &str),
    target: (&str, &str),
    placement: Option<serde_json::Value>,
) -> Result<Edge, ApiError> {
    let (sk, si) = source;
    let (tk, ti) = target;
    client::request(
        reqwest::Method::POST,
        "/api/v1/resources/edges",
        Some(serde_json::json!({
            "relation": relation,
            "source_kind": sk,
            "source_id": si,
            "target_kind": tk,
            "target_id": ti,
            "placement": placement,
        })),
    )
    .await
}

/// `DELETE /api/v1/resources/edges/{id}`.
/// 204 without body → `request_opt` (a `request` would raise "empty response").
pub async fn delete_edge(id: i64) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/resources/edges/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

// ─────────────────────────── folders ───────────────────────────

/// `GET /api/v1/resources/folders` (all pages).
pub async fn list_folders() -> Result<Vec<Folder>, ApiError> {
    client::request_all("/api/v1/resources/folders").await
}

/// `POST /api/v1/resources/folders`.
pub async fn create_folder(name: &str, emoji: Option<&str>) -> Result<Folder, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/resources/folders",
        Some(serde_json::json!({"name": name, "emoji": emoji})),
    )
    .await
}

/// `DELETE /api/v1/resources/folders/{id}` — subtree re-rooted.
/// 204 without body → `request_opt` (a `request` would raise "empty response").
pub async fn delete_folder(id: i64) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/resources/folders/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

/// `PATCH /api/v1/resources/folders/{id}` — `None` = absent field;
/// `Some(None)` = clears the emoji (the controller's `Option<Option>` idiom).
pub async fn patch_folder(
    id: i64,
    name: Option<&str>,
    emoji: Option<Option<&str>>,
) -> Result<Folder, ApiError> {
    let mut body = serde_json::Map::new();
    if let Some(name) = name {
        body.insert("name".into(), serde_json::json!(name));
    }
    if let Some(emoji) = emoji {
        body.insert(
            "emoji".into(),
            match emoji {
                Some(e) => serde_json::json!(e),
                None => serde_json::Value::Null,
            },
        );
    }
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/resources/folders/{id}"),
        Some(serde_json::Value::Object(body)),
    )
    .await
}

// ─────────────────────────── recherche ───────────────────────────

/// `GET /api/v1/resources/search?label=&kinds=`: cross-kind label search
/// (all pages).
pub async fn search(label: &str, kinds: Option<Vec<&str>>) -> Result<Vec<SearchHit>, ApiError> {
    let kinds = kinds.map(|k| k.join(","));
    let query = client::filter_query(&[("label", Some(label)), ("kinds", kinds.as_deref())]);
    client::request_all(&format!("/api/v1/resources/search{query}")).await
}

/// `GET /api/v1/resources/{kind}/{id}/location` — site breadcrumb(s).
pub async fn location(
    kind: &str,
    id: &str,
) -> Result<pnex_core::resources::ResourceLocations, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/resources/{kind}/{id}/location"),
        None,
    )
    .await
}
