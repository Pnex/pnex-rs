//! Browser endpoints of the org (site-recipes.md D325): CDP browsers the
//! navigation rules play on, with a Test button. Token = [`SecretField`]
//! (typed or picked, never shown). School of the geo providers section.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::site_recipe::{BrowserEndpoint, BrowserEndpointInput, BrowserTestResult};

use crate::api;
use crate::api::error_i18n::{localize, resolve};
use crate::components::secret_field::{SecretDraft, SecretField};
use crate::components::ui::{
    BTN_DANGER, BTN_PRIMARY_SM, BTN_SECONDARY_SM, FIELD_ERROR, FIELD_HELP, INPUT,
};

/// Who the form edits: a new endpoint or an existing one.
#[derive(Clone, PartialEq)]
enum Form {
    New,
    Edit(BrowserEndpoint),
}

#[component]
pub fn BrowserEndpoints(can_manage: bool) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut form = use_signal(|| None::<Form>);
    let mut tested = use_signal(|| None::<(String, Result<BrowserTestResult, String>)>);
    let list = use_resource(move || async move {
        let _ = reload();
        api::site_recipes::endpoints().await
    });
    let mut test = move |id: String| {
        tested.set(Some((
            id.clone(),
            Err(t!("browser-endpoints-testing").to_string()),
        )));
        spawn(async move {
            let r = api::site_recipes::endpoint_test(&id)
                .await
                .map_err(|e| localize(&e));
            tested.set(Some((id, r)));
        });
    };
    rsx! {
        div { class: "space-y-3",
            div { class: "flex items-center justify-between gap-2",
                h3 { class: "text-base font-semibold text-ink", {t!("browser-endpoints-title")} }
                if can_manage {
                    button {
                        class: BTN_SECONDARY_SM,
                        onclick: move |_| form.set(Some(Form::New)),
                        {t!("browser-endpoints-add")}
                    }
                }
            }
            p { class: FIELD_HELP, {t!("browser-endpoints-help")} }
            match &*list.read() {
                Some(Ok(rows)) if rows.is_empty() => rsx! {
                    p { class: "text-sm text-slate", {t!("browser-endpoints-empty")} }
                },
                Some(Ok(rows)) => rsx! {
                    ul { class: "divide-y divide-line rounded border border-line",
                        for e in rows.clone() {
                            li { key: "{e.id}", class: "p-2 space-y-1",
                                div { class: "flex flex-wrap items-center gap-2 text-sm",
                                    span { class: "font-medium text-ink", "{e.name}" }
                                    if e.is_default {
                                        span { class: "rounded border border-line px-1.5 text-xs text-slate",
                                            {t!("browser-endpoints-default")}
                                        }
                                    }
                                    span { class: "truncate font-mono text-xs text-slate", "{e.cdp_url}" }
                                    if can_manage {
                                        span { class: "ml-auto flex gap-2",
                                            button {
                                                class: BTN_SECONDARY_SM,
                                                onclick: {
                                                    let id = e.id.clone();
                                                    move |_| test(id.clone())
                                                },
                                                {t!("browser-endpoints-test")}
                                            }
                                            button {
                                                class: BTN_SECONDARY_SM,
                                                onclick: {
                                                    let e = e.clone();
                                                    move |_| form.set(Some(Form::Edit(e.clone())))
                                                },
                                                {t!("browser-endpoints-edit")}
                                            }
                                        }
                                    }
                                }
                                if let Some((id, r)) = tested() {
                                    if id == e.id {
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
                EndpointForm {
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
fn TestReport(result: Result<BrowserTestResult, String>) -> Element {
    match result {
        Err(m) => rsx! {
            p { class: "text-sm text-slate", "{m}" }
        },
        Ok(r) => rsx! {
            div { class: "rounded bg-gray-50 p-2 text-sm space-y-1",
                p { class: if r.ok { "font-medium text-green-800" } else { "font-medium text-alarm" },
                    if r.ok {
                        {t!("browser-endpoints-test-ok", ms : r.latency_ms)}
                    } else {
                        {t!("browser-endpoints-test-failed")}
                    }
                    if let Some(engine) = r.engine.clone() {
                        span { class: "ml-2 font-mono text-xs text-slate", "{engine}" }
                    }
                }
                ul {
                    for c in r.checks.clone() {
                        li {
                            key: "{c.check}",
                            class: if c.ok { "text-green-800" } else { "text-alarm" },
                            if c.ok {
                                "✓ "
                            } else {
                                "✗ "
                            }
                            {resolve(&format!("browser-endpoints-check-{}", c.check), None)}
                            " · {c.ms} ms"
                            if let Some(d) = c.detail.clone() {
                                span { class: "ml-1 font-mono text-xs", "({d})" }
                            }
                        }
                    }
                }
            }
        },
    }
}

#[component]
fn EndpointForm(form: Form, on_done: EventHandler<()>, on_cancel: EventHandler<()>) -> Element {
    let current = match &form {
        Form::Edit(e) => Some(e.clone()),
        Form::New => None,
    };
    let init = current.clone();
    let mut name = use_signal(|| init.as_ref().map(|e| e.name.clone()).unwrap_or_default());
    let mut url = use_signal(|| init.as_ref().map(|e| e.cdp_url.clone()).unwrap_or_default());
    let mut max = use_signal(|| init.as_ref().map_or(2, |e| e.max_connections).to_string());
    let mut is_default = use_signal(|| init.as_ref().is_some_and(|e| e.is_default));
    let token = use_signal(|| SecretDraft::from_view(init.as_ref().and_then(|e| e.token.clone())));
    let mut error = use_signal(|| None::<String>);
    let id = current.as_ref().map(|e| e.id.clone());
    let id_for_delete = id.clone();
    let save = move |_| {
        let input = BrowserEndpointInput {
            name: name(),
            cdp_url: url(),
            token: token.read().to_input(),
            max_connections: max().trim().parse().unwrap_or(0),
            is_default: is_default(),
        };
        let id = id.clone();
        spawn(async move {
            match api::site_recipes::endpoint_save(id.as_deref(), &input).await {
                Ok(_) => on_done.call(()),
                Err(e) => error.set(Some(localize(&e))),
            }
        });
    };
    rsx! {
        div { class: "rounded border border-line p-3 space-y-2",
            label { class: "block text-xs text-gray-600", {t!("browser-endpoints-name")} }
            input {
                class: INPUT,
                value: "{name}",
                oninput: move |e| name.set(e.value()),
            }
            label { class: "block text-xs text-gray-600", {t!("browser-endpoints-url")} }
            input {
                class: "{INPUT} font-mono",
                placeholder: "wss://browser.example.org",
                value: "{url}",
                oninput: move |e| url.set(e.value()),
            }
            p { class: FIELD_HELP, {t!("browser-endpoints-url-help")} }
            SecretField {
                label: t!("browser-endpoints-token").to_string(),
                draft: token,
                can_manage: true,
            }
            label { class: "block text-xs text-gray-600", {t!("browser-endpoints-max")} }
            input {
                class: INPUT,
                r#type: "number",
                min: "1",
                max: "16",
                value: "{max}",
                oninput: move |e| max.set(e.value()),
            }
            label { class: "inline-flex items-center gap-2 text-sm",
                input {
                    r#type: "checkbox",
                    checked: is_default(),
                    onchange: move |e| is_default.set(e.checked()),
                }
                {t!("browser-endpoints-default")}
            }
            if let Some(m) = error() {
                p { class: FIELD_ERROR, "{m}" }
            }
            div { class: "flex flex-wrap gap-2",
                button { class: BTN_PRIMARY_SM, onclick: save, {t!("browser-endpoints-save")} }
                button {
                    class: BTN_SECONDARY_SM,
                    onclick: move |_| on_cancel.call(()),
                    {t!("browser-endpoints-cancel")}
                }
                if let Some(id) = id_for_delete.clone() {
                    button {
                        class: "{BTN_DANGER} ml-auto",
                        onclick: move |_| {
                            let id = id.clone();
                            spawn(async move {
                                match api::site_recipes::endpoint_delete(&id).await {
                                    Ok(()) => on_done.call(()),
                                    Err(e) => error.set(Some(localize(&e))),
                                }
                            });
                        },
                        {t!("browser-endpoints-delete")}
                    }
                }
            }
        }
    }
}
