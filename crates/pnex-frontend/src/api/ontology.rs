//! Ontology API (ontology.md D176–D191): `/api/v1/ontology/*`.

use pnex_core::ontology::api::{
    ChangeView, LinkClose, LinkInput, LinkTypeView, LinkView, Neighborhood, ObjectImpact,
    ObjectInput, ObjectPath, ObjectRef, ObjectTypeInput, ObjectTypeVersionView, ObjectTypeView,
    ObjectView, OntologyQuery, PackView, QueryResult, SeriesView,
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
    client::request_all(&format!("{BASE}/types")).await
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
    client::request_all(&format!("{BASE}/types/{}/versions", urlencode(key))).await
}

pub async fn link_types() -> Result<Vec<LinkTypeView>, ApiError> {
    client::request_all(&format!("{BASE}/link-types")).await
}

pub async fn create_link_type(def: &LinkTypeDef) -> Result<LinkTypeView, ApiError> {
    client::request(Method::POST, &format!("{BASE}/link-types"), body(def)).await
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

/// Invokes an action of the object's type (D183): the flows it was sent to.
pub async fn invoke_action(
    id: &str,
    action: &str,
    params: &serde_json::Map<String, serde_json::Value>,
) -> Result<pnex_core::ontology::action::ActionInvocation, ApiError> {
    client::request(
        Method::POST,
        &format!("{BASE}/objects/{id}/actions/{}", urlencode(action)),
        Some(serde_json::json!({ "params": params })),
    )
    .await
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
    client::request_all(&path).await
}

/// Hierarchy tree under an object (D305); empty `key` = the first one,
/// `at` = a `YYYY-MM-DD` day (links valid at its end), empty = now.
pub async fn hierarchy(
    id: &str,
    key: &str,
    at: &str,
) -> Result<pnex_core::ontology::api::HierarchyView, ApiError> {
    let mut path = format!("{BASE}/objects/{id}/hierarchy?");
    if !key.is_empty() {
        path.push_str(&format!("key={}&", urlencode(key)));
    }
    if !at.is_empty() {
        path.push_str(&format!("at={}", urlencode(&format!("{at}T23:59:59Z"))));
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

/// Shortest chain of links between two objects (both directions, bounded).
pub async fn path(from: &str, to: &str) -> Result<Vec<ObjectRef>, ApiError> {
    let found: ObjectPath = client::request(
        Method::GET,
        &format!("{BASE}/path?from={from}&to={to}"),
        None,
    )
    .await?;
    Ok(found.path)
}

pub async fn impact(id: &str, upstream: bool) -> Result<Vec<ObjectRef>, ApiError> {
    let found: ObjectImpact = client::request(
        Method::GET,
        &format!("{BASE}/objects/{id}/impact?upstream={upstream}"),
        None,
    )
    .await?;
    Ok(found.objects)
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

/// Current sensor of each bound series property (type dashboards, D187).
pub async fn bindings(id: &str) -> Result<Vec<pnex_core::SeriesBinding>, ApiError> {
    client::request_all(&format!("{BASE}/objects/{id}/bindings")).await
}

/// History of an object; `available` = false when the server answers
/// `ontology-changes-unavailable` (the change journal cannot be read).
#[derive(Debug, Clone, PartialEq)]
pub struct Changes {
    pub available: bool,
    pub changes: Vec<ChangeView>,
}

pub async fn changes(id: &str) -> Result<Changes, ApiError> {
    match client::request_all(&format!("{BASE}/objects/{id}/changes")).await {
        Ok(changes) => Ok(Changes {
            available: true,
            changes,
        }),
        Err(e) if e.code.as_deref() == Some(pnex_core::err_codes::ONTOLOGY_CHANGES_UNAVAILABLE) => {
            Ok(Changes {
                available: false,
                changes: Vec::new(),
            })
        }
        Err(e) => Err(e),
    }
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
    client::request_all(&format!("{BASE}/packs")).await
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

// ─────────────────────────── imports (D301) ───────────────────────────

use pnex_core::ontology::import::{
    ImportApply, ImportPreview, ImportRunView, ObjectImportInput, ObjectImportView,
};

pub async fn imports() -> Result<Vec<ObjectImportView>, ApiError> {
    client::request_all(&format!("{BASE}/imports")).await
}

pub async fn create_import(input: &ObjectImportInput) -> Result<ObjectImportView, ApiError> {
    client::request(Method::POST, &format!("{BASE}/imports"), body(input)).await
}

pub async fn update_import(
    id: &str,
    input: &ObjectImportInput,
) -> Result<ObjectImportView, ApiError> {
    client::request(Method::PUT, &format!("{BASE}/imports/{id}"), body(input)).await
}

pub async fn delete_import(id: &str) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(Method::DELETE, &format!("{BASE}/imports/{id}"), None)
        .await
        .map(|_| ())
}

pub async fn import_runs(id: &str) -> Result<Vec<ImportRunView>, ApiError> {
    client::request_all(&format!("{BASE}/imports/{id}/runs")).await
}

pub async fn import_run(id: &str, run: &str) -> Result<ImportPreview, ApiError> {
    client::request(
        Method::GET,
        &format!("{BASE}/imports/{id}/runs/{run}"),
        None,
    )
    .await
}

/// Starts a preview; `file` = the bytes of a file import (empty for URL
/// and API sources).
pub async fn preview_import(
    id: &str,
    name: &str,
    file: Vec<u8>,
) -> Result<ImportRunView, ApiError> {
    client::request_upload(
        Method::POST,
        &format!("{BASE}/imports/{id}/dry-run?filename={}", urlencode(name)),
        file,
    )
    .await
}

/// Starts a run now (steps, then application); answers it `running`.
pub async fn start_import(id: &str) -> Result<ImportRunView, ApiError> {
    client::request(Method::POST, &format!("{BASE}/imports/{id}/runs"), None).await
}

pub async fn apply_import(id: &str, input: &ImportApply) -> Result<ImportRunView, ApiError> {
    client::request(
        Method::POST,
        &format!("{BASE}/imports/{id}/apply"),
        body(input),
    )
    .await
}
