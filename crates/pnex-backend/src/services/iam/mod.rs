//! Authorization engine (iam.md D285–D293).
//!
//! `decide(grants, permission) → Allow | Deny(reason)`, deny by default;
//! explicit deny > allow > implicit deny. The platform and org boundaries
//! come before the engine: the org is the caller's (R1), the engine only
//! restricts inside it.
//!
//! A member's [`Grants`] are the managed role of their membership (D117,
//! built from the action registry `pnex_api_contract::iam`) plus the roles
//! assigned to them directly or through a team ([`store`], lot I3). The
//! `OrgContext` extractor decides the permission of every operation before
//! its handler; [`observe`] compares the decision with the handler's answer.

pub mod audit;
pub mod observe;
pub mod service;
pub mod store;

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

use axum::extract::MatchedPath;
use axum::http::Method;
use pnex_api_contract::iam::{Access, Level, OPERATIONS, PERMISSIONS};
use serde::{Deserialize, Serialize};

use crate::models::_entities::sea_orm_active_enums::OrgMemberRole;

/// Service whose permissions only managed roles grant or take away (D293):
/// a custom role never manages access, nor locks an owner out of it.
pub const IAM_SERVICE: &str = "iam";

/// Effect of a statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    Allow,
    Deny,
}

/// One statement of a role document (D287).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Statement {
    pub effect: Effect,
    /// Permission patterns: a permission (`flows:deploy`), a service
    /// wildcard (`flows:*`) or `*`.
    pub permissions: Vec<String>,
}

/// A role: a named list of statements.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Role {
    pub key: String,
    pub statements: Vec<Statement>,
    /// Managed by the platform (D117), as opposed to an org's custom role.
    pub managed: bool,
}

/// Why a request is refused (shown by the access checker, logged in audit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DenyReason {
    /// No statement allows the permission.
    Implicit,
    /// A deny statement of this role matched.
    Explicit { role: String },
    /// The permission is not in the registry (a bug: deny by default).
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Allowed by a statement of this role.
    Allow {
        role: String,
    },
    Deny(DenyReason),
}

impl Decision {
    pub fn allowed(&self) -> bool {
        matches!(self, Decision::Allow { .. })
    }
}

pub(crate) fn matches(pattern: &str, permission: &str) -> bool {
    pattern == "*"
        || pattern == permission
        || pattern
            .strip_suffix(":*")
            .is_some_and(|svc| permission.split_once(':').is_some_and(|(s, _)| s == svc))
}

fn service_of(permission: &str) -> &str {
    permission.split_once(':').map_or(permission, |(s, _)| s)
}

/// Evaluates `permission` against the roles a principal holds.
pub fn evaluate<'a>(
    roles: impl IntoIterator<Item = &'a Role> + Clone,
    permission: &str,
) -> Decision {
    if pnex_api_contract::iam::permission_level(permission).is_none() {
        return Decision::Deny(DenyReason::Unknown);
    }
    let iam = service_of(permission) == IAM_SERVICE;
    let hit = |effect: Effect| {
        roles.clone().into_iter().find(|r| {
            (r.managed || !iam)
                && r.statements.iter().any(|s| {
                    s.effect == effect && s.permissions.iter().any(|p| matches(p, permission))
                })
        })
    };
    if let Some(r) = hit(Effect::Deny) {
        return Decision::Deny(DenyReason::Explicit {
            role: r.key.clone(),
        });
    }
    match hit(Effect::Allow) {
        Some(r) => Decision::Allow {
            role: r.key.clone(),
        },
        None => Decision::Deny(DenyReason::Implicit),
    }
}

/// Level of a managed role in the D117 order.
pub fn level_of(role: &OrgMemberRole) -> Level {
    match role {
        OrgMemberRole::Owner => Level::Owner,
        OrgMemberRole::Admin => Level::Admin,
        OrgMemberRole::Member => Level::Member,
        OrgMemberRole::Viewer => Level::Viewer,
    }
}

/// The four managed roles (D287): each one allows every permission whose
/// registry level it reaches. Not editable, versioned with the platform.
pub fn managed_roles() -> &'static [Role; 4] {
    static ROLES: OnceLock<[Role; 4]> = OnceLock::new();
    ROLES.get_or_init(|| {
        let role = |key: &str, level: Level| Role {
            key: key.to_string(),
            statements: vec![Statement {
                effect: Effect::Allow,
                permissions: PERMISSIONS
                    .iter()
                    .filter(|(_, l)| *l <= level)
                    .map(|(p, _)| p.to_string())
                    .collect(),
            }],
            managed: true,
        };
        [
            role("viewer", Level::Viewer),
            role("member", Level::Member),
            role("admin", Level::Admin),
            role("owner", Level::Owner),
        ]
    })
}

/// The managed role document of an org membership role.
pub fn managed_role(role: &OrgMemberRole) -> &'static Role {
    &managed_roles()[level_of(role) as usize]
}

/// Everything a member holds in an org: the managed role of the membership
/// first, then the assigned roles.
#[derive(Clone, Debug)]
pub struct Grants(pub Vec<Cow<'static, Role>>);

impl Grants {
    /// The managed role of a membership, without assignments.
    pub fn managed(role: &OrgMemberRole) -> Self {
        Self(vec![Cow::Borrowed(managed_role(role))])
    }

    pub fn decide(&self, permission: &str) -> Decision {
        evaluate(self.0.iter().map(|r| r.as_ref()), permission)
    }

    pub fn can(&self, permission: &str) -> bool {
        self.decide(permission).allowed()
    }

    /// Every permission of the registry these grants allow, sorted.
    pub fn permissions(&self) -> Vec<&'static str> {
        PERMISSIONS
            .iter()
            .map(|(p, _)| *p)
            .filter(|p| self.can(p))
            .collect()
    }
}

/// `(METHOD, route template)` → registry entry, built once from the spec.
fn routes() -> &'static HashMap<(String, String), (&'static str, Access)> {
    static ROUTES: OnceLock<HashMap<(String, String), (&'static str, Access)>> = OnceLock::new();
    ROUTES.get_or_init(|| {
        let spec = serde_json::to_value(crate::openapi::spec()).unwrap_or_default();
        let mut map = HashMap::new();
        for (path, item) in spec["paths"].as_object().into_iter().flatten() {
            for (method, op) in item.as_object().into_iter().flatten() {
                let Some(id) = op["operationId"].as_str() else {
                    continue;
                };
                if let Some((id, access)) = OPERATIONS.iter().find(|(o, _)| *o == id) {
                    map.insert((method.to_uppercase(), path.clone()), (*id, *access));
                }
            }
        }
        map
    })
}

/// Registry entry of a request: its operationId and who may call it.
/// `None` off the spec (WebSockets, `/internal/*`, unknown routes).
pub fn operation_of(method: &Method, path: Option<&MatchedPath>) -> Option<(&'static str, Access)> {
    routes()
        .get(&(method.to_string(), path?.as_str().to_string()))
        .copied()
}

/// Decision for a managed role alone (no assignment).
pub fn authorize(role: &OrgMemberRole, permission: &str) -> Decision {
    Grants::managed(role).decide(permission)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(key: &str, effect: Effect, permissions: &[&str]) -> Role {
        Role {
            key: key.to_string(),
            statements: vec![Statement {
                effect,
                permissions: permissions.iter().map(|p| p.to_string()).collect(),
            }],
            managed: false,
        }
    }

    #[test]
    fn managed_roles_translate_the_registry_levels() {
        for (p, level) in PERMISSIONS {
            for role in [
                OrgMemberRole::Viewer,
                OrgMemberRole::Member,
                OrgMemberRole::Admin,
                OrgMemberRole::Owner,
            ] {
                assert_eq!(
                    authorize(&role, p).allowed(),
                    level_of(&role) >= *level,
                    "{role:?} × {p}"
                );
            }
        }
    }

    #[test]
    fn deny_wins_and_unknown_is_refused() {
        let allow = custom("all", Effect::Allow, &["*"]);
        let no_flows = custom("no-flows", Effect::Deny, &["flows:*"]);
        assert!(evaluate([&allow], "flows:deploy").allowed());
        assert_eq!(
            evaluate([&allow, &no_flows], "flows:deploy"),
            Decision::Deny(DenyReason::Explicit {
                role: "no-flows".into()
            })
        );
        assert!(evaluate([&allow, &no_flows], "devices:read").allowed());
        assert_eq!(
            evaluate([&allow], "flows:fly"),
            Decision::Deny(DenyReason::Unknown)
        );
        // `flows:*` must not cover a service that merely starts with "flows".
        assert!(!matches("flows:*", "flows.x:read"));
        assert_eq!(
            evaluate([] as [&Role; 0], "flows:read"),
            Decision::Deny(DenyReason::Implicit)
        );
    }

    #[test]
    fn custom_roles_add_and_remove_but_never_touch_iam() {
        let mut viewer = Grants::managed(&OrgMemberRole::Viewer);
        assert!(!viewer.can("flows:deploy"));
        viewer.0.push(Cow::Owned(custom(
            "deployer",
            Effect::Allow,
            &["flows:deploy", "*"],
        )));
        assert!(viewer.can("flows:deploy"));
        assert!(!viewer.can("iam:read"), "a custom role never grants iam");
        let mut owner = Grants::managed(&OrgMemberRole::Owner);
        owner
            .0
            .push(Cow::Owned(custom("lockout", Effect::Deny, &["*"])));
        assert!(!owner.can("flows:read"), "a deny takes away");
        assert!(owner.can("iam:read"), "never an access lockout");
    }
}
