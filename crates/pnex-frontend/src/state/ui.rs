//! Préférences UI globales — pour l'instant le repli de la sidebar desktop
//! en rail d'icônes. Signal global réactif + persistance `pnex.sidebar_rail`
//! (même école que `org.rs` : le stockage est la source de vérité, le signal
//! le miroir réactif).

use dioxus::prelude::*;

use crate::storage::{self, KeyValueStorage, KEY_SIDEBAR_RAIL};

/// Sidebar desktop repliée (rail d'icônes `w-16`) — défaut : dépliée.
pub static RAIL: GlobalSignal<bool> = GlobalSignal::new(|| false);

/// Number of mounted full-screen editors (`EditorShell`). While > 0 the
/// mobile app header is hidden: the editor bar carries its own back button.
/// A counter, not a bool: switching editors may mount the next one before
/// the previous one drops.
pub static EDITORS_OPEN: GlobalSignal<u32> = GlobalSignal::new(|| 0);

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

/// Floating menu currently open in an editor (palette, floors panel,
/// symbols / devices panels…): at most one at a time. `None` = all closed
/// (click outside, Escape). See [`use_exclusive_menu`].
pub static ACTIVE_MENU: GlobalSignal<Option<u64>> = GlobalSignal::new(|| None);

static NEXT_MENU_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Makes the menu driven by `open` exclusive: opening it closes any other
/// menu, and it closes itself when another one opens or when everything
/// is dismissed (`close_menus`). The menu keeps owning its `open` signal.
pub fn use_exclusive_menu(mut open: Signal<bool>) {
    let id = use_hook(|| NEXT_MENU_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    // Opened → claim the slot (the other menus see it and close).
    use_effect(move || {
        if open() && *ACTIVE_MENU.peek() != Some(id) {
            *ACTIVE_MENU.write() = Some(id);
        }
    });
    // Slot taken by another menu, or cleared → close.
    use_effect(move || {
        let active = ACTIVE_MENU();
        if active != Some(id) && *open.peek() {
            open.set(false);
        }
    });
}

/// Closes every editor menu (click outside a menu, Escape).
pub fn close_menus() {
    if ACTIVE_MENU.peek().is_some() {
        *ACTIVE_MENU.write() = None;
    }
}
