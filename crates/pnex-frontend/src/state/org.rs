//! Current org (the active tenant) — reactive global signal for the UI +
//! `pnex.org` persistence read by the HTTP client for `X-Org-Id`.
//!
//! The storage is the source of truth of the header; the signal is the reactive
//! mirror (written by the org selector and the Organisations page, which
//! persist at the same time).

use dioxus::prelude::*;

use crate::storage::{self, KeyValueStorage, KEY_ORG};

pub static ORG: GlobalSignal<Option<i64>> = GlobalSignal::new(|| None);

/// Current org if set.
pub fn current() -> Option<i64> {
    *ORG.read()
}

/// Selects the org (and persists it).
pub fn set(id: i64) {
    // Intrinsic method of Global (&self) — statics do not lend themselves to
    // &mut (the setters of the Writable trait require &mut).
    ORG.with_mut(|v| *v = Some(id));
    storage::local().set(KEY_ORG, &id.to_string());
}

/// Restores the org at boot: the last selected one if the user is
/// still a member, otherwise their first org where they are not a viewer (the
/// JIT personal org in practice — the backend lists orgs by increasing id).
/// A viewer can neither create nor see much: avoiding these orgs on
/// fallback prevents landing on an empty shell (observation O1).
pub fn restore(memberships: &[pnex_core::OrgMembership]) {
    let stored = storage::local()
        .get(KEY_ORG)
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|id| memberships.iter().any(|m| m.id == *id));
    let chosen = stored.or_else(|| {
        memberships
            .iter()
            .find(|m| m.role != pnex_core::OrgRole::Viewer)
            .map(|m| m.id)
            .or_else(|| memberships.first().map(|m| m.id))
    });
    match chosen {
        Some(id) => set(id),
        None => clear(),
    }
}

/// Deselects the current org.
pub fn clear() {
    ORG.with_mut(|v| *v = None);
    storage::local().remove(KEY_ORG);
}
