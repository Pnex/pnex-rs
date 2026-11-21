//! Roles of the current org's types and link types (external-apps.md D441): the
//! UI names a shared concept (`roles::PERSON`) and asks which type carries it,
//! never a pack's key. Loaded per org by [`use_roles`], again after a pack
//! install ([`reload`]).

use std::collections::BTreeMap;

use dioxus::prelude::*;

use crate::state::org;

#[derive(Debug, Clone, Default, PartialEq)]
struct Roles {
    org_id: i64,
    /// Role → type key.
    types: BTreeMap<String, String>,
    /// Role → link type key.
    links: BTreeMap<String, String>,
}

static ROLES: GlobalSignal<Option<Roles>> = GlobalSignal::new(|| None);
static RELOAD: GlobalSignal<u32> = GlobalSignal::new(|| 0);

fn loaded() -> Option<Roles> {
    // Outside a running app (unit tests) nothing is loaded.
    dioxus::dioxus_core::Runtime::try_current()?;
    let org_id = org::current()?;
    ROLES.read().clone().filter(|r| r.org_id == org_id)
}

/// Key of the type carrying `role`; empty when no type of the org does.
// shortcut: before the roles arrive, the role's own word stands for the key (the
// shipped packs use it); a type renamed by its pack shows right after loading.
pub fn type_key(role: &str) -> String {
    match loaded() {
        Some(r) => r.types.get(role).cloned().unwrap_or_default(),
        None => role.to_string(),
    }
}

/// Key of the link type carrying `role`; empty when none does.
pub fn link_key(role: &str) -> String {
    match loaded() {
        Some(r) => r.links.get(role).cloned().unwrap_or_default(),
        None => role.to_string(),
    }
}

/// Keys of the link types carrying `roles`; a role no link type carries is left out.
pub fn link_keys(roles: &[&str]) -> Vec<String> {
    roles
        .iter()
        .map(|r| link_key(r))
        .filter(|k| !k.is_empty())
        .collect()
}

/// Is `type_key` the type carrying `role`?
pub fn is(type_key: &str, role: &str) -> bool {
    !type_key.is_empty() && type_key == self::type_key(role)
}

/// Reads the roles again (after a pack install).
pub fn reload() {
    *RELOAD.write() += 1;
}

/// Loads the roles of the current org, again on every org change. Mounted once
/// by the shell.
pub fn use_roles() {
    use_resource(move || {
        let org_id = org::current();
        let _ = RELOAD();
        async move {
            let Some(org_id) = org_id else {
                *ROLES.write() = None;
                return;
            };
            let (Ok(types), Ok(links)) = (
                crate::api::ontology::types().await,
                crate::api::ontology::link_types().await,
            ) else {
                return;
            };
            *ROLES.write() = Some(Roles {
                org_id,
                types: types
                    .into_iter()
                    .filter_map(|t| Some((t.def.role?, t.def.key)))
                    .collect(),
                links: links
                    .into_iter()
                    .filter_map(|l| Some((l.def.role?, l.def.key)))
                    .collect(),
            });
        }
    });
}
