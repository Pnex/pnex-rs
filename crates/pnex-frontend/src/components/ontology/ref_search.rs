//! A `ref` value picked by searching its target type by title: an action's
//! account among thousands of companies cannot be a plain select.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::OntologyQuery;
use serde_json::{Map, Value};

use crate::api;

/// Matches shown under the search box.
const SHOWN: u64 = 8;

/// `chosen` = title of the value already in `doc` (prefilled), shown in
/// place of its id.
#[component]
pub fn RefSearch(
    field: String,
    label: String,
    to_type: String,
    doc: Signal<Map<String, Value>>,
    #[props(default)] chosen: Option<String>,
    #[props(default)] hint: Option<String>,
    #[props(default)] error: Option<String>,
) -> Element {
    let mut doc = doc;
    let mut title = use_signal(move || chosen.clone());
    let mut text = use_signal(String::new);
    let tt = to_type.clone();
    let found = use_resource(move || {
        let q = OntologyQuery {
            type_key: Some(tt.clone()),
            text: Some(text()).filter(|s| !s.trim().is_empty()),
            limit: Some(SHOWN),
            ..Default::default()
        };
        async move {
            api::ontology::query(&q)
                .await
                .map(|r| r.rows)
                .unwrap_or_default()
        }
    });
    let current = doc
        .read()
        .get(&field)
        .and_then(Value::as_str)
        .map(str::to_string);
    let rows = found.read().clone();
    let f_clear = field.clone();
    rsx! {
        div { "data-prop": "{field}",
            label { class: "block text-xs font-medium text-gray-500 mb-1", "{label}" }
            if let Some(id) = current {
                div { class: "flex items-center justify-between gap-2 px-3 py-2 border border-gray-300 rounded-lg text-sm bg-gray-50",
                    span { class: "truncate", "data-testid": "ref-chosen", {title().unwrap_or(id)} }
                    button {
                        r#type: "button",
                        class: "shrink-0 text-xs font-medium text-blue-700 hover:underline",
                        onclick: move |_| {
                            doc.with_mut(|m| {
                                m.remove(&f_clear);
                            });
                            title.set(None);
                        },
                        {t!("onto-ref-change")}
                    }
                }
                if let Some(h) = hint {
                    p { class: "mt-1 text-xs text-gray-500", "{h}" }
                }
            } else {
                input {
                    class: crate::components::ui::INPUT,
                    "data-testid": "ref-search",
                    placeholder: t!("common-search-name").to_string(),
                    value: "{text}",
                    oninput: move |e| text.set(e.value()),
                }
                if rows.as_ref().is_some_and(Vec::is_empty) {
                    p { class: "mt-1 text-xs text-gray-500", {t!("onto-ref-none")} }
                }
                if let Some(rows) = rows.filter(|r| !r.is_empty()) {
                    ul { class: "mt-1 border border-gray-200 rounded-lg divide-y divide-gray-100 max-h-56 overflow-y-auto",
                        for r in rows {
                            li { key: "{r.object.id}",
                                button {
                                    r#type: "button",
                                    class: "w-full text-left px-3 py-2 text-sm hover:bg-gray-50 focus:bg-gray-50 focus:outline-none",
                                    "data-testid": "ref-option",
                                    onclick: {
                                        let f = field.clone();
                                        let id = r.object.id.clone();
                                        let name = r.object.title.clone();
                                        move |_| {
                                            doc.with_mut(|m| {
                                                m.insert(f.clone(), Value::String(id.clone()));
                                            });
                                            title.set(Some(name.clone()));
                                        }
                                    },
                                    "{r.object.title}"
                                }
                            }
                        }
                    }
                }
            }
            if let Some(e) = error {
                p { class: "mt-1 text-xs text-red-600", "{e}" }
            }
        }
    }
}
