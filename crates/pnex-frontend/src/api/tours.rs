//! API Studio (parcours 3D versionnés) — client du contrôleur
//! `/api/v1/tours` + endpoint public `/api/v1/public/tours` (contrat
//! `docs/contracts/tours.http`). Types depuis `pnex-core` (source de vérité
//! partagée, wasm32) ; `update` porte la concurrence optimiste
//! (`expected_version_number`, 409 si périmé — école `api/flows.rs`).

use pnex_core::{
    CreateTour, Paginated, PublicTour, PublishTour, TourDetail, TourSummary, TourVersionDetail,
    TourVersionSummary, TourViolation, UpdateTour,
};

use crate::api::client;
use crate::api::error::ApiError;

/// Filtres de `GET /api/v1/tours` + pagination (D14).
#[derive(Default)]
pub struct TourFilters {
    pub search: Option<String>,
    pub mode: Option<String>,
    /// D42 effective label filter (`name` or `name:value`).
    pub label: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl TourFilters {
    fn to_query(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(v) = &self.search {
            parts.push(format!("search={}", urlencode(v)));
        }
        if let Some(v) = &self.mode {
            parts.push(format!("mode={}", urlencode(v)));
        }
        if let Some(v) = self.label.as_deref().filter(|v| !v.is_empty()) {
            parts.push(format!("label={}", urlencode(v)));
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

pub(crate) fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `GET /api/v1/tours` — tours de l'org courante, enveloppe paginée.
pub async fn list(filters: &TourFilters) -> Result<Paginated<TourSummary>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/tours{}", filters.to_query()),
        None,
    )
    .await
}

/// `POST /api/v1/tours` — création (document minimal = version 1).
pub async fn create(params: CreateTour) -> Result<TourDetail, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/tours",
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/tours/{id}` — détail (doc de la dernière version).
pub async fn detail(id: &str) -> Result<TourDetail, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/tours/{}", urlencode(id)),
        None,
    )
    .await
}

/// `PATCH /api/v1/tours/{id}` — enregistre une **nouvelle version**
/// (append-only) ; 409 si `expected_version_number` est périmé, 400
/// `{"violations": […]}` si le document est invalide.
pub async fn update(id: &str, params: UpdateTour) -> Result<TourDetail, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/tours/{}", urlencode(id)),
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/tours/{id}` — tour + versions (cascade ; aucun blob).
pub async fn delete(id: &str) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/tours/{}", urlencode(id)),
        None,
    )
    .await
    .map(|_| ())
}

/// `GET /api/v1/tours/{id}/versions` — historique, desc.
pub async fn versions(
    id: &str,
    limit: i64,
    offset: i64,
) -> Result<Paginated<TourVersionSummary>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!(
            "/api/v1/tours/{}/versions?limit={limit}&offset={offset}",
            urlencode(id)
        ),
        None,
    )
    .await
}

/// `GET /api/v1/tours/{id}/versions/{n}` — document historique.
pub async fn version(id: &str, version_number: i64) -> Result<TourVersionDetail, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/tours/{}/versions/{version_number}", urlencode(id)),
        None,
    )
    .await
}

/// `POST /api/v1/tours/{id}/publish` — publie une version (absente =
/// dernière). La version publiée sert le viewer in-app et le lien public.
pub async fn publish(id: &str, version_number: Option<i64>) -> Result<TourDetail, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/tours/{}/publish", urlencode(id)),
        Some(serde_json::to_value(PublishTour { version_number }).unwrap_or_default()),
    )
    .await
}

/// `POST /api/v1/tours/{id}/unpublish` — dépublie ET révoque le lien public.
pub async fn unpublish(id: &str) -> Result<TourDetail, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/tours/{}/unpublish", urlencode(id)),
        Some(serde_json::json!({})),
    )
    .await
}

/// Réponse de `POST /share` — le front affiche un toast, les champs ne sont
/// pas consommés (le token sert via le détail writer).
#[derive(Debug, Clone, serde::Deserialize)]
#[allow(dead_code)]
pub struct ShareResponse {
    pub share_token: String,
    pub share_enabled: bool,
}

/// `POST /api/v1/tours/{id}/share` — génère (ou régénère) le token public.
/// 409 `not_published` si le tour n'a pas de version publiée.
pub async fn share(id: &str) -> Result<ShareResponse, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/tours/{}/share", urlencode(id)),
        Some(serde_json::json!({})),
    )
    .await
}

/// `DELETE /api/v1/tours/{id}/share` — révoque le lien (publication
/// conservée).
pub async fn revoke_share(id: &str) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/tours/{}/share", urlencode(id)),
        None,
    )
    .await
    .map(|_| ())
}

// ─────────────────────────── Public (sans auth) ───────────────────────────

/// `GET /api/v1/public/tours/{token}` — document publié + carte des assets.
/// Chemin exempté d'auth côté `client.rs` (`PUBLIC_PREFIX`) — jamais de
/// Bearer/X-Org-Id attachés.
pub async fn public_detail(token: &str) -> Result<PublicTour, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/public/tours/{}", urlencode(token)),
        None,
    )
    .await
}

/// Chemin des octets d'un asset référencé par le tour publié (`?v=n` —
/// version précise, cacheable ; absent = courante).
pub fn public_asset_path(token: &str, asset_id: &str, version: Option<i64>) -> String {
    match version {
        Some(n) => format!(
            "/api/v1/public/tours/{}/assets/{}?v={n}",
            urlencode(token),
            urlencode(asset_id)
        ),
        None => format!(
            "/api/v1/public/tours/{}/assets/{}",
            urlencode(token),
            urlencode(asset_id)
        ),
    }
}

/// URL complète d'un lien de partage (page `/share/:token` de la SPA) :
/// web = **origine de la page** (l'API peut être ailleurs en dev hot —
/// `PNEX_API_BASE_URL` — le lien doit viser la SPA, pas l'API) ; natif =
/// URL serveur (la SPA est servie par le backend en production). Le
/// routeur dioxus-web est **WebHistory** : chemin réel, PAS de `#`.
pub fn share_url(token: &str) -> String {
    #[cfg(target_arch = "wasm32")]
    let base = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default();
    #[cfg(not(target_arch = "wasm32"))]
    let base = crate::api::config::api_base();
    format!("{base}/share/{}", urlencode(token))
}

// ─────────────────────────── Échecs de save ───────────────────────────

/// Échec d'un enregistrement, trié pour l'UI (école `api/flows.rs`) :
/// conflit de concurrence (409, modal « recharger / écraser »), document
/// invalide (400 violations, bandeau), ou autre (toast verbatim).
#[derive(Debug, Clone, PartialEq)]
pub enum SaveError {
    Conflict { description: String },
    Invalid(Vec<TourViolation>),
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
                serde_json::from_value::<Vec<TourViolation>>(body["violations"].clone())
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
