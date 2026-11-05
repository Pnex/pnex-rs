//! Connectors (data-pipelines.md D352): saved access to an outside database, the
//! password in the vault. Org admins only, the server checks each right.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::connectors::{Connector, ConnectorInput, ConnectorKind};

use super::{code_label, tone, BADGE};
use crate::api;
use crate::api::error_i18n::localize;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::form::FormDialog;
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::secret_field::{can_manage_secrets, SecretDraft, SecretField};
use crate::components::ui::{BTN_SECONDARY_SM, FIELD_ERROR, FIELD_HELP, FIELD_LABEL, INPUT};
use crate::state::access::can;
use crate::state::toasts;

const DEFAULT_PORT: i32 = 5432;

#[component]
pub(super) fn ConnectorList(reload: Signal<u32>, adding: Signal<bool>) -> Element {
    let mut editing = use_signal(|| None::<Connector>);
    let mut removing = use_signal(|| None::<Connector>);
    let mut error = use_signal(|| None::<String>);
    let list = use_resource(move || async move {
        let _ = reload();
        api::pipelines::connectors().await
    });
    let (state, rows) = match &*list.read() {
        None => (None, Vec::new()),
        Some(Ok(v)) => (Some(Ok(())), v.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), Vec::new()),
    };
    let is_empty = rows.is_empty();
    let found = rows.clone();
    let columns = vec![
        Column::new(t!("pipe-col-name").to_string(), |c: &Connector| {
            rsx! { "{c.name}" }
        })
        .with_td_class("font-medium text-gray-900"),
        Column::new(t!("pipe-connector-address").to_string(), |c: &Connector| {
            rsx! { "{c.host}:{c.port}/{c.database}" }
        }),
        Column::new(t!("pipe-connector-tls").to_string(), |c: &Connector| {
            let label = if c.tls { t!("pipe-yes") } else { t!("pipe-no") };
            rsx! { "{label}" }
        }),
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
            let target = c.clone();
            let remove = move |e: MouseEvent| {
                e.stop_propagation();
                removing.set(Some(target.clone()));
            };
            rsx! {
                span { class: "inline-flex gap-2",
                    if can("connectors:test") {
                        button { class: BTN_SECONDARY_SM, onclick: test, {t!("pipe-connector-test")} }
                    }
                    if can("connectors:delete") {
                        button { class: BTN_SECONDARY_SM, onclick: remove, {t!("pipe-delete")} }
                    }
                }
            }
        })
        .header_sr_only(),
    ];
    let form_open = adding() || editing().is_some();
    rsx! {
        div { class: "space-y-3",
            p { class: FIELD_HELP,
                {t!("pipe-connector-help")}
                " "
                span { class: "{BADGE} {tone(false, false)}", "PostgreSQL" }
            }
            if let Some(e) = error() {
                p { class: FIELD_ERROR, "{e}" }
            }
            ListStates {
                state,
                is_empty,
                empty_message: t!("pipe-connector-empty").to_string(),
                DataTable {
                    columns,
                    rows,
                    row_key: RowKey::new(|c: &Connector| c.id.clone()),
                    on_row_click: move |id: String| {
                        if can("connectors:write") {
                            editing.set(found.iter().find(|c| c.id == id).cloned());
                        }
                    },
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
                on_saved: move |_| {
                    adding.set(false);
                    editing.set(None);
                    reload.with_mut(|r| *r += 1);
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
                    spawn(async move {
                        match api::pipelines::delete_connector(&id).await {
                            Ok(()) => reload.with_mut(|r| *r += 1),
                            Err(e) => error.set(Some(localize(&e))),
                        }
                    });
                },
            }
        }
    }
}

#[component]
fn ConnectorForm(
    initial: Option<Connector>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let start = initial.clone();
    let field = |f: fn(&Connector) -> String| start.as_ref().map(f).unwrap_or_default();
    let mut name = use_signal(|| field(|c| c.name.clone()));
    let mut host = use_signal(|| field(|c| c.host.clone()));
    let mut port = use_signal(|| start.as_ref().map_or(DEFAULT_PORT, |c| c.port).to_string());
    let mut database = use_signal(|| field(|c| c.database.clone()));
    let mut username = use_signal(|| field(|c| c.username.clone()));
    let mut tls = use_signal(|| start.as_ref().is_none_or(|c| c.tls));
    let password = use_signal(SecretDraft::default);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let id = initial.as_ref().map(|c| c.id.clone());
    let version = initial.as_ref().map(|c| c.version);
    let title = if initial.is_some() {
        t!("pipe-connector-edit")
    } else {
        t!("pipe-connector-add")
    };
    let input = move || ConnectorInput {
        name: name().trim().to_string(),
        kind: ConnectorKind::Postgres,
        host: host().trim().to_string(),
        port: port().trim().parse().unwrap_or(0),
        database: database().trim().to_string(),
        username: username().trim().to_string(),
        tls: tls(),
        password: password().to_input(),
        expected_version: version,
    };
    let submit = move |_| {
        let id = id.clone();
        let body = input();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match api::pipelines::save_connector(id.as_deref(), &body).await {
                Ok(_) => on_saved.call(()),
                Err(e) => error.set(Some(localize(&e))),
            }
            busy.set(false);
        });
    };
    rsx! {
        FormDialog {
            title: title.to_string(),
            submit_label: t!("pipe-save").to_string(),
            on_close: move |_| on_close.call(()),
            on_submit: submit,
            busy: busy(),
            valid: input().violation().is_none(),
            div { class: "space-y-3",
                div {
                    label { class: FIELD_LABEL, r#for: "conn-name", {t!("pipe-col-name")} }
                    input {
                        id: "conn-name",
                        class: INPUT,
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                div {
                    label { class: FIELD_LABEL, r#for: "conn-host", {t!("pipe-connector-host")} }
                    input {
                        id: "conn-host",
                        class: INPUT,
                        value: "{host}",
                        oninput: move |e| host.set(e.value()),
                    }
                }
                div {
                    label { class: FIELD_LABEL, r#for: "conn-port", {t!("pipe-connector-port")} }
                    input {
                        id: "conn-port",
                        class: INPUT,
                        value: "{port}",
                        oninput: move |e| port.set(e.value()),
                    }
                }
                div {
                    label { class: FIELD_LABEL, r#for: "conn-db", {t!("pipe-connector-database")} }
                    input {
                        id: "conn-db",
                        class: INPUT,
                        value: "{database}",
                        oninput: move |e| database.set(e.value()),
                    }
                }
                div {
                    label { class: FIELD_LABEL, r#for: "conn-user", {t!("pipe-connector-username")} }
                    input {
                        id: "conn-user",
                        class: INPUT,
                        value: "{username}",
                        oninput: move |e| username.set(e.value()),
                    }
                }
                SecretField {
                    label: t!("pipe-connector-password").to_string(),
                    draft: password,
                    can_manage: can_manage_secrets(),
                }
                if initial.as_ref().is_some_and(|c| c.has_password) {
                    p { class: FIELD_HELP, {t!("pipe-connector-password-keep")} }
                }
                label { class: "flex items-center gap-2 text-sm text-ink",
                    input {
                        r#type: "checkbox",
                        checked: tls(),
                        onchange: move |e| tls.set(e.checked()),
                    }
                    {t!("pipe-connector-tls")}
                }
                if !tls() {
                    p { class: FIELD_HELP, {t!("pipe-connector-tls-off")} }
                }
                if let Some(e) = error() {
                    p { class: FIELD_ERROR, "{e}" }
                }
            }
        }
    }
}
