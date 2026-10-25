//! Page `/access` — access management of the org (iam.md §9, lot I7):
//! roles (managed and custom), teams, assignments and service accounts
//! with their access keys, and the applications that use them (D433). Owner and admin only (`iam:*`); the server
//! decides, the page hides what the caller may not do.
//!
//! School `pages/edge_refs.rs`: tabs above the CRUD socle (ListLayout +
//! DataTable + FormDialog + ConfirmDialog), one component per tab.

mod apps;
mod assignments;
mod grid;
mod roles;
mod service_accounts;
mod teams;

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::crud::layout::ListLayout;
use crate::components::icons;
use crate::components::ui::{TAB, TAB_ACTIVE, TAB_BAR};
use crate::state::{access, org};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Roles,
    Teams,
    Assignments,
    ServiceAccounts,
    Apps,
}

/// Opens the create form of the active tab (bumped by the header button).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct AddRequest(u32);

#[component]
pub fn Access() -> Element {
    let mut tab = use_signal(|| Tab::Roles);
    let mut reload = use_signal(|| 0u32);
    let mut add = use_signal(AddRequest::default);
    let can_write = access::can("iam:write");
    let can_write_apps = access::can("apps:write");
    let add_label = match tab() {
        Tab::Roles => t!("access-new-role"),
        Tab::Teams => t!("access-new-team"),
        Tab::Assignments => t!("access-new-assignment"),
        Tab::ServiceAccounts => t!("access-new-service-account"),
        Tab::Apps => t!("access-new-app"),
    };
    let tabs = [
        (Tab::Roles, "access-tab-roles"),
        (Tab::Teams, "access-tab-teams"),
        (Tab::Assignments, "access-tab-assignments"),
        (Tab::ServiceAccounts, "access-tab-service-accounts"),
        (Tab::Apps, "access-tab-apps"),
    ];

    rsx! {
        ListLayout {
            title: t!("access-title").to_string(),
            subtitle: Some(t!("access-subtitle").to_string()),
            can_write: if tab() == Tab::Apps { can_write_apps } else { can_write },
            add_label: Some(add_label.to_string()),
            on_add: move |_| add.with_mut(|a| a.0 += 1),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            tabs: rsx! {
                div { class: TAB_BAR, role: "tablist",
                    for (id, key) in tabs {
                        button {
                            key: "{key}",
                            class: if tab() == id { TAB_ACTIVE } else { TAB },
                            r#type: "button",
                            role: "tab",
                            aria_selected: tab() == id,
                            onclick: move |_| tab.set(id),
                            span { class: "inline-flex items-center gap-1.5",
                                TabIcon { tab: id }
                                {t!(key)}
                            }
                        }
                    }
                }
            },
            if org::current().is_none() {
                p { class: "text-slate text-center py-12", {t!("orgs-empty")} }
            } else if !access::can("iam:read") {
                p { class: "text-slate text-center py-12", {t!("access-not-allowed")} }
            } else {
                match tab() {
                    Tab::Roles => rsx! {
                        roles::RolesTab { reload, add, can_write }
                    },
                    Tab::Teams => rsx! {
                        teams::TeamsTab { reload, add, can_write }
                    },
                    Tab::Assignments => rsx! {
                        assignments::AssignmentsTab { reload, add, can_write }
                    },
                    Tab::ServiceAccounts => rsx! {
                        service_accounts::ServiceAccountsTab { reload, add, can_write }
                    },
                    Tab::Apps => rsx! {
                        apps::AppsTab { reload, add, can_write: can_write_apps }
                    },
                }
            }
        }
    }
}

#[component]
fn TabIcon(tab: Tab) -> Element {
    let class = "h-4 w-4".to_string();
    match tab {
        Tab::Roles => rsx! {
            icons::Layers { class }
        },
        Tab::Teams => rsx! {
            icons::User { class }
        },
        Tab::Assignments => rsx! {
            icons::Check { class }
        },
        Tab::ServiceAccounts => rsx! {
            icons::Cpu { class }
        },
        Tab::Apps => rsx! {
            icons::Layers { class }
        },
    }
}

/// Display name of a member (`full name`, else email, else `#id`).
pub(crate) fn member_label(m: &pnex_core::OrgMember) -> String {
    m.full_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .or_else(|| m.email.clone())
        .unwrap_or_else(|| format!("#{}", m.user_id))
}

/// Localized name of a role: managed roles by their key, custom ones by
/// their name.
pub(crate) fn role_label(role: &crate::api::iam::Role) -> String {
    if role.managed {
        t!(&format!("access-managed-{}", role.key)).to_string()
    } else {
        role.name.clone()
    }
}
