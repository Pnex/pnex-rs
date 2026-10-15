//! Pagination canonique des listes — délègue au `Pager` existant
//! (`components/pager.rs`, enveloppe `{count, …}` D14) en standardisant le
//! câblage et la taille de page : le composant pose `page.set(nouvel_index)`
//! tout seul (la resource re-fetch car elle lit `page()` en partie
//! synchrone — doctrine), une seule taille de page (D14), et se pose dans
//! le corps de liste (children de `ListStates`), jamais dans `DataTable`.

use dioxus::prelude::*;

use crate::components::pager::Pager;

/// Taille de page par défaut des listes (D14 : `PAGINATION_DEFAULT_LIMIT`).
pub const PAGE_SIZE: i64 = 10;

#[component]
pub fn ListPager(
    /// Total renvoyé par l'enveloppe `{count, …}`.
    count: i64,
    /// Signal de la page courante (0-based).
    mut page: Signal<i64>,
    /// Taille de page — défaut D14 (10). Surcharge possible pour les
    /// grilles de cartes plus denses (catalog : 12).
    #[props(default = PAGE_SIZE)]
    page_size: i64,
) -> Element {
    rsx! {
        Pager {
            count,
            page_size,
            page,
            on_navigate: move |new_page| page.set(new_page),
        }
    }
}

/// Rows of page `page` (0-based) out of a fully loaded list.
// shortcut: client-side paging over `request_all`; move to server offsets if a list grows past a few hundred rows.
pub fn page_slice<T: Clone>(all: &[T], page: i64) -> Vec<T> {
    all.iter()
        .skip((page.max(0) * PAGE_SIZE) as usize)
        .take(PAGE_SIZE as usize)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_slice_bounds() {
        let all: Vec<i64> = (0..23).collect();
        assert_eq!(page_slice(&all, 0), (0..10).collect::<Vec<_>>());
        assert_eq!(page_slice(&all, 2), vec![20, 21, 22]);
        assert!(page_slice(&all, 5).is_empty());
    }
}
