//! The markings a table or an object carries (governance.md D800–D802), and for an
//! administrator the means to add one or to declassify one with a reason.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::governance::JUSTIFICATION_MAX;

use crate::api;
use crate::components::crud::form::FormDialog;
use crate::components::ui::{BTN_SECONDARY_SM, FIELD_HELP, FIELD_LABEL, INPUT};
use crate::state::access;

// A key may be 64 characters long: the chip wraps instead of pushing its button out.
const CHIP: &str = "inline-flex max-w-full flex-wrap items-center gap-x-1.5 rounded-full border border-line-strong bg-surface px-2.5 py-0.5 text-xs font-medium text-ink";

#[component]
pub fn MarkingsPanel(
    markings: Vec<String>,
    on_add: EventHandler<String>,
    /// `(marking, justification)`.
    on_declassify: EventHandler<(String, String)>,
) -> Element {
    let can_write = access::can("governance:write");
    let mut picked = use_signal(String::new);
    let mut declassifying = use_signal(|| None::<String>);
    let all =
        use_resource(move || async move { api::governance::markings().await.unwrap_or_default() });
    let addable: Vec<(String, String)> = all
        .read()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|m| !markings.contains(&m.key))
        .map(|m| (m.key, m.name))
        .collect();
    let chosen = picked();
    rsx! {
        section { class: "space-y-2", "data-testid": "markings",
            h2 { class: "text-sm font-semibold text-ink", {t!("gov-markings")} }
            if markings.is_empty() {
                p { class: FIELD_HELP, {t!("gov-none")} }
            } else {
                ul { class: "flex flex-wrap gap-2",
                    for m in markings.iter().cloned() {
                        li { key: "{m}", class: CHIP,
                            code { class: "min-w-0 [overflow-wrap:anywhere]", "{m}" }
                            if can_write {
                                button {
                                    r#type: "button",
                                    class: "text-slate underline-offset-2 hover:underline",
                                    onclick: {
                                        let m = m.clone();
                                        move |_| declassifying.set(Some(m.clone()))
                                    },
                                    {t!("gov-declassify")}
                                }
                            }
                        }
                    }
                }
            }
            if can_write && !addable.is_empty() {
                div { class: "flex gap-2",
                    select {
                        class: INPUT,
                        aria_label: t!("gov-col-marking").to_string(),
                        onchange: move |e| picked.set(e.value()),
                        option { value: "", selected: chosen.is_empty(), {t!("gov-choose")} }
                        for (key, name) in addable {
                            option {
                                key: "{key}",
                                value: "{key}",
                                selected: chosen == key,
                                "{name}"
                            }
                        }
                    }
                    button {
                        r#type: "button",
                        class: BTN_SECONDARY_SM,
                        disabled: chosen.is_empty(),
                        onclick: move |_| {
                            let key = picked();
                            picked.set(String::new());
                            on_add.call(key);
                        },
                        {t!("gov-mark")}
                    }
                }
            }
        }
        if let Some(m) = declassifying() {
            DeclassifyDialog {
                marking: m.clone(),
                on_close: move |_| declassifying.set(None),
                on_confirm: move |reason: String| {
                    declassifying.set(None);
                    on_declassify.call((m.clone(), reason));
                },
            }
        }
    }
}

#[component]
fn DeclassifyDialog(
    marking: String,
    on_close: EventHandler<()>,
    on_confirm: EventHandler<String>,
) -> Element {
    let mut reason = use_signal(String::new);
    let n = reason().trim().chars().count();
    rsx! {
        FormDialog {
            title: t!("gov-declassify-title", marking : marking.clone()).to_string(),
            submit_label: t!("gov-declassify").to_string(),
            on_close: move |_| on_close.call(()),
            on_submit: move |_| on_confirm.call(reason().trim().to_string()),
            busy: false,
            valid: (10..=JUSTIFICATION_MAX).contains(&n),
            div {
                label { class: FIELD_LABEL, r#for: "gov-justification", {t!("gov-justification")} }
                textarea {
                    id: "gov-justification",
                    class: INPUT,
                    rows: "3",
                    value: "{reason}",
                    oninput: move |e| reason.set(e.value()),
                }
                p { class: FIELD_HELP, {t!("gov-justification-help")} }
            }
        }
    }
}

/// The markings of an ontology object, for those who may read them.
#[component]
pub fn ObjectMarkings(id: String) -> Element {
    let mut reload = use_signal(|| 0u32);
    let oid = id.clone();
    let markings = use_resource(move || {
        let id = oid.clone();
        async move {
            let _ = reload();
            api::governance::object_markings(&id).await
        }
    });
    if !access::can("governance:read") {
        return rsx! {};
    }
    let Some(Ok(current)) = markings.read().clone() else {
        return rsx! {};
    };
    let (add_id, declassify_id) = (id.clone(), id.clone());
    rsx! {
        MarkingsPanel {
            markings: current,
            on_add: move |key: String| {
                let id = add_id.clone();
                spawn(async move {
                    match api::governance::mark_object(&id, vec![key]).await {
                        Ok(_) => reload.with_mut(|r| *r += 1),
                        Err(e) => {
                            crate::state::toasts::error(crate::api::error_i18n::localize(&e))
                        }
                    }
                });
            },
            on_declassify: move |(marking, justification): (String, String)| {
                let id = declassify_id.clone();
                spawn(async move {
                    let body = pnex_core::governance::Declassify {
                        marking,
                        justification,
                    };
                    match api::governance::declassify_object(&id, &body).await {
                        Ok(_) => reload.with_mut(|r| *r += 1),
                        Err(e) => {
                            crate::state::toasts::error(crate::api::error_i18n::localize(&e))
                        }
                    }
                });
            },
        }
    }
}

/// Everything the org holds about an object, saved as one JSON file (D806).
#[component]
pub fn ExportObjectButton(id: String, class: &'static str) -> Element {
    if !access::can("governance:write") {
        return rsx! {};
    }
    rsx! {
        button {
            r#type: "button",
            class,
            "data-testid": "object-export",
            onclick: move |_| {
                let id = id.clone();
                spawn(async move {
                    match api::governance::export_object(&id).await {
                        Ok(bytes) => crate::util::save_blob(&format!("export-{id}.json"), &bytes),
                        Err(e) => {
                            crate::state::toasts::error(crate::api::error_i18n::localize(&e))
                        }
                    }
                });
            },
            {t!("gov-export-object")}
        }
    }
}
