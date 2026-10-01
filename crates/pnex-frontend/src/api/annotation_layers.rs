//! API annotations sur médias (D55–D60) — client des contrôleurs
//! `/api/v1/annotation-layers` + read model `/api/v1/media/{id}/annotations`
//! (contrat `docs/contracts/annotation-layers.http`). Types depuis
//! `pnex-core` (source de vérité partagée, wasm32) ; `update` porte la
//! concurrence optimiste (`expected_version_number`, 409 si périmé — école
//! `api/tours.rs`).

use pnex_core::{
    AnnotationLayerDetail, AnnotationLayerSummary, AnnotationLayerVersionDetail,
    AnnotationLayerVersionSummary, AnnotationViolation, CreateAnnotationLayer, MediaAnnotations,
    Paginated, PublishAnnotationLayer, UpdateAnnotationLayer,
};

use crate::api::client;
use crate::api::error::ApiError;

/// Filtres de `GET /api/v1/annotation-layers` + pagination (D14).
#[derive(Default)]
pub struct LayerFilters {
    pub search: Option<String>,
    /// Filtre exact par média associé (pivot UX : les ensembles d'un média).
    pub media: Option<String>,
    /// Filtre exact par tour associé (000028 : les ensembles d'un tour).
    pub tour: Option<String>,
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

/// `GET /api/v1/media/{asset_id}/annotations` — read model viewers : items
/// fusionnés des couches publiées (union, ordre déterministe) avec cibles
/// résolues (`device_pk`/`dead`).
pub async fn media_annotations(asset_id: &str) -> Result<MediaAnnotations, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!(
            "/api/v1/media/{}/annotations",
            crate::api::tours::urlencode(asset_id)
        ),
        None,
    )
    .await
}

/// `GET /api/v1/annotation-layers` — couches de l'org courante, enveloppe
/// paginée.
pub async fn list(filters: &LayerFilters) -> Result<Paginated<AnnotationLayerSummary>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/annotation-layers{}", filters.to_query()),
        None,
    )
    .await
}

/// `POST /api/v1/annotation-layers` — création (couche + v1 doc vide).
pub async fn create(params: CreateAnnotationLayer) -> Result<AnnotationLayerDetail, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/annotation-layers",
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}
/// `GET /api/v1/annotation-layers/{id}[?version=n]` — détail (doc de la
/// dernière version, ou de la version demandée).
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

/// `PATCH /api/v1/annotation-layers/{id}` — enregistre une **nouvelle
/// version** (append-only) ; 409 si `expected_version_number` est périmé,
/// 400 `{"violations": […]}` si le document est invalide.
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
/// Pas encore d'UI de purge (les couches implicites sont invisibles).
#[allow(dead_code)]
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
/// `POST /api/v1/annotation-layers/{id}/publish` — publie une version
/// (absente = dernière). Retourne le détail mis à jour.
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

/// `POST /api/v1/annotation-layers/{id}/unpublish` — pointeur → NULL : les
/// items disparaissent de tous les viewers.
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

/// Échec d'un save, trié pour l'UI (école `api/tours.rs::classify_save_error`)
/// : conflit de concurrence (409, modal « recharger / écraser »), document
/// invalide (400 violations, bandeau), ou autre (toast verbatim).
#[derive(Debug, Clone, PartialEq)]
pub enum SaveError {
    Conflict { description: String },
    Invalid(Vec<AnnotationViolation>),
    Other(String),
}

/// Classe une `ApiError` issue de `update()` — uniquement un 409 ou un 400
/// à champ `violations` a une sémantique exploitable.
pub fn classify_save_error(err: &ApiError) -> SaveError {
    if err.status == Some(409) {
        return SaveError::Conflict {
            description: err.message.clone(),
        };
    }
    if err.status == Some(400) {
        if let Some(body) = &err.body {
            if let Ok(violations) =
                serde_json::from_value::<Vec<AnnotationViolation>>(body["violations"].clone())
            {
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
            r#"{"violations":[{"subject":null,"code":"unknown_link_target","message":"lien orphelin"}]}"#,
        );
        match classify_save_error(&invalid) {
            SaveError::Invalid(violations) => {
                assert_eq!(violations[0].code, "unknown_link_target");
            }
            other => panic!("attendu Invalid, eu {other:?}"),
        }

        let other = ApiError::http(403, r#"{"error":"forbidden"}"#);
        assert!(matches!(classify_save_error(&other), SaveError::Other(_)));
    }
}
