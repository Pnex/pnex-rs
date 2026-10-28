//! Organisation switcher — the active tenant (`X-Org-Id` header). Lists the
//! orgs of the current user-info; writing goes through `state::org::set`
//! (reactive signal + persistence, source of the header in the HTTP client).

use dioxus::prelude::*;

use crate::components::icons;
use crate::state::{org, session};

#[component]
pub fn OrgSwitcher() -> Element {
    let mut open = use_signal(|| false);
    let orgs: Vec<pnex_core::OrgMembership> =
        session::user().map(|user| user.orgs).unwrap_or_default();

    if orgs.is_empty() {
        return rsx! {};
    }
    let current = org::current();
    let current_name = orgs
        .iter()
        .find(|membership| Some(membership.id) == current)
        .map(|membership| membership.name.clone())
        .unwrap_or_default();

    rsx! {
        div { class: "relative",
            button {
                class: "w-full flex items-center justify-between gap-2 rounded-md border border-line bg-surface px-3 py-2 text-left text-sm font-medium text-ink hover:bg-gray-50 transition-colors",
                onclick: move |_| open.toggle(),
                span { class: "truncate", {current_name} }
                icons::ChevronDown { class: "h-4 w-4 text-gray-500 flex-shrink-0" }
            }
            if open() {
                div { class: "pnex-pop-in-up absolute bottom-full left-0 right-0 mb-1 rounded-md border border-line bg-surface shadow-lg overflow-hidden z-20",
                    for membership in orgs {
                        button {
                            key: "{membership.id}",
                            class: "w-full text-left px-3 py-2 text-sm text-ink hover:bg-gray-50 transition-colors truncate",
                            onclick: move |_| {
                                org::set(membership.id);
                                open.set(false);
                            },
                            {membership.name.clone()}
                        }
                    }
                }
            }
        }
    }
}
