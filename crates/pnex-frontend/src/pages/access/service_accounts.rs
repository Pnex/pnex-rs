//! Service accounts tab: non-human principals of the org for external
//! applications (server side only), and their access keys. A secret is
//! shown once, right after its creation.

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::roles::use_add_request;
use super::AddRequest;
use crate::api;
use crate::api::iam::{AccessKey, CreatedKey, ServiceAccount};
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::DANGER_BTN;
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::modal::Modal;
use crate::components::ui::{BTN_PRIMARY_SM, BTN_SECONDARY_SM, FIELD_HELP, FIELD_LABEL, INPUT};
use crate::state::toasts;

/// Default lifetime of a new key, in days (the server allows 366 at most).
const DEFAULT_KEY_DAYS: i64 = 90;

#[component]
pub(crate) fn ServiceAccountsTab(
    reload: Signal<u32>,
    add: Signal<AddRequest>,
    can_write: bool,
) -> Element {
    let mut editing = use_signal(|| None::<Option<ServiceAccount>>);
    let mut keys_of = use_signal(|| None::<ServiceAccount>);
    let mut confirm = use_signal(|| None::<ServiceAccount>);
    use_add_request(add, move || editing.set(Some(None)));
    let accounts = use_resource(move || {
        let _ = reload();
        async move { api::iam::service_accounts().await }
    });
    let (state, is_empty, rows) = match &*accounts.value().read() {
        None => (None, false, Vec::new()),
        Some(Ok(r)) => (Some(Ok(())), r.is_empty(), r.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), false, Vec::new()),
    };
    let editing_now = editing.read().clone();
    let keys_now = keys_of.read().clone();
    let confirm_now = confirm.read().clone();

    let columns = vec![
        Column::new(
            t!("access-col-service-account").to_string(),
            |a: &ServiceAccount| {
                rsx! {
                    div { class: "text-sm font-medium text-ink", {a.name.clone()} }
                    if !a.description.is_empty() {
                        div { class: "text-xs text-slate", {a.description.clone()} }
                    }
                }
            },
        ),
        Column::new(
            t!("access-col-active-keys").to_string(),
            |a: &ServiceAccount| {
                rsx! {
                    span { class: "text-sm text-ink", {a.active_keys.to_string()} }
                }
            },
        ),
        Column::new(String::new(), move |a: &ServiceAccount| {
            let (keys, edit, remove) = (a.clone(), a.clone(), a.clone());
            rsx! {
                if can_write {
                    div { class: "flex justify-end gap-2",
                        button {
                            class: BTN_SECONDARY_SM,
                            r#type: "button",
                            onclick: move |_| keys_of.set(Some(keys.clone())),
                            {t!("access-keys")}
                        }
                        button {
                            class: BTN_SECONDARY_SM,
                            r#type: "button",
                            onclick: move |_| editing.set(Some(Some(edit.clone()))),
                            {t!("access-edit")}
                        }
                        button {
                            class: DANGER_BTN,
                            r#type: "button",
                            onclick: move |_| confirm.set(Some(remove.clone())),
                            {t!("common-delete")}
                        }
                    }
                }
            }
        }),
    ];

    rsx! {
        p { class: "{FIELD_HELP} mb-3", {t!("access-service-accounts-help")} }
        ListStates {
            state,
            is_empty,
            empty_message: t!("access-service-accounts-empty").to_string(),
            DataTable {
                columns,
                rows,
                row_key: RowKey::new(|a: &ServiceAccount| a.id.clone()),
            }
        }
        if let Some(account) = editing_now {
            AccountDialog {
                account,
                on_close: move |_| editing.set(None),
                on_saved: move |_| {
                    editing.set(None);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
        if let Some(account) = keys_now {
            KeysDialog {
                account,
                on_close: move |_| {
                    keys_of.set(None);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
        if let Some(account) = confirm_now {
            ConfirmDialog {
                title: t!("access-delete-service-account-title").to_string(),
                message: t!("access-delete-service-account-message", name : account.name.clone()).to_string(),
                confirm_label: t!("common-delete").to_string(),
                on_cancel: move |_| confirm.set(None),
                on_confirm: move |_| {
                    let id = account.id.clone();
                    confirm.set(None);
                    spawn(async move {
                        match api::iam::delete_service_account(&id).await {
                            Ok(()) => {
                                toasts::success("toast-access-service-account-deleted");
                                reload.with_mut(|r| *r += 1);
                            }
                            Err(e) => toasts::error(e),
                        }
                    });
                },
            }
        }
    }
}

#[component]
fn AccountDialog(
    account: Option<ServiceAccount>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let mut name = use_signal(|| account.as_ref().map(|a| a.name.clone()).unwrap_or_default());
    let mut description = use_signal(|| {
        account
            .as_ref()
            .map(|a| a.description.clone())
            .unwrap_or_default()
    });
    let mut busy = use_signal(|| false);
    let title = if account.is_some() {
        t!("access-edit-service-account")
    } else {
        t!("access-new-service-account")
    };
    let save = move |_| {
        let (n, d, target) = (name(), description(), account.clone());
        busy.set(true);
        spawn(async move {
            let res = match &target {
                None => api::iam::create_service_account(&n, &d).await,
                Some(a) => api::iam::update_service_account(&a.id, &n, &d).await,
            };
            busy.set(false);
            match res {
                Ok(_) => {
                    toasts::success("toast-access-service-account-saved");
                    on_saved.call(());
                }
                Err(e) => toasts::error(e),
            }
        });
    };
    rsx! {
        FormDialog {
            title: title.to_string(),
            submit_label: t!("common-save").to_string(),
            on_close: move |_| on_close.call(()),
            on_submit: save,
            busy: busy(),
            valid: !name().trim().is_empty(),
            div {
                label { class: FIELD_LABEL, r#for: "access-field-name", {t!("access-field-name")} }
                input {
                    id: "access-field-name",
                    class: INPUT,
                    value: "{name}",
                    placeholder: "site-app",
                    oninput: move |e| name.set(e.value()),
                }
            }
            div {
                label { class: FIELD_LABEL, r#for: "access-field-description",
                    {t!("access-field-description")}
                }
                input {
                    id: "access-field-description",
                    class: INPUT,
                    value: "{description}",
                    oninput: move |e| description.set(e.value()),
                }
            }
            p { class: FIELD_HELP, {t!("access-service-account-roles-help")} }
        }
    }
}

/// `YYYY-MM-DD` of a day `days` from today.
fn day_in(days: i64) -> String {
    (chrono::Local::now() + chrono::Duration::days(days))
        .format("%Y-%m-%d")
        .to_string()
}

/// Lifecycle label of a key: revoked, expired or active.
fn key_state(k: &AccessKey) -> String {
    let expired =
        chrono::DateTime::parse_from_rfc3339(&k.expires_at).is_ok_and(|e| e < chrono::Utc::now());
    if k.revoked_at.is_some() {
        t!("access-key-revoked").to_string()
    } else if expired {
        t!("access-key-expired").to_string()
    } else {
        t!("access-key-active").to_string()
    }
}

#[component]
fn KeysDialog(account: ServiceAccount, on_close: EventHandler<()>) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut expiry = use_signal(|| day_in(DEFAULT_KEY_DAYS));
    let mut created = use_signal(|| None::<CreatedKey>);
    let mut busy = use_signal(|| false);
    let account_id = account.id.clone();
    let list_id = account.id.clone();
    let keys = use_resource(move || {
        let _ = reload();
        let id = list_id.clone();
        async move { api::iam::access_keys(&id).await }
    });
    let rows = match &*keys.value().read() {
        Some(Ok(k)) => k.clone(),
        _ => Vec::new(),
    };
    let created_now = created.read().clone();
    let create_id = account_id.clone();
    let create = move |_| {
        let (id, day) = (create_id.clone(), expiry());
        busy.set(true);
        spawn(async move {
            // End of the chosen day, UTC.
            let res = api::iam::create_access_key(&id, &format!("{day}T23:59:59Z")).await;
            busy.set(false);
            match res {
                Ok(k) => {
                    created.set(Some(k));
                    reload.with_mut(|r| *r += 1);
                }
                Err(e) => toasts::error(e),
            }
        });
    };

    rsx! {
        Modal {
            title: t!("access-keys-title", name : account.name.clone()).to_string(),
            max_width: "max-w-2xl".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "space-y-4",
                if let Some(k) = created_now {
                    NewSecret { secret: k.secret.clone() }
                }
                div { class: "flex flex-wrap items-end gap-3",
                    div {
                        label { class: FIELD_LABEL, r#for: "access-key-expires",
                            {t!("access-key-expires")}
                        }
                        input {
                            id: "access-key-expires",
                            class: INPUT,
                            r#type: "date",
                            min: day_in(1),
                            max: day_in(366),
                            value: "{expiry}",
                            oninput: move |e| expiry.set(e.value()),
                        }
                    }
                    button {
                        class: BTN_PRIMARY_SM,
                        r#type: "button",
                        disabled: busy() || expiry().is_empty(),
                        onclick: create,
                        {t!("access-new-key")}
                    }
                }
                p { class: FIELD_HELP, {t!("access-keys-help")} }
                div { class: "divide-y divide-line rounded-md border border-line",
                    if rows.is_empty() {
                        p { class: "px-3 py-4 text-sm text-slate", {t!("access-keys-empty")} }
                    }
                    for k in rows {
                        KeyRow {
                            key: "{k.id}",
                            account_id: account_id.clone(),
                            k,
                            reload,
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn NewSecret(secret: String) -> Element {
    let copy = secret.clone();
    rsx! {
        div { class: "rounded-md border border-signal-200 bg-signal-50 p-3 space-y-2",
            p { class: "text-sm font-medium text-signal-800", {t!("access-secret-once")} }
            div { class: "flex items-center gap-2",
                code { class: "flex-1 break-all rounded bg-surface px-2 py-1 font-mono text-xs text-ink",
                    {secret}
                }
                button {
                    class: BTN_SECONDARY_SM,
                    r#type: "button",
                    onclick: move |_| {
                        crate::util::copy_text(&copy);
                        toasts::success("toast-access-secret-copied");
                    },
                    {t!("access-copy")}
                }
            }
            p { class: "text-xs text-signal-700", {t!("access-secret-server-only")} }
        }
    }
}

#[component]
fn KeyRow(account_id: String, k: AccessKey, reload: Signal<u32>) -> Element {
    let state = key_state(&k);
    let live = k.revoked_at.is_none();
    let last = k
        .last_used_at
        .as_deref()
        .map(date_label)
        .unwrap_or_else(|| t!("access-key-never-used").to_string());
    let key_id = k.id.clone();
    let revoke = move |_| {
        let (account, key) = (account_id.clone(), key_id.clone());
        spawn(async move {
            match api::iam::revoke_access_key(&account, &key).await {
                Ok(()) => {
                    toasts::success("toast-access-key-revoked");
                    reload.with_mut(|r| *r += 1);
                }
                Err(e) => toasts::error(e),
            }
        });
    };
    rsx! {
        div { class: "flex flex-wrap items-center gap-3 px-3 py-2 text-sm",
            code { class: "font-mono text-xs text-ink", "{k.prefix}…" }
            span { class: "text-slate", {state} }
            span { class: "text-slate",
                {t!("access-key-expires-on", date : date_label(& k.expires_at))}
            }
            span { class: "text-slate", {t!("access-key-last-used", date : last)} }
            if live {
                button {
                    class: "{DANGER_BTN} ml-auto",
                    r#type: "button",
                    onclick: revoke,
                    {t!("access-revoke")}
                }
            }
        }
    }
}
