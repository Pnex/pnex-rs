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
            count: count,
            page_size: page_size,
            page: page,
            on_navigate: move |new_page| page.set(new_page),
        }
    }
}
