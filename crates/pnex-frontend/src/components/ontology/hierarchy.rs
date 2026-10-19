//! Hierarchy tab of the object page (partner-watch.md D305): a tree
//! computed on the links a pack declares, at a date; a node with several
//! parents shows under each, with the others named.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{HierarchyView, ObjectTypeView};

use super::{icon_of, tr, TypeIcon};
use crate::api;
use crate::app::Route;

const INPUT: &str = "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white";

#[component]
pub fn HierarchyPanel(id: String, initial: HierarchyView, types: Vec<ObjectTypeView>) -> Element {
    let mut key = use_signal(|| initial.key.clone());
    let mut at = use_signal(String::new);
    let hid = id.clone();
    let view = use_resource(move || {
        let id = hid.clone();
        let (k, a) = (key(), at());
        async move { api::ontology::hierarchy(&id, &k, &a).await }
    });
    let body = match &*view.read() {
        Some(Ok(v)) if v.nodes.len() <= 1 => rsx! {
            p { class: "text-sm text-gray-500", {t!("onto-hierarchy-empty")} }
        },
        Some(Ok(v)) => {
            let v = v.clone();
            rsx! {
                ul { class: "space-y-1", "data-testid": "onto-hierarchy",
                    for (i, n) in v.nodes.iter().cloned().enumerate() {
                        li {
                            key: "{i}",
                            class: "flex flex-wrap items-center gap-2 text-sm",
                            style: "padding-left: {n.depth * 20}px",
                            TypeIcon {
                                icon: types
                                    .iter()
                                    .find(|t| t.def.key == n.type_key)
                                    .map(|t| icon_of(&t.def))
                                    .unwrap_or_default(),
                                class: "h-4 w-4 text-gray-500",
                            }
                            Link {
                                class: "text-signal-700 hover:underline",
                                to: Route::OntologyObject {
                                    id: n.id.clone(),
                                },
                                "{n.title}"
                            }
                            if !n.also_under.is_empty() {
                                span { class: "text-xs text-gray-500",
                                    {t!("onto-hierarchy-also-under", parents : n.also_under.join(", "))}
                                }
                            }
                        }
                    }
                }
                if v.truncated {
                    p { class: "text-xs text-amber-700", {t!("onto-hierarchy-truncated")} }
                }
            }
        }
        Some(Err(e)) => {
            let msg = crate::api::error_i18n::localize(e);
            rsx! {
                p { class: "text-sm text-red-600", "{msg}" }
            }
        }
        None => rsx! {},
    };
    rsx! {
        div { class: "bg-white rounded-lg border border-gray-200 p-4 space-y-3",
            div { class: "flex flex-wrap items-center gap-3",
                if initial.hierarchies.len() > 1 {
                    select { class: INPUT, onchange: move |e| key.set(e.value()),
                        for h in initial.hierarchies.clone() {
                            option {
                                key: "{h.key}",
                                value: "{h.key}",
                                selected: key() == h.key,
                                {tr(&h.name, &h.names)}
                            }
                        }
                    }
                }
                label { class: "text-sm text-gray-700", {t!("onto-hierarchy-at")} }
                input {
                    class: INPUT,
                    r#type: "date",
                    value: "{at}",
                    onchange: move |e| at.set(e.value()),
                }
            }
            {body}
        }
    }
}
