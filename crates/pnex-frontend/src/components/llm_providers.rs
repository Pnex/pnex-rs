//! LLM providers of the org (secrets.md D116, D119): list + inline form,
//! shown in the org detail page — the only place they are managed (users
//! bring their own LLM, the platform provides none). The API key is a
//! [`SecretField`]: typed or picked, never shown.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{LlmProvider, LlmProviderInput};
use uuid::Uuid;

use crate::api;
use crate::components::secret_field::{SecretDraft, SecretField};
use crate::state::{ai, toasts};

const BTN: &str = "px-2 py-1 text-xs border border-gray-300 rounded-lg hover:bg-gray-50";
const BTN_PRIMARY: &str =
    "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm";
const INPUT: &str = "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm";

/// Which form is open.
#[derive(Clone, Debug, PartialEq)]
enum Editing {
    New,
    Edit(LlmProvider),
}

#[component]
pub fn LlmProviders(can_manage: bool) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut editing = use_signal(|| None::<Editing>);
    let mut confirm_delete = use_signal(|| None::<Uuid>);
    let list = use_resource(move || async move {
        let _ = reload();
        api::ai::providers().await
    });

    let mut refresh = move || {
        reload += 1;
        spawn(async move { ai::refresh_status().await });
    };

    let help = t!("llm-help-org");
    let rows: Vec<LlmProvider> = match &*list.value().read() {
        Some(Ok(rows)) => rows.clone(),
        _ => Vec::new(),
    };
    let failed = matches!(&*list.value().read(), Some(Err(_)));

    rsx! {
        div { class: "space-y-3",
            div { class: "flex items-start justify-between gap-2",
                div {
                    h3 { class: "text-sm font-semibold text-gray-900", {t!("llm-title")} }
                    p { class: "text-xs text-gray-500 mt-1", {help} }
                }
                if can_manage && editing().is_none() {
                    button {
                        r#type: "button",
                        class: BTN,
                        onclick: move |_| editing.set(Some(Editing::New)),
                        {t!("llm-add")}
                    }
                }
            }
            if failed {
                if let Some(Err(err)) = &*list.value().read() {
                    p { class: "text-sm text-red-700", {api::error_i18n::localize(err)} }
                }
            } else if rows.is_empty() {
                p { class: "text-sm text-gray-500", {t!("llm-empty")} }
            } else {
                div { class: "overflow-x-auto",
                    table { class: "min-w-full text-sm",
                        thead {
                            tr { class: "text-left text-xs uppercase text-gray-500 border-b",
                                th { class: "py-2 pr-4", {t!("llm-col-name")} }
                                th { class: "py-2 pr-4", {t!("llm-col-kind")} }
                                th { class: "py-2 pr-4", {t!("llm-col-model")} }
                                th { class: "py-2 pr-4", {t!("llm-col-key")} }
                                th { class: "py-2" }
                            }
                        }
                        tbody {
                            for p in rows {
                                ProviderRow {
                                    key: "{p.id}",
                                    provider: p.clone(),
                                    can_manage,
                                    confirming: confirm_delete() == Some(p.id),
                                    on_edit: move |p: LlmProvider| editing.set(Some(Editing::Edit(p))),
                                    on_ask_delete: move |id: Uuid| confirm_delete.set(Some(id)),
                                    on_deleted: move |_| {
                                        confirm_delete.set(None);
                                        refresh();
                                    },
                                }
                            }
                        }
                    }
                }
            }
            match editing() {
                Some(Editing::New) => rsx! {
                    ProviderForm {
                        existing: None,
                        on_done: move |_| {
                            editing.set(None);
                            refresh();
                        },
                        on_cancel: move |_| editing.set(None),
                    }
                },
                Some(Editing::Edit(p)) => rsx! {
                    ProviderForm {
                        key: "{p.id}",
                        existing: Some(p.clone()),
                        on_done: move |_| {
                            editing.set(None);
                            refresh();
                        },
                        on_cancel: move |_| editing.set(None),
                    }
                },
                None => rsx! {},
            }
        }
    }
}

#[component]
fn ProviderRow(
    provider: LlmProvider,
    can_manage: bool,
    confirming: bool,
    on_edit: EventHandler<LlmProvider>,
    on_ask_delete: EventHandler<Uuid>,
    on_deleted: EventHandler<()>,
) -> Element {
    let mut testing = use_signal(|| false);
    let id = provider.id;
    let editable = can_manage;
    let key_label = match &provider.api_key {
        Some(view) => format!("{} · {}", t!("llm-key-set"), view.name),
        None if provider.api_key_set => t!("llm-key-set").to_string(),
        None => t!("llm-key-unset").to_string(),
    };
    let for_edit = provider.clone();
    rsx! {
        tr { class: "border-b border-gray-100",
            td { class: "py-2 pr-4 text-gray-900",
                span { class: "font-medium", {provider.name.clone()} }
                if provider.is_default {
                    span { class: "ml-2 px-1.5 py-0.5 rounded bg-blue-50 text-blue-700 text-xs",
                        {t!("llm-default")}
                    }
                }
            }
            td { class: "py-2 pr-4 text-gray-600", {provider.kind.clone()} }
            td { class: "py-2 pr-4 text-gray-600 font-mono text-xs", {provider.model.clone()} }
            td { class: "py-2 pr-4 text-gray-600 text-xs font-mono", {key_label} }
            td { class: "py-2 text-right whitespace-nowrap space-x-1",
                if editable {
                    button {
                        r#type: "button",
                        class: BTN,
                        disabled: testing(),
                        onclick: move |_| {
                            testing.set(true);
                            spawn(async move {
                                match api::ai::test_provider(id).await {
                                    Ok(r) if r.ok => {
                                        toasts::success(
                                            t!("llm-test-ok", ms : r.latency_ms.unwrap_or_default())
                                                .to_string(),
                                        )
                                    }
                                    Ok(r) => {
                                        toasts::error(
                                            format!(
                                                "{} — {}",
                                                t!("llm-test-fail"),
                                                r.error.unwrap_or_default(),
                                            ),
                                        )
                                    }
                                    Err(err) => toasts::error(err),
                                }
                                testing.set(false);
                            });
                        },
                        {t!("llm-test")}
                    }
                    button {
                        r#type: "button",
                        class: BTN,
                        onclick: move |_| on_edit.call(for_edit.clone()),
                        {t!("llm-edit")}
                    }
                    if confirming {
                        button {
                            r#type: "button",
                            class: "px-2 py-1 text-xs rounded-lg bg-red-600 text-white hover:bg-red-700",
                            onclick: move |_| {
                                spawn(async move {
                                    match api::ai::delete_provider(id).await {
                                        Ok(()) => {
                                            toasts::success(t!("llm-deleted").to_string());
                                            on_deleted.call(());
                                        }
                                        Err(err) => toasts::error(err),
                                    }
                                });
                            },
                            {t!("llm-delete-confirm")}
                        }
                    } else {
                        button {
                            r#type: "button",
                            class: "px-2 py-1 text-xs border border-red-200 text-red-700 rounded-lg hover:bg-red-50",
                            onclick: move |_| on_ask_delete.call(id),
                            {t!("llm-delete")}
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ProviderForm(
    existing: Option<LlmProvider>,
    on_done: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    let init = existing.clone();
    let mut name = use_signal(|| init.as_ref().map(|p| p.name.clone()).unwrap_or_default());
    let mut kind = use_signal(|| {
        init.as_ref()
            .map(|p| p.kind.clone())
            .unwrap_or_else(|| "anthropic".to_string())
    });
    let mut base_url = use_signal(|| {
        init.as_ref()
            .and_then(|p| p.base_url.clone())
            .unwrap_or_default()
    });
    let mut model = use_signal(|| init.as_ref().map(|p| p.model.clone()).unwrap_or_default());
    let mut is_default = use_signal(|| init.as_ref().is_some_and(|p| p.is_default));
    let key = use_signal(|| SecretDraft::from_view(init.as_ref().and_then(|p| p.api_key.clone())));
    let mut busy = use_signal(|| false);
    let id = existing.as_ref().map(|p| p.id);

    let save = move |_| {
        let input = LlmProviderInput {
            name: name(),
            kind: kind(),
            base_url: Some(base_url().trim().to_string()).filter(|u| !u.is_empty()),
            model: model(),
            api_key: key.read().to_input(),
            is_default: is_default(),
        };
        busy.set(true);
        spawn(async move {
            let result = match id {
                Some(id) => api::ai::update_provider(id, &input).await,
                None => api::ai::create_provider(&input).await,
            };
            busy.set(false);
            match result {
                Ok(_) => {
                    toasts::success(t!("llm-saved").to_string());
                    on_done.call(());
                }
                Err(err) => toasts::error(err),
            }
        });
    };

    rsx! {
        div { class: "mt-2 rounded-lg border border-gray-200 bg-gray-50 p-4 space-y-3",
            div { class: "grid grid-cols-1 md:grid-cols-2 gap-3",
                div {
                    label { class: "block text-xs text-gray-600 mb-1", {t!("llm-name")} }
                    input {
                        class: INPUT,
                        r#type: "text",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                div {
                    label { class: "block text-xs text-gray-600 mb-1", {t!("llm-kind")} }
                    select {
                        class: "{INPUT} bg-white",
                        value: "{kind}",
                        onchange: move |e| kind.set(e.value()),
                        option {
                            value: "anthropic",
                            selected: kind() == "anthropic",
                            "Anthropic"
                        }
                        option {
                            value: "openai_compat",
                            selected: kind() == "openai_compat",
                            "OpenAI-compatible"
                        }
                    }
                }
                div {
                    label { class: "block text-xs text-gray-600 mb-1", {t!("llm-base-url")} }
                    input {
                        class: INPUT,
                        r#type: "text",
                        value: "{base_url}",
                        placeholder: t!("llm-base-url-hint"),
                        oninput: move |e| base_url.set(e.value()),
                    }
                }
                div {
                    label { class: "block text-xs text-gray-600 mb-1", {t!("llm-model")} }
                    input {
                        class: INPUT,
                        r#type: "text",
                        value: "{model}",
                        oninput: move |e| model.set(e.value()),
                    }
                }
            }
            SecretField {
                label: t!("llm-api-key").to_string(),
                draft: key,
                can_manage: true,
            }
            label { class: "inline-flex items-center gap-2 text-sm text-gray-700",
                input {
                    r#type: "checkbox",
                    checked: is_default(),
                    onchange: move |e| is_default.set(e.checked()),
                }
                {t!("llm-is-default")}
            }
            div { class: "flex gap-2",
                button {
                    r#type: "button",
                    class: BTN_PRIMARY,
                    disabled: busy(),
                    onclick: save,
                    {t!("llm-save")}
                }
                button {
                    r#type: "button",
                    class: "px-4 py-2 border border-gray-300 text-gray-700 rounded-lg hover:bg-gray-50 text-sm",
                    onclick: move |_| on_cancel.call(()),
                    {t!("llm-cancel")}
                }
            }
        }
    }
}
