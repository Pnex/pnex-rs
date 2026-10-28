//! Studio API (versioned 3D tours) — client of the `/api/v1/tours`
//! controller + public endpoint `/api/v1/public/tours` (contract
//! `docs/contracts/tours.http`). Types from `pnex-core` (shared source of
//! truth, wasm32); `update` carries optimistic concurrency
//! (`expected_version_number`, 409 if stale — same approach as `api/flows.rs`).

use pnex_core::{
    CreateTour, Paginated, PublicTour, PublishTour, TourDetail, TourSummary, TourVersionDetail,
    TourVersionSummary, TourViolation, UpdateTour,
};

use crate::api::client;
use crate::api::error::ApiError;

/// Filters of `GET /api/v1/tours` + pagination (D14).
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

/// `GET /api/v1/tours` — tours of the current org, paginated envelope.
pub async fn list(filters: &TourFilters) -> Result<Paginated<TourSummary>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/tours{}", filters.to_query()),
        None,
    )
    .await
}

/// `GET /api/v1/tours`, every page; the filters' own `limit`/`offset`
/// must be left unset.
pub async fn list_all(filters: &TourFilters) -> Result<Vec<TourSummary>, ApiError> {
    client::request_all(&format!("/api/v1/tours{}", filters.to_query())).await
}

/// `POST /api/v1/tours` — creation (minimal document = version 1).
pub async fn create(params: CreateTour) -> Result<TourDetail, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/tours",
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/tours/{id}` — detail (doc of the latest version).
pub async fn detail(id: &str) -> Result<TourDetail, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/tours/{}", urlencode(id)),
        None,
    )
    .await
}

/// `PATCH /api/v1/tours/{id}` — saves a **new version**
/// (append-only); 409 if `expected_version_number` is stale, 400
/// `{"violations": […]}` if the document is invalid.
pub async fn update(id: &str, params: UpdateTour) -> Result<TourDetail, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/tours/{}", urlencode(id)),
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/tours/{id}` — tour + versions (cascade; no blob).
pub async fn delete(id: &str) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/tours/{}", urlencode(id)),
        None,
    )
    .await
    .map(|_| ())
}

/// `GET /api/v1/tours/{id}/versions` — history, desc.
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

/// `GET /api/v1/tours/{id}/versions/{n}` — historical document.
pub async fn version(id: &str, version_number: i64) -> Result<TourVersionDetail, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/tours/{}/versions/{version_number}", urlencode(id)),
        None,
    )
    .await
}

/// `POST /api/v1/tours/{id}/publish` — publishes a version (absent =
/// latest). The published version serves the in-app viewer and the public link.
pub async fn publish(id: &str, version_number: Option<i64>) -> Result<TourDetail, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/tours/{}/publish", urlencode(id)),
        Some(serde_json::to_value(PublishTour { version_number }).unwrap_or_default()),
    )
    .await
}

/// `POST /api/v1/tours/{id}/unpublish` — unpublishes AND revokes the public link.
pub async fn unpublish(id: &str) -> Result<TourDetail, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/tours/{}/unpublish", urlencode(id)),
        Some(serde_json::json!({})),
    )
    .await
}

/// Response of `POST /share` — the front shows a toast, the fields are not
/// consumed (the token is served through the writer detail).
#[derive(Debug, Clone, serde::Deserialize)]
#[allow(dead_code)]
pub struct ShareResponse {
    pub share_token: String,
    pub share_enabled: bool,
}

/// `POST /api/v1/tours/{id}/share` — generates (or regenerates) the public token.
/// 409 `not_published` if the tour has no published version.
pub async fn share(id: &str) -> Result<ShareResponse, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/tours/{}/share", urlencode(id)),
        Some(serde_json::json!({})),
    )
    .await
}

/// `DELETE /api/v1/tours/{id}/share` — revokes the link (publication
/// kept).
pub async fn revoke_share(id: &str) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/tours/{}/share", urlencode(id)),
        None,
    )
    .await
    .map(|_| ())
}

// ─────────────────────────── Public (no auth) ───────────────────────────

/// `GET /api/v1/public/tours/{token}` — published document + asset map.
/// Path exempted from auth in `client.rs` (`PUBLIC_PREFIX`) — never any
/// Bearer/X-Org-Id attached.
pub async fn public_detail(token: &str) -> Result<PublicTour, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/public/tours/{}", urlencode(token)),
        None,
    )
    .await
}

/// Path of the bytes of an asset referenced by the published tour (`?v=n` —
/// exact version, cacheable; absent = current).
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

/// Full URL of a share link (the SPA's `/share/:token` page):
/// web = **page origin** (the API may live elsewhere in hot dev —
/// `PNEX_API_BASE_URL` — the link must target the SPA, not the API); native =
/// server URL (the SPA is served by the backend in production). The
/// dioxus-web router is **WebHistory**: real path, NO `#`.
pub fn share_url(token: &str) -> String {
    #[cfg(target_arch = "wasm32")]
    let base = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default();
    #[cfg(not(target_arch = "wasm32"))]
    let base = crate::api::config::api_base();
    format!("{base}/share/{}", urlencode(token))
}

// ─────────────────────────── Save failures ───────────────────────────

/// Display text of a tour or annotation document violation: `err-<code>`
/// with the offending id as `$subject` (prefixed when present), else the
/// canonical English message. Render scope required.
pub fn violation_text(subject: Option<&str>, code: &str, message: &str) -> String {
    let violation = pnex_core::FlowViolation::with_args(
        None,
        code,
        message,
        serde_json::json!({ "subject": subject.unwrap_or_default() }),
    );
    let text = crate::api::error_i18n::localize_violation(&violation);
    match subject {
        // The English fallback message already names the element.
        Some(s) if text != message => format!("{s} : {text}"),
        _ => text,
    }
}

/// Failure of a save, sorted for the UI (same approach as `api/flows.rs`):
/// concurrency conflict (409, "reload / overwrite" modal), invalid document
/// (400 violations, banner), or other (verbatim toast).
#[derive(Debug, Clone, PartialEq)]
pub enum SaveError {
    Conflict { description: String },
    Invalid(Vec<TourViolation>),
    Other(String),
}

/// Classifies an `ApiError` coming from `update()` — only a 409 or a 400
/// with a `violations` field has usable semantics.
pub fn classify_save_error(err: &ApiError) -> SaveError {
    if err.status == Some(409) {
        return SaveError::Conflict {
            description: err.message.clone(),
        };
    }
    if err.status == Some(400) {
        if let Some(v) = err.violations() {
            if let Ok(violations) = serde_json::from_value::<Vec<TourViolation>>(v.clone()) {
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
    fn save_failure_classification() {
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
