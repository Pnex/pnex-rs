//! Pagination des listes — enveloppe unique (décision D14) : toutes les
//! listes de l'API renvoient `{count, next, previous, results}` (forme
//! LimitOffsetPagination de DRF), pilotée par `limit` (défaut 20, max 100)
//! et `offset`. Le scaffold Django renvoyait des tableaux nus ; Rust
//! améliore le contrat (décision utilisateur 2026-08-16 : sans pagination
//! bornée, la base et les réponses souffriraient à l'échelle).

use serde::{Deserialize, Serialize};

/// Enveloppe d'une liste paginée.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paginated<T> {
    /// Total avant découpage (les filtres s'appliquent au compte).
    pub count: i64,
    /// Lien vers la page suivante — `null` sur la dernière page.
    pub next: Option<String>,
    /// Lien vers la page précédente — `null` sur la première page.
    pub previous: Option<String>,
    pub results: Vec<T>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Forme exacte sur le wire (noms DRF, next/previous null en bord de liste).
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
        assert_eq!(back, serde_json::from_str::<serde_json::Value>(json).unwrap());
    }
}
