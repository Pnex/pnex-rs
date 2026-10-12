//! Retention of an archived case (cases.md K16): the persons and
//! identifiers no other case and no other link holds, proposed for a
//! purge that deletes them for good.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{CasePurgeInput, ObjectTypeView};

use crate::api;
use crate::components::confirm::ConfirmDialog;
use crate::components::ontology::type_label_of;
use crate::components::ui::BTN_DANGER;
use crate::state::toasts;

#[component]
pub fn PurgePanel(case_id: String, types: Vec<ObjectTypeView>, on_purged: Callback<()>) -> Element {
    let mut reload = use_signal(|| 0u32);
    let cid = case_id.clone();
    let candidates = use_resource(move || {
        let id = cid.clone();
        let _ = reload();
        async move { api::cases::purge_candidates(&id).await.unwrap_or_default() }
    });
    // Unticked ids: every candidate is proposed, the user takes some out.
    let mut kept = use_signal(Vec::<String>::new);
    let mut confirming = use_signal(|| false);
    let list = candidates.read().clone().unwrap_or_default();
    if list.is_empty() {
        return rsx! {};
    }
    let picked: Vec<String> = list
        .iter()
        .map(|c| c.id.clone())
        .filter(|id| !kept().contains(id))
        .collect();
    let count = picked.len();
    let purge = move |_| {
        confirming.set(false);
        let input = CasePurgeInput {
            ids: picked.clone(),
        };
        let id = case_id.clone();
        spawn(async move {
            match api::cases::purge(&id, &input).await {
                Ok(_) => {
                    toasts::success("case-purged");
                    reload.with_mut(|r| *r += 1);
                    on_purged.call(());
                }
                Err(e) => toasts::error(e),
            }
        });
    };

    rsx! {
        div {
            class: "space-y-3 rounded-md border border-alarm p-3",
            "data-testid": "case-purge",
            h2 { class: "font-semibold text-gray-900", {t!("case-purge")} }
            p { class: "text-sm text-gray-700", {t!("case-purge-help")} }
            ul { class: "space-y-1",
                for c in list.iter() {
                    li { key: "{c.id}",
                        label { class: "flex items-center gap-2 text-sm text-gray-900",
                            input {
                                r#type: "checkbox",
                                checked: !kept().contains(&c.id),
                                onchange: {
                                    let id = c.id.clone();
                                    move |e: Event<FormData>| {
                                        if e.checked() {
                                            kept.with_mut(|k| k.retain(|x| *x != id));
                                        } else {
                                            kept.with_mut(|k| k.push(id.clone()));
                                        }
                                    }
                                },
                            }
                            span { class: "truncate", "{c.title}" }
                            span { class: "text-xs text-gray-500",
                                {type_label_of(&types, &c.type_key)}
                            }
                        }
                    }
                }
            }
            button {
                class: BTN_DANGER,
                r#type: "button",
                "data-testid": "case-purge-start",
                disabled: count == 0,
                onclick: move |_| confirming.set(true),
                {t!("case-purge-action", count : count)}
            }
            if confirming() {
                ConfirmDialog {
                    title: t!("case-purge-confirm-title").to_string(),
                    message: t!("case-purge-confirm", count : count).to_string(),
                    confirm_label: t!("case-purge-action", count : count).to_string(),
                    on_confirm: purge,
                    on_cancel: move |_| confirming.set(false),
                }
            }
        }
    }
}
