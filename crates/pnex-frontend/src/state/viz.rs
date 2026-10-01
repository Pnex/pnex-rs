//! Deep-links et mode d'édition des bases viz (D32/D34, école
//! `state/flows.rs`) : la route `/dashboards` porte des query params
//! **statiques** (`?id&:mode`) qui hydratent ces signaux au mount — les
//! props de route ne sont pas des signaux (dioxus #2784).

use dioxus::prelude::*;

/// Dashboard à ouvrir directement dans la page (éditeur ou live).
pub static OPEN_DASHBOARD: GlobalSignal<Option<String>> = GlobalSignal::new(|| None);

/// Mode édition demandé (`?mode=edit`). La page le valide : un viewer
/// retombe en live (D34), l'API refuserait de toute façon (403).
pub static VIZ_EDIT: GlobalSignal<bool> = GlobalSignal::new(|| false);

/// Pose les deux signaux d'un coup (à l'entrée dans la page).
pub fn open_dashboard(id: Option<String>, edit: bool) {
    // GlobalSignal : pas de `&mut` static — mutation via with_mut
    // (méthode intrinsèque, mémoire projet).
    OPEN_DASHBOARD.with_mut(|v| *v = id);
    VIZ_EDIT.with_mut(|v| *v = edit);
}

/// Sortie de page : évite qu'un prochain mount hérite de l'état précédent
/// (consommé par les futures navs croisées viz, école `state/flows.rs`).
#[allow(dead_code)]
pub fn clear_dashboard() {
    OPEN_DASHBOARD.with_mut(|v| *v = None);
    VIZ_EDIT.with_mut(|v| *v = false);
}
