//! Global search client (D69) — mirror of `controllers/global_search.rs`:
//! grouped typeahead across the org's objects, shape
//! `{count, q, groups:[{entity_type, results}]}` (deliberate D14 deviation).

use crate::api::{client, error::ApiError, media::urlencode};

/// Query params of `GET /api/v1/search` — `q` is trimmed server-side,
/// `limit` is the per-group cap (server clamps to 20).
#[derive(Default)]
pub struct SearchFilters {
    pub q: String,
    pub limit: Option<i64>,
}

impl SearchFilters {
    fn to_query(&self) -> String {
        let trimmed = self.q.trim();
        let mut parts: Vec<String> = Vec::new();
        if !trimmed.is_empty() {
            parts.push(format!("q={}", urlencode(trimmed)));
        }
        if let Some(v) = self.limit {
            parts.push(format!("limit={v}"));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("?{}", parts.join("&"))
        }
    }
}

/// `GET /api/v1/search` — typeahead across the org's objects.
pub async fn global(filters: &SearchFilters) -> Result<SearchResponse, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/search{}", filters.to_query()),
        None,
    )
    .await
}

/// One result row, uniform across entity types; `id` is a String on the
/// wire (i64 PKs stringified, UUIDs as-is).
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct SearchHit {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub updated_at: String,
}

/// Results grouped by entity type (empty groups omitted server-side).
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct SearchGroup {
    pub entity_type: String,
    pub results: Vec<SearchHit>,
}

/// Typeahead response.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct SearchResponse {
    pub count: i64,
    pub q: String,
    pub groups: Vec<SearchGroup>,
}
