//! `/cases` — investigation cases (docs/architecture/cases.md): a case is
//! an ontology object (`case`) whose members are `in_case` links. The list
//! creates a case from one line; the workspace (`workspace.rs`) gathers
//! members, dated links and the timeline on one screen.

pub mod workspace;

use dioxus::prelude::*;
use dioxus_i18n::prelude::i18n;
use dioxus_i18n::t;
use pnex_core::ontology::api::{FilterOp, ObjectInput, ObjectView, OntologyQuery, PropertyFilter};
use pnex_core::ontology::{CASE_QUESTION, CASE_STATUS, CASE_STATUSES, KIND_CASE};
use serde_json::{Map, Value};

use crate::api;
use crate::app::Route;
use crate::components::crud::layout::ListLayout;
use crate::components::ontology::short_time;
use crate::state::{org, toasts};

pub use workspace::CaseWorkspace;

const CHIP_ON: &str = "px-3 py-1 text-sm rounded-full bg-blue-600 text-white";
const CHIP_OFF: &str =
    "px-3 py-1 text-sm rounded-full bg-white border border-gray-300 text-gray-700 hover:bg-gray-50";

/// Translated case status (`case-status-<status>`).
pub fn status_label(status: &str) -> String {
    i18n()
        .try_translate(&format!("case-status-{status}"))
        .unwrap_or_else(|_| status.to_string())
}

pub fn status_class(status: &str) -> &'static str {
    match status {
        "open" => "rounded px-2 py-0.5 text-xs bg-green-50 text-green-700",
        "concluded" => "rounded px-2 py-0.5 text-xs bg-blue-50 text-blue-700",
        _ => "rounded px-2 py-0.5 text-xs bg-gray-100 text-gray-600",
    }
}

fn prop_str(o: &ObjectView, key: &str) -> String {
    o.properties
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[component]
pub fn Cases() -> Element {
    let mut reload = use_signal(|| 0u32);
    // Empty = every status.
    let mut status = use_signal(|| "open".to_string());
    let mut draft = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let can_write = org::current_can_write();
    let navigator = use_navigator();

    let list = use_resource(move || {
        let st = status();
        let q = OntologyQuery {
            type_key: Some(KIND_CASE.to_string()),
            filters: if st.is_empty() {
                Vec::new()
            } else {
                vec![PropertyFilter {
                    key: CASE_STATUS.to_string(),
                    op: FilterOp::Eq,
                    value: Value::String(st),
                }]
            },
            limit: Some(200),
            ..Default::default()
        };
        async move {
            let _ = reload();
            api::ontology::query(&q).await
        }
    });
    let mut create = move || {
        let title = draft().trim().to_string();
        if title.is_empty() || busy() {
            return;
        }
        let mut props = Map::new();
        props.insert(CASE_STATUS.into(), Value::String("open".into()));
        let input = ObjectInput {
            type_key: KIND_CASE.to_string(),
            title,
            properties: props,
            expected_version: None,
        };
        busy.set(true);
        spawn(async move {
            let res = api::ontology::create_object(&input).await;
            busy.set(false);
            match res {
                Ok(o) => {
                    draft.set(String::new());
                    navigator.push(Route::CaseWorkspace { id: o.id });
                }
                Err(e) => toasts::error(e),
            }
        });
    };
    let rows: Vec<ObjectView> = match &*list.read() {
        Some(Ok(r)) => r.rows.iter().map(|r| r.object.clone()).collect(),
        _ => Vec::new(),
    };
    let loading = list.read().is_none();
    let failed = matches!(&*list.read(), Some(Err(_)));
    let mut filters: Vec<(String, String)> = CASE_STATUSES
        .iter()
        .map(|s| (s.to_string(), status_label(s)))
        .collect();
    filters.push((String::new(), t!("case-filter-all").to_string()));

    rsx! {
        ListLayout {
            title: t!("cases-title").to_string(),
            subtitle: t!("cases-subtitle").to_string(),
            can_write,
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            div { class: "space-y-4",
                if can_write {
                    form {
                        class: "flex gap-2",
                        onsubmit: move |e| {
                            e.prevent_default();
                            create();
                        },
                        input {
                            class: "flex-1 px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                            "data-testid": "case-new-title",
                            placeholder: t!("case-new-placeholder"),
                            maxlength: 255,
                            value: "{draft}",
                            oninput: move |e| draft.set(e.value()),
                        }
                        button {
                            r#type: "submit",
                            class: "px-4 py-2 text-sm font-medium rounded-lg bg-blue-600 text-white hover:bg-blue-700 disabled:opacity-50",
                            disabled: busy() || draft().trim().is_empty(),
                            {t!("case-new")}
                        }
                    }
                }
                div { class: "flex flex-wrap gap-2",
                    for (key, label) in filters {
                        button {
                            key: "{key}",
                            r#type: "button",
                            class: if status() == key { CHIP_ON } else { CHIP_OFF },
                            onclick: move |_| status.set(key.clone()),
                            "{label}"
                        }
                    }
                }
                if loading {
                    p { class: "text-sm text-gray-500", {t!("common-loading")} }
                } else if failed {
                    p { class: "text-sm text-red-600", {t!("cases-load-error")} }
                } else if rows.is_empty() {
                    p { class: "text-sm text-gray-500 py-8 text-center", {t!("cases-empty")} }
                }
                ul { class: "grid gap-3 sm:grid-cols-2 xl:grid-cols-3",
                    for c in rows {
                        CaseCard { key: "{c.id}", case: c.clone() }
                    }
                }
            }
        }
    }
}

#[component]
fn CaseCard(case: ObjectView) -> Element {
    let st = prop_str(&case, CASE_STATUS);
    let question = prop_str(&case, CASE_QUESTION);
    let updated = short_time(&case.updated_at);
    rsx! {
        li {
            Link {
                class: "block h-full bg-white rounded-lg border border-gray-200 p-4 hover:border-blue-400 hover:shadow-sm",
                "data-testid": "case-card",
                to: Route::CaseWorkspace {
                    id: case.id.clone(),
                },
                div { class: "flex items-start justify-between gap-2",
                    span { class: "font-medium text-gray-900", "{case.title}" }
                    span { class: status_class(&st), {status_label(&st)} }
                }
                if !question.is_empty() {
                    p { class: "mt-1 text-sm text-gray-600 line-clamp-2", "{question}" }
                }
                p { class: "mt-2 text-xs text-gray-400", "{updated}" }
            }
        }
    }
}
