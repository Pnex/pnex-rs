//! List pagination — single envelope (decision D14): every API list
//! returns `{count, next, previous, results}` (LimitOffsetPagination
//! shape), driven by `limit` (default 20, max 100) and `offset`. Bounded
//! pagination keeps the database and the responses sane at scale (user
//! decision 2026-08-16).

use serde::{Deserialize, Serialize};

/// Envelope of a paginated list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Paginated<T> {
    /// Total before slicing (filters apply to the count).
    pub count: i64,
    /// Link to the next page — `null` on the last page.
    pub next: Option<String>,
    /// Link to the previous page — `null` on the first page.
    pub previous: Option<String>,
    /// Items of this page.
    pub results: Vec<T>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exact wire shape (null next/previous at list edges).
    #[test]
    fn paginated_shape_roundtrip() {
        let json = r#"{
            "count": 42,
            "next": "/api/v1/devices?limit=20&offset=20",
            "previous": null,
            "results": [{ "id": 7 }]
        }"#;
        let page: Paginated<serde_json::Value> = serde_json::from_str(json).unwrap();
        assert_eq!(page.count, 42);
        assert_eq!(page.results.len(), 1);
        assert!(page.previous.is_none());
        let back = serde_json::to_value(&page).unwrap();
        assert_eq!(
            back,
            serde_json::from_str::<serde_json::Value>(json).unwrap()
        );
    }
}
