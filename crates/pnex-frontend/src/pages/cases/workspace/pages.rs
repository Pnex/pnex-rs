//! Pages of a case (cases.md K8), in the case frame: start a page from a pack template
//! (5 whys, Ishikawa…) and open it at once; designate the conclusion page,
//! versioned when the case concludes.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{CasePageInput, ObjectInput, ObjectRef, ObjectView};
use pnex_core::ontology::CASE_CONCLUSION_PAGE;
use serde_json::Value;

use super::super::prop_str;
use crate::api;
use crate::app::Route;
use crate::components::ui::{BTN_GHOST, BTN_SECONDARY_SM, INPUT};
use crate::state::toasts;

#[component]
pub fn PagesPanel(
    case: ObjectView,
    pages: Vec<ObjectRef>,
    editable: bool,
    on_changed: Callback<()>,
) -> Element {
    let navigator = use_navigator();
    let mut title = use_signal(String::new);
    let mut template = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let templates = use_resource(|| async { api::cases::templates().await.unwrap_or_default() });
    let templates = templates.read().clone().unwrap_or_default();
    let help = templates
        .iter()
        .find(|t| t.key == template())
        .map(|t| crate::components::ontology::tr(&t.description, &t.descriptions))
        .unwrap_or_default();
    let conclusion = prop_str(&case, CASE_CONCLUSION_PAGE);

    let case_id = case.id.clone();
    let mut create = move || {
        let t = title().trim().to_string();
        if t.is_empty() || busy() {
            return;
        }
        let input = CasePageInput {
            title: t,
            template: Some(template()).filter(|k| !k.is_empty()),
        };
        let id = case_id.clone();
        busy.set(true);
        spawn(async move {
            let res = api::cases::add_page(&id, &input).await;
            busy.set(false);
            match res {
                Ok(doc) => {
                    navigator.push(Route::Docs {
                        id: doc.id.to_string(),
                    });
                }
                Err(e) => toasts::error(e),
            }
        });
    };
    // Designates (or clears) the conclusion page on the version read.
    let designate = use_callback(move |page: Option<String>| {
        let mut props = case.properties.clone();
        match page {
            Some(p) => props.insert(CASE_CONCLUSION_PAGE.into(), Value::String(p)),
            None => props.remove(CASE_CONCLUSION_PAGE),
        };
        let input = ObjectInput {
            type_key: case.type_key.clone(),
            title: case.title.clone(),
            properties: props,
            expected_version: Some(case.version),
        };
        let id = case.id.clone();
        spawn(async move {
            match api::ontology::update_object(&id, &input).await {
                Ok(_) => on_changed.call(()),
                Err(e) => toasts::error(e),
            }
        });
    });

    rsx! {
        div { class: "space-y-3", "data-testid": "case-pages",
            h3 { class: "text-sm font-semibold text-ink", {t!("case-pages")} }
            if editable {
                form {
                    class: "space-y-2",
                    onsubmit: move |e| {
                        e.prevent_default();
                        create();
                    },
                    input {
                        class: INPUT,
                        "data-testid": "case-page-title",
                        maxlength: 255,
                        placeholder: t!("case-page-title"),
                        "aria-label": t!("case-page-title"),
                        value: "{title}",
                        oninput: move |e| title.set(e.value()),
                    }
                    div { class: "flex gap-2",
                        select {
                            class: "{INPUT} flex-1",
                            "data-testid": "case-page-template",
                            "aria-label": t!("case-page-template"),
                            value: "{template}",
                            onchange: move |e| template.set(e.value()),
                            option { value: "", {t!("case-page-blank")} }
                            for tpl in templates.iter() {
                                option { key: "{tpl.key}", value: "{tpl.key}",
                                    {crate::components::ontology::tr(&tpl.name, &tpl.names)}
                                }
                            }
                        }
                        button {
                            class: BTN_SECONDARY_SM,
                            r#type: "submit",
                            "data-testid": "case-page-create",
                            disabled: busy() || title().trim().is_empty(),
                            {t!("case-page-create")}
                        }
                    }
                    if !help.is_empty() {
                        p { class: "text-xs text-gray-500", "{help}" }
                    }
                }
            }
            if pages.is_empty() {
                p { class: "text-sm text-gray-500", {t!("case-pages-empty")} }
            } else {
                ul { class: "divide-y divide-gray-100",
                    for p in pages {
                        li {
                            key: "{p.id}",
                            class: "flex items-center gap-2 py-1.5",
                            "data-testid": "case-page-row",
                            Link {
                                class: "flex-1 min-w-0 truncate text-sm text-gray-900 hover:text-signal hover:underline",
                                to: Route::Docs {
                                    id: p.native_id.clone(),
                                },
                                "{p.title}"
                            }
                            if p.id == conclusion {
                                span {
                                    class: "rounded bg-signal-soft px-1.5 py-0.5 text-xs font-medium text-signal",
                                    "data-testid": "case-page-conclusion",
                                    {t!("case-page-conclusion")}
                                }
                                if editable {
                                    button {
                                        class: "text-xs text-gray-500 hover:underline",
                                        r#type: "button",
                                        onclick: move |_| designate.call(None),
                                        {t!("case-page-unset")}
                                    }
                                }
                            } else if editable {
                                button {
                                    class: "{BTN_GHOST} px-2! py-0.5! text-xs!",
                                    r#type: "button",
                                    "data-testid": "case-page-designate",
                                    onclick: {
                                        let id = p.id.clone();
                                        move |_| designate.call(Some(id.clone()))
                                    },
                                    {t!("case-page-designate")}
                                }
                            }
                        }
                    }
                }
            }
            p { class: "text-xs text-gray-500", {t!("case-pages-help")} }
        }
    }
}
