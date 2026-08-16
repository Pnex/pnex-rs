//! Sélecteur d'organisation — le tenant actif (header `X-Org-Id`). Liste les
//! orgs du user-info courant ; l'écriture passe par `state::org::set`
//! (signal réactif + persistance, source du header côté client HTTP).

use dioxus::prelude::*;

use crate::components::icons;
use crate::state::{org, session};

#[component]
pub fn OrgSwitcher() -> Element {
    let mut open = use_signal(|| false);
    let orgs: Vec<pnex_core::OrgMembership> = session::user()
        .map(|user| user.orgs)
        .unwrap_or_default();

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
                class: "w-full flex items-center justify-between gap-2 rounded-lg bg-gray-800 px-3 py-2 text-left text-sm text-gray-200 hover:bg-gray-700 transition-colors",
                onclick: move |_| open.toggle(),
                span { class: "truncate", {current_name} }
                icons::ChevronDown { class: "h-4 w-4 text-gray-400 flex-shrink-0" }
            }
            if open() {
                div { class: "absolute bottom-full left-0 right-0 mb-1 rounded-lg bg-gray-800 shadow-lg overflow-hidden z-20",
                    for membership in orgs {
                        button {
                            key: "{membership.id}",
                            class: "w-full text-left px-3 py-2 text-sm text-gray-200 hover:bg-gray-700 transition-colors truncate",
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
