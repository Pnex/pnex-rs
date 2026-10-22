//! "New page" on an object (account-mapping.md D340): a page started from a
//! pack template whose subject is the object's type, filled from the
//! object (account plan, competitor sheet, meeting preparation), then
//! opened in the page editor.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::ObjectPageInput;

use crate::api;
use crate::app::Route;
use crate::components::crud::form::FormDialog;
use crate::components::ontology::tr;
use crate::components::ui::{BTN_SECONDARY_SM as BTN, FIELD_HELP, FIELD_LABEL as LABEL, INPUT};
use crate::state::toasts;

#[component]
pub fn PageFromTemplate(object_id: String, type_key: String) -> Element {
    let mut open = use_signal(|| false);
    let mut picked = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let navigator = use_navigator();
    let templates = use_resource(|| async { api::cases::templates().await.unwrap_or_default() });
    let mine: Vec<_> = templates
        .read()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.subject.as_deref() == Some(type_key.as_str()))
        .collect();
    if mine.is_empty() {
        return rsx! {};
    }
    let first = mine[0].key.clone();
    let submit = move |_| {
        let key = if picked().is_empty() {
            first.clone()
        } else {
            picked()
        };
        let id = object_id.clone();
        busy.set(true);
        spawn(async move {
            let input = ObjectPageInput {
                template: key,
                title: None,
                lang: Some(crate::i18n::current_tag()),
            };
            match api::ontology::page_from_template(&id, &input).await {
                Ok(page) => {
                    open.set(false);
                    navigator.push(Route::Docs {
                        id: page.id.to_string(),
                    });
                }
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    rsx! {
        button {
            class: BTN,
            r#type: "button",
            "data-testid": "page-from-template",
            onclick: move |_| open.set(true),
            {t!("onto-page-new")}
        }
        if open() {
            FormDialog {
                title: t!("onto-page-new").to_string(),
                submit_label: t!("onto-page-create").to_string(),
                on_close: move |_| open.set(false),
                on_submit: submit,
                busy: busy(),
                valid: true,
                div { class: "space-y-2",
                    label { class: LABEL, {t!("onto-page-template")} }
                    select {
                        class: INPUT,
                        onchange: move |e| picked.set(e.value()),
                        for tpl in mine.iter() {
                            option { key: "{tpl.key}", value: "{tpl.key}", {tr(&tpl.name, &tpl.names)} }
                        }
                    }
                    p { class: FIELD_HELP, {t!("onto-page-help")} }
                }
            }
        }
    }
}
