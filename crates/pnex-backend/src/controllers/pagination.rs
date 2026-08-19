//! Pagination + recherche des listes — décision D14 (docs/inventory.md) :
//! **toutes** les listes de l'API renvoient l'enveloppe `{count, next,
//! previous, results}` (forme LimitOffsetPagination de DRF) et acceptent
//! un paramètre `search` multi-champs. Loco n'a pas de mécanisme intégré
//! équivalent aux `DjangoFilterBackend` : l'idiome est l'extracteur
//! `Query<T>` d'Axum pour les filtres + le paginateur SeaORM (`count()` /
//! `.offset().limit()`) côté SQL quand les filtres y remontent ; sinon
//! filtre Rust puis découpage.
//!
//! Paramètres : `limit` (défaut : var d'env `PAGINATION_DEFAULT_LIMIT`,
//! à 10 si absente — **une seule var pour toutes les listes** ; max 100),
//! `offset` (≥ 0) et `search` (OU insensible à la casse sur les champs
//! texte pertinents de chaque liste). Valeurs invalides ou hors bornes →
//! défauts silencieux, jamais d'erreur 400.

use serde::Serialize;
use std::collections::HashMap;

pub const MAX_LIMIT: i64 = 100;
/// Var d'env unique pilotant la limite par défaut de toutes les listes.
pub const DEFAULT_LIMIT_ENV: &str = "PAGINATION_DEFAULT_LIMIT";
pub const DEFAULT_LIMIT_FALLBACK: i64 = 10;

/// Limite par défaut partagée par toutes les listes — lue à chaque appel
/// (testable, réactive au changement d'env du process).
pub fn default_limit() -> i64 {
    std::env::var(DEFAULT_LIMIT_ENV)
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|l| *l >= 1)
        .unwrap_or(DEFAULT_LIMIT_FALLBACK)
        .min(MAX_LIMIT)
}

/// Page demandée, après clamp.
#[derive(Debug, Clone, Copy)]
pub struct PageParams {
    pub limit: i64,
    pub offset: i64,
}

impl PageParams {
    /// Depuis des paramètres bruts (`Option<String>` des extracteurs) :
    /// non numérique / `< 1` → défaut ; `> MAX_LIMIT` → clamp ;
    /// offset négatif → 0.
    pub fn from(limit: Option<&str>, offset: Option<&str>) -> Self {
        let limit = limit
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|l| *l >= 1)
            .unwrap_or_else(default_limit)
            .min(MAX_LIMIT);
        let offset = offset
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|o| *o >= 0)
            .unwrap_or(0);
        Self { limit, offset }
    }

    /// Depuis une raw query map multi-valeurs (cf. `raw_query_map`).
    pub fn from_map(map: &HashMap<String, Vec<String>>) -> Self {
        let first = |k: &str| map.get(k).and_then(|v| v.first().map(String::as_str));
        Self::from(first("limit"), first("offset"))
    }

    /// Offset de la page suivante — `None` sur la dernière page.
    pub fn next_offset(&self, count: i64) -> Option<i64> {
        (self.offset + self.limit < count).then_some(self.offset + self.limit)
    }

    /// Offset de la page précédente — `None` sur la première page.
    pub fn previous_offset(&self) -> Option<i64> {
        (self.offset > 0).then_some((self.offset - self.limit).max(0))
    }

    /// Bornes `skip`/`take` pour un découpage Rust d'un Vec déjà filtré.
    pub fn slice(&self, len: usize) -> (usize, usize) {
        let skip = (self.offset as usize).min(len);
        let take = (self.limit as usize).min(len - skip);
        (skip, take)
    }
}

/// Recherche multi-champs côté Rust : le terme (minuscules) doit apparaître
/// dans l'une des valeurs fournies (OU insensible à la casse).
pub fn rust_search_match(search: &Option<String>, haystacks: &[&str]) -> bool {
    match search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(term) => {
            let term = term.to_lowercase();
            haystacks.iter().any(|h| h.to_lowercase().contains(&term))
        }
        None => true,
    }
}

/// Enveloppe paginée avec liens `next`/`previous` relatifs (chemin + filtres
/// conservés). `filters` : paires clé/valeur actives, hors limit/offset.
pub fn envelope<T: Serialize>(
    path: &str,
    filters: &[(String, String)],
    page: PageParams,
    count: i64,
    results: Vec<T>,
) -> serde_json::Value {
    let link = |offset: i64| {
        let mut pairs: Vec<String> = filters
            .iter()
            .map(|(k, v)| format!("{k}={}", urlencode(v)))
            .collect();
        pairs.push(format!("limit={}", page.limit));
        pairs.push(format!("offset={offset}"));
        format!("{path}?{}", pairs.join("&"))
    };
    serde_json::json!({
        "count": count,
        "next": page.next_offset(count).map(&link),
        "previous": page.previous_offset().map(link),
        "results": results,
    })
}

/// Percent-encoding minimal pour les valeurs de filtres dans les liens.
fn urlencode(value: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_et_clamp() {
        // Défaut piloté par l'env (absente ici) → fallback 10, partout.
        assert_eq!(PageParams::from(None, None).limit, default_limit());
        assert_eq!(default_limit(), DEFAULT_LIMIT_FALLBACK);
        // limit invalide/négative → défaut ; > 100 → clamp.
        assert_eq!(PageParams::from(Some("0"), None).limit, default_limit());
        assert_eq!(PageParams::from(Some("-3"), None).limit, default_limit());
        assert_eq!(PageParams::from(Some("abc"), None).limit, default_limit());
        assert_eq!(PageParams::from(Some("500"), None).limit, MAX_LIMIT);
        assert_eq!(PageParams::from(Some("2"), None).limit, 2);
        // offset négatif/invalide → 0.
        assert_eq!(PageParams::from(None, Some("-5")).offset, 0);
        assert_eq!(PageParams::from(None, Some("zz")).offset, 0);
        assert_eq!(PageParams::from(None, Some("42")).offset, 42);
    }

    #[test]
    fn recherche_rust_multichamps() {
        let search = Some("RELAY".to_string());
        assert!(rust_search_match(&search, &["4_chan_relay", "actuator"]));
        // Insensible à la casse.
        assert!(rust_search_match(&Some("CHAN".into()), &["4_chan_relay"]));
        assert!(!rust_search_match(&Some("zzz".into()), &["4_chan_relay"]));
        // Absent/vide → pas de filtre.
        assert!(rust_search_match(&None, &[]));
        assert!(rust_search_match(
            &Some("   ".into()),
            &["quoi que ce soit"]
        ));
    }

    #[test]
    fn offsets_de_navigation() {
        let page = PageParams {
            limit: 20,
            offset: 0,
        };
        assert_eq!(page.next_offset(40), Some(20));
        assert_eq!(page.previous_offset(), None);

        let page = PageParams {
            limit: 20,
            offset: 20,
        };
        assert_eq!(
            page.next_offset(40),
            None,
            "40 items = dernière page en 2×20"
        );
        assert_eq!(page.previous_offset(), Some(0));

        let page = PageParams {
            limit: 20,
            offset: 40,
        };
        assert_eq!(page.next_offset(41), None, "dernière page incomplète");
        assert_eq!(page.previous_offset(), Some(20));

        // offset « au milieu du vide » : previous ramène sur une page réelle.
        let page = PageParams {
            limit: 20,
            offset: 55,
        };
        assert_eq!(page.previous_offset(), Some(35));
    }

    #[test]
    fn slice_ne_deborde_pas() {
        let page = PageParams {
            limit: 20,
            offset: 30,
        };
        assert_eq!(page.slice(35), (30, 5), "take borné par la fin du Vec");
        let page = PageParams {
            limit: 20,
            offset: 50,
        };
        assert_eq!(page.slice(35), (35, 0), "offset au-delà → vide");
    }

    #[test]
    fn enveloppe_conserve_les_filtres() {
        let page = PageParams {
            limit: 2,
            offset: 0,
        };
        let body = envelope(
            "/api/v1/devices",
            &[("device_type".into(), "sen sor".into())],
            page,
            5,
            vec!["a"],
        );
        assert_eq!(body["count"], 5);
        assert_eq!(
            body["next"].as_str().unwrap(),
            "/api/v1/devices?device_type=sen%20sor&limit=2&offset=2"
        );
        assert!(body["previous"].is_null());
        assert_eq!(body["results"], serde_json::json!(["a"]));
    }
}
