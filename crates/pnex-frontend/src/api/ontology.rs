//! Ontology API (ontology.md D176–D191): `/api/v1/ontology/*`.

use pnex_core::ontology::api::{
    ChangeView, LinkClose, LinkInput, LinkTypeInput, LinkTypeView, LinkView, Neighborhood,
    ObjectInput, ObjectRef, ObjectTypeInput, ObjectTypeVersionView, ObjectTypeView, ObjectView,
    OntologyQuery, PackView, QueryResult, SeriesView,
};
use pnex_core::ontology::LinkTypeDef;
use reqwest::Method;

use crate::api::client;
use crate::api::error::ApiError;
use crate::api::media::urlencode;

fn body<T: serde::Serialize>(input: &T) -> Option<serde_json::Value> {
    Some(serde_json::to_value(input).unwrap_or_default())
}

const BASE: &str = "/api/v1/ontology";

pub async fn types() -> Result<Vec<ObjectTypeView>, ApiError> {
    client::request(Method::GET, &format!("{BASE}/types"), None).await
}

pub async fn create_type(input: &ObjectTypeInput) -> Result<ObjectTypeView, ApiError> {
    client::request(Method::POST, &format!("{BASE}/types"), body(input)).await
}

/// New version on top of `expected_version` (409 `ontology-version-conflict`).
pub async fn update_type(key: &str, input: &ObjectTypeInput) -> Result<ObjectTypeView, ApiError> {
    client::request(
        Method::PUT,
        &format!("{BASE}/types/{}", urlencode(key)),
        body(input),
    )
    .await
}

pub async fn delete_type(key: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(
        Method::DELETE,
        &format!("{BASE}/types/{}", urlencode(key)),
        None,
    )
    .await
}

pub async fn type_versions(key: &str) -> Result<Vec<ObjectTypeVersionView>, ApiError> {
    client::request(
        Method::GET,
        &format!("{BASE}/types/{}/versions", urlencode(key)),
        None,
    )
    .await
}

pub async fn link_types() -> Result<Vec<LinkTypeView>, ApiError> {
    client::request(Method::GET, &format!("{BASE}/link-types"), None).await
}

pub async fn create_link_type(def: &LinkTypeDef) -> Result<LinkTypeView, ApiError> {
    client::request(Method::POST, &format!("{BASE}/link-types"), body(def)).await
}

pub async fn update_link_type(key: &str, input: &LinkTypeInput) -> Result<LinkTypeView, ApiError> {
    client::request(
        Method::PUT,
        &format!("{BASE}/link-types/{}", urlencode(key)),
        body(input),
    )
    .await
}

pub async fn delete_link_type(key: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(
        Method::DELETE,
        &format!("{BASE}/link-types/{}", urlencode(key)),
        None,
    )
    .await
}

pub async fn query(q: &OntologyQuery) -> Result<QueryResult, ApiError> {
    client::request(Method::POST, &format!("{BASE}/query"), body(q)).await
}

pub async fn object(id: &str) -> Result<ObjectView, ApiError> {
    client::request(Method::GET, &format!("{BASE}/objects/{id}"), None).await
}

/// Identity of a system object from its native key (device id, media id…).
pub async fn resolve(kind: &str, native_id: &str) -> Result<ObjectView, ApiError> {
    client::request(
        Method::GET,
        &format!(
            "{BASE}/resolve/{}/{}",
            urlencode(kind),
            urlencode(native_id)
        ),
        None,
    )
    .await
}

pub async fn create_object(input: &ObjectInput) -> Result<ObjectView, ApiError> {
    client::request(Method::POST, &format!("{BASE}/objects"), body(input)).await
}

pub async fn update_object(id: &str, input: &ObjectInput) -> Result<ObjectView, ApiError> {
    client::request(Method::PUT, &format!("{BASE}/objects/{id}"), body(input)).await
}

pub async fn archive_object(id: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(Method::DELETE, &format!("{BASE}/objects/{id}"), None).await
}

/// Links of an object: valid at `as_of` (RFC 3339, default now), or every
/// link ever with `history`.
pub async fn object_links(
    id: &str,
    as_of: Option<&str>,
    history: bool,
) -> Result<Vec<LinkView>, ApiError> {
    let mut path = format!("{BASE}/objects/{id}/links?history={history}");
    if let Some(t) = as_of {
        path.push_str(&format!("&as_of={}", urlencode(t)));
    }
    client::request(Method::GET, &path, None).await
}

pub async fn graph(id: &str, depth: u32) -> Result<Neighborhood, ApiError> {
    client::request(
        Method::GET,
        &format!("{BASE}/objects/{id}/graph?depth={depth}"),
        None,
    )
    .await
}

pub async fn impact(id: &str, upstream: bool) -> Result<Vec<ObjectRef>, ApiError> {
    client::request(
        Method::GET,
        &format!("{BASE}/objects/{id}/impact?upstream={upstream}"),
        None,
    )
    .await
}

pub async fn series(id: &str, property: &str, window: &str) -> Result<SeriesView, ApiError> {
    client::request(
        Method::GET,
        &format!(
            "{BASE}/objects/{id}/series/{}?window={}",
            urlencode(property),
            urlencode(window)
        ),
        None,
    )
    .await
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct Changes {
    pub available: bool,
    pub changes: Vec<ChangeView>,
}

pub async fn changes(id: &str) -> Result<Changes, ApiError> {
    client::request(Method::GET, &format!("{BASE}/objects/{id}/changes"), None).await
}

pub async fn create_link(input: &LinkInput) -> Result<LinkView, ApiError> {
    client::request(Method::POST, &format!("{BASE}/links"), body(input)).await
}

pub async fn close_link(id: i64, close: &LinkClose) -> Result<LinkView, ApiError> {
    client::request(
        Method::POST,
        &format!("{BASE}/links/{id}/close"),
        body(close),
    )
    .await
}

pub async fn packs() -> Result<Vec<PackView>, ApiError> {
    client::request(Method::GET, &format!("{BASE}/packs"), None).await
}

pub async fn install_pack(key: &str) -> Result<PackView, ApiError> {
    client::request(
        Method::POST,
        &format!("{BASE}/packs/{}/install", urlencode(key)),
        None,
    )
    .await
}

/// The org schema as pack YAML.
pub async fn export_yaml() -> Result<Vec<u8>, ApiError> {
    client::request_bytes(Method::GET, &format!("{BASE}/export")).await
}

pub async fn import_yaml(yaml: Vec<u8>) -> Result<PackView, ApiError> {
    client::request_upload(Method::POST, &format!("{BASE}/import"), yaml).await
}
