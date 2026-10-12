//! "Add to a case" (cases-workspace.md K22): puts any object among the
//! undated pieces of an open case; its role is chosen when it is attached
//! to an event of the case.

use dioxus::prelude::*;
use dioxus_i18n::prelude::i18n;
use dioxus_i18n::t;
use pnex_core::ontology::api::{CaseAttachInput, FilterOp, OntologyQuery, PropertyFilter};
use pnex_core::ontology::{CASE_STATUS, KIND_CASE};
use serde_json::Value;

use crate::api;
use crate::components::crud::form::FormDialog;
use crate::components::icons;
use crate::components::ui::{BTN_SECONDARY_SM as BTN, FIELD_HELP, FIELD_LABEL as LABEL, INPUT};
use crate::state::toasts;

/// Translated role of an attachment (`case-role-<role>`, else the raw value).
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
    let mut busy = use_signal(|| false);
    rsx! {
        FormDialog {
            title: t!("case-add").to_string(),
            submit_label: t!("case-add-submit").to_string(),
            busy: busy(),
            valid: !case_id().is_empty(),
            on_close: move |_| on_close.call(()),
            on_submit: move |_| {
                let case = case_id();
                let input = CaseAttachInput {
                    object_id: object_id.clone(),
                    ..Default::default()
                };
                let done = t!("case-added").to_string();
                busy.set(true);
                spawn(async move {
                    let res = api::cases::attach(&case, &input).await;
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
                p { class: FIELD_HELP, {t!("case-add-help")} }
            }
        }
    }
}
