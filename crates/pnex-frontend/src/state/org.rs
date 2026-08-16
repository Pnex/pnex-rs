//! Org courante (le tenant actif) — signal global réactif pour l'UI +
//! persistance `pnex.org` lue par le client HTTP pour `X-Org-Id`.
//!
//! Le stockage est la source de vérité du header ; le signal est le miroir
//! réactif (écrit par le sélecteur d'org et la page Organisations, qui
//! persistent en même temps).

use dioxus::prelude::*;

use crate::storage::{self, KeyValueStorage, KEY_ORG};

pub static ORG: GlobalSignal<Option<i64>> = GlobalSignal::new(|| None);

/// Org courante si définie.
pub fn current() -> Option<i64> {
    *ORG.read()
}

/// Sélectionne l'org (et la persiste).
pub fn set(id: i64) {
    // Méthode intrinsèque de Global (&self) — les static ne prêtent pas à
    // &mut (les setters du trait Writable exigent &mut).
    ORG.with_mut(|v| *v = Some(id));
    storage::local().set(KEY_ORG, &id.to_string());
}

/// Restaure l'org au boot : la dernière sélectionnée si l'utilisateur en est
/// toujours membre, sinon sa première org (l'org personnelle JIT).
pub fn restore(memberships: &[pnex_core::OrgMembership]) {
    let stored = storage::local()
        .get(KEY_ORG)
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|id| memberships.iter().any(|m| m.id == *id));
    let chosen = stored.or_else(|| memberships.first().map(|m| m.id));
    match chosen {
        Some(id) => set(id),
        None => clear(),
    }
}

/// Désélectionne l'org courante.
pub fn clear() {
    ORG.with_mut(|v| *v = None);
    storage::local().remove(KEY_ORG);
}
