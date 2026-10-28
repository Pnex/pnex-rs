//! Deep-links and edit mode of the viz bases (D32/D34, `state/flows.rs`
//! school): the `/dashboards` route carries **static** query params
//! (`?id&:mode`) that hydrate these signals on mount — route props
//! are not signals (dioxus #2784).

use dioxus::prelude::*;

/// Dashboard to open directly in the page (editor or live).
pub static OPEN_DASHBOARD: GlobalSignal<Option<String>> = GlobalSignal::new(|| None);

/// Requested edit mode (`?mode=edit`). The page validates it: a viewer
/// falls back to live (D34), the API would refuse anyway (403).
pub static VIZ_EDIT: GlobalSignal<bool> = GlobalSignal::new(|| false);

/// Sets both signals at once (on entering the page).
pub fn open_dashboard(id: Option<String>, edit: bool) {
    // GlobalSignal: no `&mut` static — mutation via with_mut
    // (intrinsic method, project memory).
    OPEN_DASHBOARD.with_mut(|v| *v = id);
    VIZ_EDIT.with_mut(|v| *v = edit);
}
