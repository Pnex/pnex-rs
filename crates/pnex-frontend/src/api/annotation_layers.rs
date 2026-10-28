//! Media annotations API (D55–D60) — client of the controllers
//! `/api/v1/annotation-layers` + read model `/api/v1/media/{id}/annotations`
//! (contract `docs/contracts/annotation-layers.http`). Types from
//! `pnex-core` (shared source of truth, wasm32); `update` carries the
//! optimistic concurrency (`expected_version_number`, 409 if stale — same
//! approach as `api/tours.rs`).

use pnex_core::{
    AnnotationLayerDetail, AnnotationLayerSummary, AnnotationLayerVersionDetail,
    AnnotationLayerVersionSummary, AnnotationViolation, CreateAnnotationLayer, MediaAnnotations,
    Paginated, PublishAnnotationLayer, UpdateAnnotationLayer,
};

use crate::api::client;
use crate::api::error::ApiError;

/// Filters of `GET /api/v1/annotation-layers` + pagination (D14).
#[derive(Default)]
pub struct LayerFilters {
    pub search: Option<String>,
    /// Exact filter by associated media (UX pivot: the sets of a media).
    pub media: Option<String>,
    /// Exact filter by associated tour (000028: the sets of a tour).
    pub tour: Option<String>,
    /// D218 label filter (`name` or `name:value`).
    pub label: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl LayerFilters {
    fn to_query(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(v) = &self.search {
            parts.push(format!("search={}", crate::api::tours::urlencode(v)));
        }
        if let Some(v) = &self.media {
            parts.push(format!("media={}", crate::api::tours::urlencode(v)));
        }
        if let Some(v) = &self.tour {
            parts.push(format!("tour={}", crate::api::tours::urlencode(v)));
        }
        if let Some(v) = self.label.as_deref().filter(|v| !v.is_empty()) {
            parts.push(format!("label={}", crate::api::tours::urlencode(v)));
        }
        if let Some(v) = self.limit {
            parts.push(format!("limit={v}"));
        }
        if let Some(v) = self.offset {
            parts.push(format!("offset={v}"));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("?{}", parts.join("&"))
        }
    }
}

/// `GET /api/v1/media/{asset_id}/annotations[?tour=]` — viewers read model:
/// items merged from the published layers (union, deterministic order) with
/// resolved targets (`device_pk`/`dead`). Context (D147): `tour` = that
/// tour's sets only; `None` = the sets of the standalone media only.
pub async fn media_annotations(
    asset_id: &str,
    tour: Option<&str>,
) -> Result<MediaAnnotations, ApiError> {
    let tq = tour
        .map(|t| format!("?tour={}", crate::api::tours::urlencode(t)))
        .unwrap_or_default();
    client::request(
        reqwest::Method::GET,
        &format!(
            "/api/v1/media/{}/annotations{tq}",
            crate::api::tours::urlencode(asset_id)
        ),
        None,
    )
    .await
}

/// `GET /api/v1/annotation-layers` — every layer of the current org
/// matching the filters (`limit`/`offset` are driven by `request_all`).
pub async fn list(filters: &LayerFilters) -> Result<Vec<AnnotationLayerSummary>, ApiError> {
    client::request_all(&format!("/api/v1/annotation-layers{}", filters.to_query())).await
}

/// `POST /api/v1/annotation-layers` — creation (layer + empty v1 doc).
pub async fn create(params: CreateAnnotationLayer) -> Result<AnnotationLayerDetail, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/annotation-layers",
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}
/// `GET /api/v1/annotation-layers/{id}[?version=n]` — detail (doc of the
/// latest version, or of the requested version).
pub async fn detail(id: &str, version: Option<i64>) -> Result<AnnotationLayerDetail, ApiError> {
    let vq = version.map(|v| format!("?version={v}")).unwrap_or_default();
    client::request(
        reqwest::Method::GET,
        &format!(
            "/api/v1/annotation-layers/{}{vq}",
            crate::api::tours::urlencode(id)
        ),
        None,
    )
    .await
}

/// `PATCH /api/v1/annotation-layers/{id}` — saves a **new version**
/// (append-only); 409 if `expected_version_number` is stale,
/// 400 `{"violations": […]}` if the document is invalid.
pub async fn update(
    id: &str,
    params: UpdateAnnotationLayer,
) -> Result<AnnotationLayerDetail, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!(
            "/api/v1/annotation-layers/{}",
            crate::api::tours::urlencode(id)
        ),
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/annotation-layers/{id}` — couche + versions (cascade).
pub async fn delete(id: &str) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!(
            "/api/v1/annotation-layers/{}",
            crate::api::tours::urlencode(id)
        ),
        None,
    )
    .await
    .map(|_| ())
}

/// `GET /api/v1/annotation-layers/{id}/versions` — historique, desc.
pub async fn versions(
    id: &str,
    limit: i64,
    offset: i64,
) -> Result<Paginated<AnnotationLayerVersionSummary>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!(
            "/api/v1/annotation-layers/{}/versions?limit={limit}&offset={offset}",
            crate::api::tours::urlencode(id)
        ),
        None,
    )
    .await
}

/// `GET /api/v1/annotation-layers/{id}/versions/{n}` — document historique.
pub async fn version(
    id: &str,
    version_number: i64,
) -> Result<AnnotationLayerVersionDetail, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!(
            "/api/v1/annotation-layers/{}/versions/{version_number}",
            crate::api::tours::urlencode(id)
        ),
        None,
    )
    .await
}
/// `POST /api/v1/annotation-layers/{id}/publish` — publishes a version
/// (missing = latest). Returns the updated detail.
pub async fn publish(
    id: &str,
    version_number: Option<i64>,
) -> Result<AnnotationLayerDetail, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!(
            "/api/v1/annotation-layers/{}/publish",
            crate::api::tours::urlencode(id)
        ),
        Some(serde_json::to_value(PublishAnnotationLayer { version_number }).unwrap_or_default()),
    )
    .await
}

/// `POST /api/v1/annotation-layers/{id}/unpublish` — pointer → NULL: the
/// items disappear from every viewer.
pub async fn unpublish(id: &str) -> Result<AnnotationLayerDetail, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!(
            "/api/v1/annotation-layers/{}/unpublish",
            crate::api::tours::urlencode(id)
        ),
        Some(serde_json::json!({})),
    )
    .await
}

/// Save failure, sorted for the UI (same approach as
/// `api/tours.rs::classify_save_error`): concurrency conflict (409, "reload /
/// overwrite" modal), invalid document (400 violations, banner), or other (verbatim toast).
#[derive(Debug, Clone, PartialEq)]
pub enum SaveError {
    Conflict { description: String },
    Invalid(Vec<AnnotationViolation>),
    Other(String),
}

/// Classifies an `ApiError` from `update()` — only a 409 or a 400 with a
/// `violations` field has usable semantics.
pub fn classify_save_error(err: &ApiError) -> SaveError {
    if err.status == Some(409) {
        return SaveError::Conflict {
            description: err.message.clone(),
        };
    }
    if err.status == Some(400) {
        if let Some(v) = err.violations() {
            if let Ok(violations) = serde_json::from_value::<Vec<AnnotationViolation>>(v.clone()) {
                return SaveError::Invalid(violations);
            }
        }
    }
    SaveError::Other(err.message.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_des_echecs_de_save() {
        let conflict = ApiError::http(
            409,
            r#"{"error":"conflict","description":"version périmée"}"#,
        );
        assert!(matches!(
            classify_save_error(&conflict),
            SaveError::Conflict { .. }
        ));

        let invalid = ApiError::http(
            400,
            r#"{"error":"x-invalid","errors":{"violations":[{"subject":null,"code":"unknown_link_target","message":"lien orphelin"}]}}"#,
        );
        match classify_save_error(&invalid) {
            SaveError::Invalid(violations) => {
                assert_eq!(violations[0].code, "unknown_link_target");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }

        let other = ApiError::http(403, r#"{"error":"forbidden"}"#);
        assert!(matches!(classify_save_error(&other), SaveError::Other(_)));
    }
}
