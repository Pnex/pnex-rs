//! LLM providers of the org (secrets.md D116, D119): list + inline form,
//! shown in the org detail page — the only place they are managed (users
//! bring their own LLM, the platform provides none). The API key is a
//! [`SecretField`]: typed or picked, never shown.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{LlmProvider, LlmProviderInput, LlmUsage};
use uuid::Uuid;

use crate::api;
use crate::components::secret_field::{SecretDraft, SecretField};
use crate::components::ui::{BTN_DANGER, BTN_DANGER_SOLID, BTN_PRIMARY, BTN_SECONDARY, INPUT};
use crate::state::{ai, toasts};

const BTN: &str = crate::components::ui::BTN_SECONDARY_SM;
// Explicit white: the edit form sits on a grey block (fields must not look
// disabled).

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
            div { class: "flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between",
                div {
                    h3 { class: "text-sm font-semibold text-gray-900", {t!("llm-title")} }
                    p { class: "text-xs text-gray-500 mt-1", {help} }
                }
                if can_manage && editing().is_none() {
                    // Same primary « + » action as « Add member » (org page).
                    button {
                        r#type: "button",
                        class: "{BTN_PRIMARY} self-start whitespace-nowrap",
                        onclick: move |_| editing.set(Some(Editing::New)),
                        crate::components::icons::Plus { class: "h-4 w-4 mr-1" }
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
                            tr { class: "text-left text-xs text-gray-500 border-b",
                                th { class: "py-2 pr-4", {t!("llm-col-name")} }
                                th { class: "py-2 pr-4", {t!("llm-col-kind")} }
                                th { class: "py-2 pr-4 hidden md:table-cell", {t!("llm-col-model")} }
                                th { class: "py-2 pr-4 hidden md:table-cell", {t!("llm-col-key")} }
                                th { class: "py-2 sticky right-0 bg-white md:static" }
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
                p { class: "text-xs text-gray-500 font-mono md:hidden", {provider.model.clone()} }
                if provider.is_default {
                    span { class: "ml-2 px-1.5 py-0.5 rounded bg-signal-50 text-signal-700 text-xs",
                        {t!("llm-default")}
                    }
                }
                for u in provider.usages.clone() {
                    span { class: "ml-2 px-1.5 py-0.5 rounded bg-gray-100 text-gray-700 text-xs",
                        {usage_label(u)}
                    }
                }
            }
            td { class: "py-2 pr-4 text-gray-600", {provider.kind.as_str()} }
            td { class: "py-2 pr-4 hidden text-gray-600 font-mono text-xs md:table-cell",
                {provider.model.clone()}
            }
            td { class: "py-2 pr-4 hidden text-gray-600 text-xs font-mono md:table-cell",
                {key_label}
            }
            td { class: "py-2 pl-2 text-right space-x-1 sticky right-0 bg-white whitespace-nowrap shadow-[-6px_0_6px_-6px_rgba(0,0,0,0.25)] md:static md:shadow-none",
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
                                        let reason = match r.code.as_deref() {
                                            Some(code) => {
                                                api::error_i18n::localize_code(
                                                    code,
                                                    r.args.as_ref(),
                                                    r.error.as_deref().unwrap_or_default(),
                                                )
                                            }
                                            None => r.error.unwrap_or_default(),
                                        };
                                        toasts::error(format!("{} — {}", t!("llm-test-fail"), reason))
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
                            class: BTN_DANGER_SOLID,
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
                            class: BTN_DANGER,
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
            .map(|p| p.kind.as_str().to_string())
            .unwrap_or_else(|| "anthropic".to_string())
    });
    let mut base_url = use_signal(|| {
        init.as_ref()
            .and_then(|p| p.base_url.clone())
            .unwrap_or_default()
    });
    let mut model = use_signal(|| init.as_ref().map(|p| p.model.clone()).unwrap_or_default());
    let mut is_default = use_signal(|| init.as_ref().is_some_and(|p| p.is_default));
    let mut usages = use_signal(|| init.as_ref().map(|p| p.usages.clone()).unwrap_or_default());
    let key = use_signal(|| SecretDraft::from_view(init.as_ref().and_then(|p| p.api_key.clone())));
    let mut busy = use_signal(|| false);
    let id = existing.as_ref().map(|p| p.id);

    let save = move |_| {
        let input = LlmProviderInput {
            name: name(),
            kind: pnex_core::LlmProviderKind::parse(&kind()),
            base_url: Some(base_url().trim().to_string()).filter(|u| !u.is_empty()),
            model: model(),
            api_key: key.read().to_input(),
            is_default: is_default(),
            usages: usages(),
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
                    label {
                        r#for: "llm-providers-field-1",
                        class: "block text-xs text-gray-600 mb-1",
                        {t!("llm-name")}
                    }
                    input {
                        id: "llm-providers-field-1",
                        class: INPUT,
                        r#type: "text",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                div {
                    label {
                        r#for: "llm-providers-field-2",
                        class: "block text-xs text-gray-600 mb-1",
                        {t!("llm-kind")}
                    }
                    select {
                        id: "llm-providers-field-2",
                        class: INPUT,
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
                    label {
                        r#for: "llm-providers-field-3",
                        class: "block text-xs text-gray-600 mb-1",
                        {t!("llm-base-url")}
                    }
                    input {
                        id: "llm-providers-field-3",
                        class: INPUT,
                        r#type: "text",
                        value: "{base_url}",
                        placeholder: t!("llm-base-url-hint"),
                        oninput: move |e| base_url.set(e.value()),
                    }
                }
                div {
                    label {
                        r#for: "llm-providers-field-4",
                        class: "block text-xs text-gray-600 mb-1",
                        {t!("llm-model")}
                    }
                    input {
                        id: "llm-providers-field-4",
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
            fieldset { class: "space-y-1",
                legend { class: "block text-xs text-gray-600", {t!("llm-used-for")} }
                p { class: "text-xs text-gray-500", {t!("llm-used-for-help")} }
                div { class: "flex flex-wrap gap-x-4 gap-y-1",
                    for u in LlmUsage::ALL {
                        label { class: "inline-flex items-center gap-2 text-sm text-gray-700",
                            input {
                                r#type: "checkbox",
                                checked: usages.read().contains(&u),
                                onchange: move |e| {
                                    let on = e.checked();
                                    usages.with_mut(|v| v.retain(|x| *x != u));
                                    if on {
                                        usages.write().push(u);
                                    }
                                },
                            }
                            {usage_label(u)}
                        }
                    }
                }
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
                    class: BTN_SECONDARY,
                    onclick: move |_| on_cancel.call(()),
                    {t!("llm-cancel")}
                }
            }
        }
    }
}

fn usage_label(u: LlmUsage) -> String {
    match u {
        LlmUsage::Assistant => t!("llm-usage-assistant"),
        LlmUsage::Extraction => t!("llm-usage-extraction"),
        LlmUsage::Vision => t!("llm-usage-vision"),
    }
    .to_string()
}
