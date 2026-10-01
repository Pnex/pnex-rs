//! Préférences UI globales — pour l'instant le repli de la sidebar desktop
//! en rail d'icônes. Signal global réactif + persistance `pnex.sidebar_rail`
//! (même école que `org.rs` : le stockage est la source de vérité, le signal
//! le miroir réactif).

use dioxus::prelude::*;

use crate::storage::{self, KeyValueStorage, KEY_SIDEBAR_RAIL};

/// Sidebar desktop repliée (rail d'icônes `w-16`) — défaut : dépliée.
pub static RAIL: GlobalSignal<bool> = GlobalSignal::new(|| false);

/// Replie la sidebar (rail) et persiste la préférence.
pub fn set(rail: bool) {
    RAIL.with_mut(|v| *v = rail);
    storage::local().set(KEY_SIDEBAR_RAIL, if rail { "true" } else { "false" });
}

/// Bascule rail ⇄ dépliée (bouton du pied de sidebar).
pub fn toggle() {
    // La garde du `read()` est un temporaire vivant jusqu'à la fin de
    // l'instruction : l'empiler avec `set()` re-entrerait `with_mut` sur un
    // signal encore emprunté (AlreadyBorrowed). On la rend d'abord.
    let next = !*RAIL.read();
    set(next);
}

/// Restaure la préférence au boot — à appeler au montage du shell (une fois,
/// l'idempotence est totale : relire le storage donne le même résultat).
pub fn restore() {
    let rail = storage::local().get(KEY_SIDEBAR_RAIL).as_deref() == Some("true");
    RAIL.with_mut(|v| *v = rail);
}
