//! "Add to a case" (cases.md K2): opens an `in_case` link from any object
//! to an open case, with its role in the case.

use dioxus::prelude::*;
use dioxus_i18n::prelude::i18n;
use dioxus_i18n::t;
use pnex_core::ontology::api::{FilterOp, LinkInput, OntologyQuery, PropertyFilter};
use pnex_core::ontology::{
    CASE_STATUS, IN_CASE_NOTE, IN_CASE_ROLE, IN_CASE_ROLES, KIND_CASE, REL_IN_CASE,
};
use serde_json::{Map, Value};

use crate::api;
use crate::components::crud::form::FormDialog;
use crate::components::icons;
use crate::state::toasts;

const BTN: &str = "inline-flex items-center gap-1 px-3 py-1 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50";
const INPUT: &str = "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white";
const LABEL: &str = "block text-xs font-medium text-gray-500 mb-1";

/// Translated role of a member (`case-role-<role>`, else the raw value).
pub fn role_label(role: &str) -> String {
    i18n()
        .try_translate(&format!("case-role-{role}"))
        .unwrap_or_else(|_| role.to_string())
}

/// Button + dialog; `object_id` is the identity of the object to add.
#[component]
pub fn AddToCase(object_id: String) -> Element {
    let mut open = use_signal(|| false);
    rsx! {
        button {
            class: BTN,
            r#type: "button",
            "data-testid": "add-to-case",
            onclick: move |_| open.set(true),
            icons::Folder { class: Some("h-4 w-4".to_string()) }
            {t!("case-add")}
        }
        if open() {
            AddToCaseDialog {
                object_id: object_id.clone(),
                on_close: move |_| open.set(false),
            }
        }
    }
}

#[component]
fn AddToCaseDialog(object_id: String, on_close: Callback<()>) -> Element {
    let cases = use_resource(move || async move {
        let q = OntologyQuery {
            type_key: Some(KIND_CASE.to_string()),
            filters: vec![PropertyFilter {
                key: CASE_STATUS.to_string(),
                op: FilterOp::Eq,
                value: Value::String("open".into()),
            }],
            limit: Some(500),
            ..Default::default()
        };
        api::ontology::query(&q)
            .await
            .map(|r| r.rows.into_iter().map(|r| r.object).collect::<Vec<_>>())
            .unwrap_or_default()
    });
    let me = object_id.clone();
    let cases: Vec<_> = cases
        .read()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.id != me)
        .collect();
    let mut case_id = use_signal(String::new);
    let mut role = use_signal(|| IN_CASE_ROLES[0].to_string());
    let mut note = use_signal(String::new);
    let mut busy = use_signal(|| false);
    rsx! {
        FormDialog {
            title: t!("case-add").to_string(),
            submit_label: t!("case-add-submit").to_string(),
            busy: busy(),
            valid: !case_id().is_empty(),
            on_close: move |_| on_close.call(()),
            on_submit: move |_| {
                let mut attrs = Map::new();
                attrs.insert(IN_CASE_ROLE.into(), Value::String(role()));
                let n = note().trim().to_string();
                if !n.is_empty() {
                    attrs.insert(IN_CASE_NOTE.into(), Value::String(n));
                }
                let input = LinkInput {
                    link_type: REL_IN_CASE.to_string(),
                    source_id: object_id.clone(),
                    target_id: case_id(),
                    attributes: attrs,
                    valid_from: None,
                };
                let done = t!("case-added").to_string();
                busy.set(true);
                spawn(async move {
                    let res = api::ontology::create_link(&input).await;
                    busy.set(false);
                    match res {
                        Ok(_) => {
                            toasts::success(done);
                            on_close.call(());
                        }
                        Err(e) => toasts::error(e),
                    }
                });
            },
            div { class: "space-y-3",
                div {
                    label { class: LABEL, {t!("case-pick")} }
                    select {
                        class: INPUT,
                        "data-testid": "add-to-case-case",
                        onchange: move |e| case_id.set(e.value()),
                        option { value: "", "—" }
                        for c in cases {
                            option {
                                key: "{c.id}",
                                value: "{c.id}",
                                selected: c.id == case_id(),
                                "{c.title}"
                            }
                        }
                    }
                }
                div {
                    label { class: LABEL, {t!("case-role")} }
                    select {
                        class: INPUT,
                        "data-testid": "add-to-case-role",
                        onchange: move |e| role.set(e.value()),
                        for r in IN_CASE_ROLES {
                            option {
                                key: "{r}",
                                value: "{r}",
                                selected: r == role(),
                                {role_label(r)}
                            }
                        }
                    }
                }
                div {
                    label { class: LABEL, {t!("case-note")} }
                    input {
                        class: INPUT,
                        maxlength: 255,
                        value: "{note}",
                        oninput: move |e| note.set(e.value()),
                    }
                }
            }
        }
    }
}
