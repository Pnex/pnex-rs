//! Global UI preferences — for now the collapse of the desktop sidebar
//! into an icon rail. Reactive global signal + `pnex.sidebar_rail` persistence
//! (same style as `org.rs`: storage is the source of truth, the signal
//! the reactive mirror).

use dioxus::prelude::*;

use crate::storage::{self, KeyValueStorage, KEY_SIDEBAR_RAIL};

/// Collapsed desktop sidebar (icon rail `w-16`) — default: expanded.
pub static RAIL: GlobalSignal<bool> = GlobalSignal::new(|| false);

/// API requests in flight: drives the loading mark next to the logo.
pub static IN_FLIGHT: GlobalSignal<u32> = GlobalSignal::new(|| 0);

/// Counts one request for its lifetime. Outside the Dioxus runtime (a
/// detached task) it counts nothing: writing a global signal there panics.
pub struct InFlight(bool);

impl InFlight {
    pub fn start() -> Self {
        let live = dioxus::dioxus_core::Runtime::try_current().is_some();
        if live {
            *IN_FLIGHT.write() += 1;
        }
        Self(live)
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if self.0 && dioxus::dioxus_core::Runtime::try_current().is_some() {
            let mut n = IN_FLIGHT.write();
            *n = n.saturating_sub(1);
        }
    }
}

/// Number of mounted full-screen editors (`EditorShell`). While > 0 the
/// mobile app header is hidden: the editor bar carries its own back button.
/// A counter, not a bool: switching editors may mount the next one before
/// the previous one drops.
pub static EDITORS_OPEN: GlobalSignal<u32> = GlobalSignal::new(|| 0);

/// Collapses the sidebar (rail) and persists the preference.
pub fn set(rail: bool) {
    RAIL.with_mut(|v| *v = rail);
    storage::local().set(KEY_SIDEBAR_RAIL, if rail { "true" } else { "false" });
}

/// Toggles rail ⇄ expanded (sidebar footer button).
pub fn toggle() {
    // The `read()` guard is a temporary living until the end of the
    // statement: stacking it with `set()` would re-enter `with_mut` on a
    // still-borrowed signal (AlreadyBorrowed). We drop it first.
    let next = !*RAIL.read();
    set(next);
}

/// Restores the preference at boot — to call on shell mount (once,
/// fully idempotent: re-reading the storage gives the same result).
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
