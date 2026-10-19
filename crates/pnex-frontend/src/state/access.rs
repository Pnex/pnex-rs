//! Rights of the signed-in user in the current org (iam.md §9): the one
//! place the UI asks "may I do this?". The UI hides what the server would
//! refuse; the server still decides on every call (R2).
//!
//! Permissions are the `service:verb` of the shared action registry
//! (`pnex_api_contract::iam`). The server's answer
//! (`GET /iam/me/permissions`, assigned roles included) is kept per org by
//! [`use_permissions`]; until it arrives, the managed role of the
//! membership decides from the registry.

use std::collections::HashSet;

use dioxus::prelude::*;
use pnex_api_contract::iam::{permission_level, Level};
use pnex_core::OrgRole;

use crate::state::{org, session};

/// Permissions the server granted, for one org.
static GRANTED: GlobalSignal<Option<(i64, HashSet<String>)>> = GlobalSignal::new(|| None);

fn level_of(role: OrgRole) -> Level {
    match role {
        OrgRole::Owner => Level::Owner,
        OrgRole::Admin => Level::Admin,
        OrgRole::Member => Level::Member,
        OrgRole::Viewer => Level::Viewer,
    }
}

/// Does this managed role hold `permission`? Unknown permission = no.
pub fn role_can(role: OrgRole, permission: &str) -> bool {
    permission_level(permission).is_some_and(|need| level_of(role) >= need)
}

/// Role of the signed-in user in the current org.
pub fn current_role() -> Option<OrgRole> {
    let user = session::user()?;
    let org_id = org::current()?;
    user.orgs.iter().find(|m| m.id == org_id).map(|m| m.role)
}

/// May the signed-in user use `permission` in the current org?
pub fn can(permission: &str) -> bool {
    let org_id = org::current();
    if let Some((granted_org, set)) = GRANTED.read().as_ref() {
        if Some(*granted_org) == org_id {
            return set.contains(permission);
        }
    }
    current_role().is_some_and(|r| role_can(r, permission))
}

/// Loads the server's answer for the current org, again on every org
/// change. Mounted once by the shell.
pub fn use_permissions() {
    use_resource(move || {
        // Read in the synchronous part: the org is the resource's dependency.
        let org_id = org::current();
        async move {
            let Some(org_id) = org_id else {
                GRANTED.with_mut(|g| *g = None);
                return;
            };
            // A failed call keeps the role fallback: the server decides anyway.
            if let Ok(me) = crate::api::iam::my_permissions().await {
                let set = me.permissions.into_iter().collect();
                GRANTED.with_mut(|g| *g = Some((org_id, set)));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_roles_follow_the_registry() {
        assert!(role_can(OrgRole::Viewer, "flows:read"));
        assert!(!role_can(OrgRole::Viewer, "flows:write"));
        assert!(role_can(OrgRole::Member, "flows:deploy"));
        assert!(!role_can(OrgRole::Member, "secrets:write"));
        assert!(role_can(OrgRole::Admin, "secrets:write"));
        assert!(!role_can(OrgRole::Admin, "orgs:delete"));
        assert!(role_can(OrgRole::Owner, "orgs:delete"));
        assert!(!role_can(OrgRole::Owner, "flows:fly"));
    }
}
