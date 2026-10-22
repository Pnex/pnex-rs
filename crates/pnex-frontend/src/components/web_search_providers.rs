//! Web search providers of the org (web-presence.md D360): what the
//! `web_search` flow nodes query, with a Test button. Key = [`SecretField`]
//! (typed or picked, never shown). School of the browser endpoints section.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::web_search::{
    WebSearchKind, WebSearchProvider, WebSearchProviderInput, WebSearchTestResult,
};

use crate::api;
use crate::api::error_i18n::localize;
use crate::components::secret_field::{SecretDraft, SecretField};
use crate::components::ui::{
    BTN_DANGER, BTN_PRIMARY_SM, BTN_SECONDARY_SM, FIELD_ERROR, FIELD_HELP, INPUT,
};

/// Who the form edits: a new provider or an existing one.
#[derive(Clone, PartialEq)]
enum Form {
    New,
    Edit(WebSearchProvider),
}

#[component]
pub fn WebSearchProviders(can_manage: bool) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut form = use_signal(|| None::<Form>);
    let mut tested = use_signal(|| None::<(String, Result<WebSearchTestResult, String>)>);
    let list = use_resource(move || async move {
        let _ = reload();
        api::web_search::providers().await
    });
    let mut test = move |id: String| {
        tested.set(Some((
            id.clone(),
            Err(t!("web-search-testing").to_string()),
        )));
        spawn(async move {
            let r = api::web_search::test(&id).await.map_err(|e| localize(&e));
            tested.set(Some((id, r)));
        });
    };
    rsx! {
        div { class: "space-y-3",
            div { class: "flex items-center justify-between gap-2",
                h3 { class: "text-base font-semibold text-ink", {t!("web-search-title")} }
                if can_manage {
                    button {
                        class: BTN_SECONDARY_SM,
                        onclick: move |_| form.set(Some(Form::New)),
                        {t!("web-search-add")}
                    }
                }
            }
            p { class: FIELD_HELP, {t!("web-search-help")} }
            match &*list.read() {
                Some(Ok(rows)) if rows.is_empty() => rsx! {
                    p { class: "text-sm text-slate", {t!("web-search-empty")} }
                },
                Some(Ok(rows)) => rsx! {
                    ul { class: "divide-y divide-line rounded border border-line",
                        for p in rows.clone() {
                            li { key: "{p.id}", class: "p-2 space-y-1",
                                div { class: "flex flex-wrap items-center gap-2 text-sm",
                                    span { class: "font-medium text-ink", "{p.name}" }
                                    span { class: "rounded border border-line px-1.5 text-xs text-slate",
                                        "{p.kind.as_str()}"
                                    }
                                    if p.is_default {
                                        span { class: "rounded border border-line px-1.5 text-xs text-slate",
                                            {t!("web-search-default")}
                                        }
                                    }
                                    span { class: "truncate font-mono text-xs text-slate", "{p.base_url}" }
                                    if can_manage {
                                        span { class: "ml-auto flex gap-2",
                                            button {
                                                class: BTN_SECONDARY_SM,
                                                onclick: {
                                                    let id = p.id.clone();
                                                    move |_| test(id.clone())
                                                },
                                                {t!("web-search-test")}
                                            }
                                            button {
                                                class: BTN_SECONDARY_SM,
                                                onclick: {
                                                    let p = p.clone();
                                                    move |_| form.set(Some(Form::Edit(p.clone())))
                                                },
                                                {t!("web-search-edit")}
                                            }
                                        }
                                    }
                                }
                                if let Some((id, r)) = tested() {
                                    if id == p.id {
                                        TestReport { result: r }
                                    }
                                }
                            }
                        }
                    }
                },
                Some(Err(e)) => rsx! {
                    p { class: FIELD_ERROR, "{localize(e)}" }
                },
                None => rsx! {},
            }
            if let Some(f) = form() {
                ProviderForm {
                    form: f,
                    on_done: move |_| {
                        form.set(None);
                        reload.with_mut(|n| *n += 1);
                    },
                    on_cancel: move |_| form.set(None),
                }
            }
        }
    }
}

#[component]
fn TestReport(result: Result<WebSearchTestResult, String>) -> Element {
    match result {
        Err(m) => rsx! {
            p { class: "text-sm text-slate", "{m}" }
        },
        Ok(r) if r.ok => rsx! {
            p { class: "text-sm font-medium text-green-800",
                {t!("web-search-test-ok", n : r.results, ms : r.latency_ms)}
            }
        },
        Ok(r) => rsx! {
            p { class: "text-sm font-medium text-alarm",
                {t!("web-search-test-failed")}
                if let Some(d) = r.detail.clone() {
                    span { class: "ml-1 font-mono text-xs", "({d})" }
                }
            }
        },
    }
}

#[component]
fn ProviderForm(form: Form, on_done: EventHandler<()>, on_cancel: EventHandler<()>) -> Element {
    let current = match &form {
        Form::Edit(p) => Some(p.clone()),
        Form::New => None,
    };
    let init = current.clone();
    let mut name = use_signal(|| init.as_ref().map(|p| p.name.clone()).unwrap_or_default());
    let mut kind = use_signal(|| init.as_ref().map(|p| p.kind).unwrap_or_default());
    let mut url = use_signal(|| {
        init.as_ref()
            .map(|p| p.base_url.clone())
            .unwrap_or_default()
    });
    let mut is_default = use_signal(|| init.as_ref().is_some_and(|p| p.is_default));
    let key = use_signal(|| SecretDraft::from_view(init.as_ref().and_then(|p| p.api_key.clone())));
    let mut error = use_signal(|| None::<String>);
    let id = current.as_ref().map(|p| p.id.clone());
    let id_for_delete = id.clone();
    let save = move |_| {
        let input = WebSearchProviderInput {
            name: name(),
            kind: kind(),
            base_url: url(),
            api_key: key.read().to_input(),
            is_default: is_default(),
        };
        let id = id.clone();
        spawn(async move {
            match api::web_search::save(id.as_deref(), &input).await {
                Ok(_) => on_done.call(()),
                Err(e) => error.set(Some(localize(&e))),
            }
        });
    };
    let placeholder = kind()
        .default_base_url()
        .unwrap_or("https://searx.example.org");
    rsx! {
        div { class: "rounded border border-line p-3 space-y-2",
            label { class: "block text-xs text-gray-600", {t!("web-search-name")} }
            input {
                class: INPUT,
                value: "{name}",
                oninput: move |e| name.set(e.value()),
            }
            label { class: "block text-xs text-gray-600", {t!("web-search-kind")} }
            select {
                class: INPUT,
                value: "{kind().as_str()}",
                onchange: move |e| kind.set(WebSearchKind::parse(&e.value()).unwrap_or_default()),
                for k in WebSearchKind::ALL {
                    option { value: "{k.as_str()}", selected: k == kind(), "{k.as_str()}" }
                }
            }
            p { class: FIELD_HELP,
                if kind() == WebSearchKind::Searxng {
                    {t!("web-search-kind-help-searxng")}
                } else {
                    {t!("web-search-kind-help-serper")}
                }
            }
            label { class: "block text-xs text-gray-600", {t!("web-search-url")} }
            input {
                class: "{INPUT} font-mono",
                placeholder: "{placeholder}",
                value: "{url}",
                oninput: move |e| url.set(e.value()),
            }
            SecretField {
                label: t!("web-search-key").to_string(),
                draft: key,
                can_manage: true,
            }
            label { class: "inline-flex items-center gap-2 text-sm",
                input {
                    r#type: "checkbox",
                    checked: is_default(),
                    onchange: move |e| is_default.set(e.checked()),
                }
                {t!("web-search-default")}
            }
            if let Some(m) = error() {
                p { class: FIELD_ERROR, "{m}" }
            }
            div { class: "flex flex-wrap gap-2",
                button { class: BTN_PRIMARY_SM, onclick: save, {t!("web-search-save")} }
                button {
                    class: BTN_SECONDARY_SM,
                    onclick: move |_| on_cancel.call(()),
                    {t!("web-search-cancel")}
                }
                if let Some(id) = id_for_delete.clone() {
                    button {
                        class: "{BTN_DANGER} ml-auto",
                        onclick: move |_| {
                            let id = id.clone();
                            spawn(async move {
                                match api::web_search::delete(&id).await {
                                    Ok(()) => on_done.call(()),
                                    Err(e) => error.set(Some(localize(&e))),
                                }
                            });
                        },
                        {t!("web-search-delete")}
                    }
                }
            }
        }
    }
}
