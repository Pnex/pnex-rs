//! Connectors (connectors.md D791–D797): saved access to an outside database or API,
//! read only, or a webhook other systems call; secrets in the vault. Org admins only,
//! the server checks each right.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::connectors::{Connector, ConnectorKind};

use super::code_label;
use super::connector_form::{kind_label, ConnectorForm};
use crate::api;
use crate::api::error_i18n::localize;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::pager::{page_slice, ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::modal::{Modal, MODAL_FOOTER};
use crate::components::ui::{
    BTN_PRIMARY, BTN_SECONDARY_SM, FIELD_ERROR, FIELD_HELP, FIELD_LABEL, INPUT,
};
use crate::state::access::can;
use crate::state::toasts;

/// Where a connector points, in the words of its kind.
fn address(c: &Connector) -> String {
    match c.kind {
        ConnectorKind::Postgres | ConnectorKind::Mysql => {
            format!("{}:{}/{}", c.host, c.port, c.database)
        }
        ConnectorKind::Rest => {
            let scheme = if c.tls { "https" } else { "http" };
            let path = c.rest.as_ref().map(|r| r.base_path.as_str()).unwrap_or("");
            format!("{scheme}://{}:{}{path}", c.host, c.port)
        }
        ConnectorKind::Webhook => t!("pipe-connector-receives").to_string(),
    }
}

/// The address a sender calls.
fn hook_url(id: &str) -> String {
    format!("{}/api/v1/hooks/{id}", crate::api::config::api_base())
}

/// A secret just generated, shown once.
#[derive(Clone, PartialEq)]
struct Revealed {
    name: String,
    url: String,
    secret: String,
}

/// Generates a new secret for `c` and shows it once.
fn generate(
    c: &Connector,
    mut revealed: Signal<Option<Revealed>>,
    mut error: Signal<Option<String>>,
    mut reload: Signal<u32>,
) {
    let (id, name) = (c.id.clone(), c.name.clone());
    spawn(async move {
        match api::pipelines::connector_secret(&id).await {
            Ok(s) => {
                revealed.set(Some(Revealed {
                    name,
                    url: hook_url(&id),
                    secret: s.secret,
                }));
                // The row now holds a secret: the next one asks first.
                reload.with_mut(|r| *r += 1);
            }
            Err(e) => error.set(Some(localize(&e))),
        }
    });
}

#[component]
pub(super) fn ConnectorList(reload: Signal<u32>, adding: Signal<bool>) -> Element {
    let mut editing = use_signal(|| None::<Connector>);
    let mut removing = use_signal(|| None::<Connector>);
    let mut regenerating = use_signal(|| None::<Connector>);
    let mut revealed = use_signal(|| None::<Revealed>);
    let error = use_signal(|| None::<String>);
    let page = use_signal(|| 0i64);
    let list = use_resource(move || async move {
        let _ = reload();
        api::pipelines::connectors().await
    });
    let (state, all) = match &*list.read() {
        None => (None, Vec::new()),
        Some(Ok(v)) => (Some(Ok(())), v.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), Vec::new()),
    };
    let count = all.len() as i64;
    let current = page().clamp(0, (count - 1).max(0) / PAGE_SIZE);
    let rows = page_slice(&all, current);
    let is_empty = all.is_empty();
    let columns = vec![
        Column::new(t!("pipe-col-name").to_string(), |c: &Connector| {
            rsx! { "{c.name}" }
        })
        // Breaks a word only when it cannot fit; the table scrolls past the pinned actions.
        .with_td_class("font-medium text-gray-900 break-words"),
        Column::new(t!("pipe-col-kind").to_string(), |c: &Connector| {
            rsx! { "{kind_label(c.kind)}" }
        }),
        Column::new(t!("pipe-connector-address").to_string(), |c: &Connector| {
            rsx! {
                span { class: "break-words", "{address(c)}" }
            }
        })
        .secondary(),
        Column::new(String::new(), move |c: &Connector| {
            let test_id = c.id.clone();
            let test = move |e: MouseEvent| {
                e.stop_propagation();
                let id = test_id.clone();
                spawn(async move {
                    match api::pipelines::test_connector(&id).await {
                        Ok(r) if r.ok => toasts::success(t!("pipe-connector-test-ok").to_string()),
                        Ok(r) => toasts::error(code_label(
                            r.code.as_deref().unwrap_or("connector-unreachable"),
                            None,
                        )),
                        Err(e) => toasts::error(localize(&e)),
                    }
                });
            };
            let for_secret = c.clone();
            let secret = move |e: MouseEvent| {
                e.stop_propagation();
                if for_secret.has_password {
                    regenerating.set(Some(for_secret.clone()));
                } else {
                    generate(&for_secret, revealed, error, reload);
                }
            };
            let target = c.clone();
            let remove = move |e: MouseEvent| {
                e.stop_propagation();
                removing.set(Some(target.clone()));
            };
            let hook = c.kind == ConnectorKind::Webhook;
            rsx! {
                span { class: "inline-flex flex-col items-stretch gap-2 md:flex-row md:items-center",
                    if can("connectors:test") {
                        button { class: BTN_SECONDARY_SM, onclick: test, {t!("pipe-connector-test")} }
                    }
                    if hook && can("connectors:write") {
                        button { class: BTN_SECONDARY_SM, onclick: secret, {t!("pipe-connector-generate")} }
                    }
                    if can("connectors:delete") {
                        button { class: BTN_SECONDARY_SM, onclick: remove, {t!("pipe-delete")} }
                    }
                }
            }
        })
        .header_sr_only()
        .actions(),
    ];
    let form_open = adding() || editing().is_some();
    rsx! {
        div { class: "space-y-3",
            p { class: FIELD_HELP, {t!("pipe-connector-help")} }
            if let Some(e) = error() {
                p { class: FIELD_ERROR, role: "alert", "{e}" }
            }
            ListStates {
                state,
                is_empty,
                empty_message: t!("pipe-connector-empty").to_string(),
                div { class: "space-y-4",
                    DataTable {
                        columns,
                        rows,
                        row_key: RowKey::new(|c: &Connector| c.id.clone()),
                        on_row_click: move |id: String| {
                            if can("connectors:write") {
                                editing.set(all.iter().find(|c| c.id == id).cloned());
                            }
                        },
                    }
                    ListPager { count, page }
                }
            }
        }
        if form_open {
            ConnectorForm {
                key: "{editing().map(|c| c.id).unwrap_or_default()}",
                initial: editing(),
                on_close: move |_| {
                    adding.set(false);
                    editing.set(None);
                },
                on_saved: move |saved: Connector| {
                    let created = editing().is_none();
                    adding.set(false);
                    editing.set(None);
                    reload.with_mut(|r| *r += 1);
                    // A new webhook leaves with its address and a secret.
                    if created && saved.kind == ConnectorKind::Webhook && !saved.has_password {
                        generate(&saved, revealed, error, reload);
                    }
                },
            }
        }
        if let Some(c) = removing() {
            ConfirmDialog {
                title: t!("pipe-connector-delete-title").to_string(),
                message: t!("pipe-connector-delete-message", name : c.name.clone()).to_string(),
                confirm_label: t!("pipe-delete").to_string(),
                on_cancel: move |_| removing.set(None),
                on_confirm: move |_| {
                    let id = c.id.clone();
                    removing.set(None);
                    let mut error = error;
                    spawn(async move {
                        match api::pipelines::delete_connector(&id).await {
                            Ok(()) => reload.with_mut(|r| *r += 1),
                            Err(e) => error.set(Some(localize(&e))),
                        }
                    });
                },
            }
        }
        if let Some(c) = regenerating() {
            ConfirmDialog {
                title: t!("pipe-connector-generate-title").to_string(),
                message: t!("pipe-connector-generate-message", name : c.name.clone()).to_string(),
                confirm_label: t!("pipe-connector-generate").to_string(),
                on_cancel: move |_| regenerating.set(None),
                on_confirm: move |_| {
                    regenerating.set(None);
                    generate(&c, revealed, error, reload);
                },
            }
        }
        if let Some(r) = revealed() {
            SecretOnce { revealed: r, on_close: move |_| revealed.set(None) }
        }
    }
}

/// One value to hand over, selectable and copyable.
#[component]
fn CopyField(id: String, label: String, value: String) -> Element {
    let copied = value.clone();
    rsx! {
        div {
            label { class: FIELD_LABEL, r#for: "{id}", "{label}" }
            div { class: "flex gap-2",
                input {
                    id: "{id}",
                    class: "{INPUT} font-mono",
                    readonly: true,
                    value: "{value}",
                }
                button {
                    r#type: "button",
                    class: BTN_SECONDARY_SM,
                    onclick: move |_| {
                        crate::util::copy_text(&copied);
                        toasts::success(t!("pipe-connector-copied").to_string());
                    },
                    {t!("pipe-connector-copy")}
                }
            }
        }
    }
}

/// The secret just generated and the address of the webhook: shown this once.
#[component]
fn SecretOnce(revealed: Revealed, on_close: EventHandler<()>) -> Element {
    rsx! {
        Modal {
            title: t!("pipe-connector-secret-title", name : revealed.name.clone()).to_string(),
            max_width: "max-w-lg".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "space-y-4",
                p { class: "text-sm text-ink", {t!("pipe-connector-secret-once")} }
                CopyField {
                    id: "hook-url",
                    label: t!("pipe-connector-url").to_string(),
                    value: revealed.url.clone(),
                }
                CopyField {
                    id: "hook-secret",
                    label: t!("pipe-connector-secret").to_string(),
                    value: revealed.secret.clone(),
                }
                div { class: MODAL_FOOTER,
                    button {
                        r#type: "button",
                        class: BTN_PRIMARY,
                        onclick: move |_| on_close.call(()),
                        {t!("pipe-connector-done")}
                    }
                }
            }
        }
    }
}
