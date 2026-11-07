//! Image registries tab (agent-environments.md D703): where environment images go. A
//! registry is read without its secrets; its read and write secrets are set through the
//! common secret field and never shown.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::api::error::ApiError;
use crate::components::crud::form::FormDialog;
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::secret_field::{can_manage_secrets, SecretDraft, SecretField};
use crate::components::ui::{BTN_PRIMARY, BTN_SECONDARY_SM, FIELD_LABEL, INPUT};
use crate::state::access;
use crate::state::toasts;
use pnex_core::image_registries::{ImageRegistry, ImageRegistryCheck, ImageRegistryInput};

fn check_label(check: &ImageRegistryCheck) -> String {
    match &check.code {
        _ if check.ok => t!("agents-reg-test-ok").to_string(),
        Some(code) => crate::api::error_i18n::localize_code(code, None, code),
        None => t!("agents-reg-test-failed").to_string(),
    }
}

#[component]
pub(super) fn RegistriesTab(reload: Signal<u32>) -> Element {
    // `Some(None)` = a new registry.
    let mut editing = use_signal(|| Option::<Option<ImageRegistry>>::None);
    let list = use_resource(move || async move {
        let _ = reload();
        api::hosted_agents::registries().await
    });
    let (state, is_empty, rows) = match &*list.value().read() {
        None => (None, false, Vec::new()),
        Some(Ok(p)) => (Some(Ok(())), p.count == 0, p.results.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), false, Vec::new()),
    };
    let can_write = access::can("image-registries:write");
    let can_test = access::can("image-registries:test");
    let columns = vec![
        Column::new(t!("agents-col-name").to_string(), |r: &ImageRegistry| {
            rsx! {
                div { class: "text-sm font-medium text-ink", {r.name.clone()} }
                div { class: "font-mono text-xs text-slate break-all", "{r.host}/{r.repository}" }
            }
        }),
        Column::new(t!("agents-reg-col-secrets").to_string(), |r: &ImageRegistry| {
            let read = if r.has_read_secret { t!("agents-reg-set") } else { t!("agents-reg-unset") };
            let write = if r.has_write_secret { t!("agents-reg-set") } else { t!("agents-reg-unset") };
            rsx! {
                div { class: "text-xs text-slate", {t!("agents-reg-read-state", state : read)} }
                div { class: "text-xs text-slate", {t!("agents-reg-write-state", state : write)} }
            }
        })
        .secondary(),
        Column::new(t!("common-actions").to_string(), move |r: &ImageRegistry| {
            let registry = r.clone();
            let id = r.id.clone();
            rsx! {
                div { class: "flex gap-1",
                    if can_test {
                        button {
                            class: BTN_SECONDARY_SM,
                            onclick: move |_| {
                                let id = id.clone();
                                spawn(async move {
                                    match api::hosted_agents::test_registry(&id).await {
                                        Ok(result) => {
                                            let passed = result.read.ok && w_ok(&result.write);
                                            let mut text = t!("agents-reg-test-read", result : check_label(& result.read)).to_string();
                                            if let Some(w) = result.write {
                                                text.push_str(" · ");
                                                text.push_str(&t!("agents-reg-test-write", result : check_label(& w)));
                                            }
                                            if passed {
                                                toasts::info(text);
                                            } else {
                                                toasts::error(text);
                                            }
                                        }
                                        Err(e) => toasts::error(e),
                                    }
                                });
                            },
                            {t!("agents-reg-test")}
                        }
                    }
                    if can_write {
                        button {
                            class: BTN_SECONDARY_SM,
                            onclick: move |_| editing.set(Some(Some(registry.clone()))),
                            {t!("agents-reg-edit")}
                        }
                    }
                }
            }
        })
        .actions(),
    ];
    rsx! {
        if let Some(current) = editing() {
            RegistryForm { current, editing, reload }
        }
        if can_write {
            div { class: "flex justify-end mb-4",
                button {
                    class: BTN_PRIMARY,
                    onclick: move |_| editing.set(Some(None)),
                    {t!("agents-reg-new")}
                }
            }
        }
        ListStates {
            state,
            is_empty,
            empty_message: t!("agents-reg-empty").to_string(),
            empty_detail: rsx! {
                p { class: "text-sm text-slate mt-2", {t!("agents-reg-empty-hint")} }
            },
            DataTable {
                columns,
                rows,
                row_key: RowKey::new(|r: &ImageRegistry| r.id.clone()),
            }
        }
    }
}

/// A write check that did not run counts as passed.
fn w_ok(write: &Option<ImageRegistryCheck>) -> bool {
    write.as_ref().is_none_or(|w| w.ok)
}

fn some_trimmed(s: String) -> Option<String> {
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[component]
fn RegistryForm(
    current: Option<ImageRegistry>,
    editing: Signal<Option<Option<ImageRegistry>>>,
    reload: Signal<u32>,
) -> Element {
    let mut editing = editing;
    let mut reload = reload;
    let init = current.clone();
    let name = use_signal(|| init.as_ref().map(|r| r.name.clone()).unwrap_or_default());
    let host = use_signal(|| init.as_ref().map(|r| r.host.clone()).unwrap_or_default());
    let repository = use_signal(|| {
        init.as_ref()
            .map(|r| r.repository.clone())
            .unwrap_or_default()
    });
    let mut tls = use_signal(|| init.as_ref().is_none_or(|r| r.tls));
    let read_user = use_signal(|| {
        init.as_ref()
            .and_then(|r| r.read_username.clone())
            .unwrap_or_default()
    });
    let write_user = use_signal(|| {
        init.as_ref()
            .and_then(|r| r.write_username.clone())
            .unwrap_or_default()
    });
    // Empty = keep the current secret: a registry is never read with its secrets.
    let read_secret = use_signal(SecretDraft::default);
    let write_secret = use_signal(SecretDraft::default);
    let mut busy = use_signal(|| false);
    let mut form_error = use_signal(|| Option::<ApiError>::None);
    let has_read = current.as_ref().is_some_and(|r| r.has_read_secret);
    let has_write = current.as_ref().is_some_and(|r| r.has_write_secret);
    let valid =
        !name().trim().is_empty() && !host().trim().is_empty() && !repository().trim().is_empty();
    let id = current.as_ref().map(|r| r.id.clone());
    let version = current.as_ref().map(|r| r.version);
    let title = if current.is_some() {
        t!("agents-reg-edit")
    } else {
        t!("agents-reg-new")
    };
    let field = |id: &'static str, key: &'static str, mut value: Signal<String>, mono: bool| {
        rsx! {
            div {
                label { r#for: id, class: FIELD_LABEL, {t!(key)} }
                input {
                    id,
                    class: if mono { "{INPUT} w-full font-mono" } else { "{INPUT} w-full" },
                    value: "{value}",
                    oninput: move |e| value.set(e.value()),
                }
            }
        }
    };
    rsx! {
        FormDialog {
            title: title.to_string(),
            submit_label: if current.is_some() { t!("common-save").to_string() } else { t!("common-create").to_string() },
            on_close: move |_| editing.set(None),
            busy: busy(),
            valid,
            on_submit: move |_| {
                let input = ImageRegistryInput {
                    name: name().trim().to_string(),
                    host: host().trim().to_string(),
                    repository: repository().trim().to_string(),
                    tls: tls(),
                    read_username: some_trimmed(read_user()),
                    read_secret: read_secret.read().to_input(),
                    write_username: some_trimmed(write_user()),
                    write_secret: write_secret.read().to_input(),
                    expected_version: version,
                };
                let id = id.clone();
                busy.set(true);
                spawn(async move {
                    let result = api::hosted_agents::save_registry(id.as_deref(), &input).await;
                    busy.set(false);
                    match result {
                        Ok(_) => {
                            editing.set(None);
                            reload += 1;
                        }
                        Err(e) => form_error.set(Some(e)),
                    }
                });
            },
            div { class: "space-y-4",
                {field("reg-name", "agents-field-name", name, false)}
                {field("reg-host", "agents-reg-field-host", host, true)}
                {field("reg-repository", "agents-reg-field-repository", repository, true)}
                label { class: "flex items-center gap-2 text-sm text-ink",
                    input {
                        r#type: "checkbox",
                        checked: tls(),
                        onchange: move |e| tls.set(e.checked()),
                    }
                    {t!("agents-reg-field-tls")}
                }
                {field("reg-read-user", "agents-reg-field-read-user", read_user, true)}
                SecretField {
                    label: t!("agents-reg-field-read-secret").to_string(),
                    draft: read_secret,
                    can_manage: can_manage_secrets(),
                }
                if has_read {
                    p { class: "text-xs text-slate", {t!("agents-reg-secret-keep")} }
                }
                {field("reg-write-user", "agents-reg-field-write-user", write_user, true)}
                SecretField {
                    label: t!("agents-reg-field-write-secret").to_string(),
                    draft: write_secret,
                    can_manage: can_manage_secrets(),
                }
                if has_write {
                    p { class: "text-xs text-slate", {t!("agents-reg-secret-keep")} }
                }
                p { class: "text-xs text-slate", {t!("agents-reg-secrets-hint")} }
                if let Some(err) = form_error() {
                    div { class: "bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                        {crate::api::error_i18n::localize(&err)}
                    }
                }
            }
        }
    }
}
