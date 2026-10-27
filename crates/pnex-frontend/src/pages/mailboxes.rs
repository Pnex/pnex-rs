//! Page `/mailboxes`: the mailboxes PneX polls (mail.md D453–D454).
//!
//! CRUD socle (ListLayout + FilterBar + ListStates + DataTable + ListPager,
//! FormDialog, ConfirmDialog). A mailbox is personal (the caller's),
//! a service account's, or shared with teams; the last two need
//! `mail.shared:write`. Never shows a password, only whether one is stored.
//! The server re-checks every right; the UI only hides what it would refuse.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::mail::{Mailbox, MailboxInput, MailboxState, MailboxTestResult, IMAPS_PORT};

use crate::api;
use crate::api::error::ApiError;
use crate::api::iam::{ServiceAccount, Team};
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::filters::{FilterBar, SearchInput};
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::{page_slice, ListPager};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;
use crate::components::modal::{Modal, MODAL_FOOTER};
use crate::components::secret_field::{can_manage_secrets, SecretDraft, SecretField};
use crate::components::ui::{State, StateMark, BTN_SECONDARY, FIELD_HELP, FIELD_LABEL, INPUT};
use crate::state::{org, session, toasts};

const SHARED_WRITE: &str = "mail.shared:write";
const ROW_BTN: &str = "px-3 py-1 text-sm border border-gray-300 rounded-lg hover:bg-gray-50";

/// Who a mailbox belongs to, as picked in the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ownership {
    Mine,
    Service,
    Shared,
}

/// The caller changes this mailbox (same rule as `Access::manages`).
fn manages(m: &Mailbox, me: i64, shared_write: bool) -> bool {
    match &m.owner {
        Some(o) if o.user_id == me => true,
        Some(o) => o.service && shared_write,
        None => shared_write,
    }
}

/// The caller pauses or deletes it (`Access::moderates`).
fn moderates(m: &Mailbox, me: i64, shared_write: bool) -> bool {
    shared_write || manages(m, me, shared_write)
}

fn search_hit(m: &Mailbox, term: &str) -> bool {
    let term = term.trim().to_lowercase();
    term.is_empty()
        || m.name.to_lowercase().contains(&term)
        || m.address.to_lowercase().contains(&term)
}

#[component]
pub fn Mailboxes() -> Element {
    let mut reload = use_signal(|| 0u32);
    let search = use_signal(String::new);
    let mut page = use_signal(|| 0i64);
    // None = closed, Some(None) = create, Some(Some(m)) = edit.
    let mut editing = use_signal(|| None::<Option<Mailbox>>);
    let mut delete_target = use_signal(|| None::<(String, String)>);
    let mut tested = use_signal(|| None::<(String, Result<MailboxTestResult, ApiError>)>);

    let can_write = crate::state::access::can("mail:write");
    let shared_write = crate::state::access::can(SHARED_WRITE);
    let me = session::user().map(|u| u.id).unwrap_or_default();

    let list = use_resource(move || {
        let _ = reload();
        async move { api::mailboxes::list().await }
    });
    let teams = use_resource(|| async { api::iam::teams().await.unwrap_or_default() });
    let team_list = teams.cloned().unwrap_or_default();

    let (state, is_empty, count, rows) = match &*list.value().read() {
        None => (None, false, 0, Vec::new()),
        Some(Ok(all)) => {
            let hits: Vec<Mailbox> = all
                .iter()
                .filter(|m| search_hit(m, &search()))
                .cloned()
                .collect();
            (
                Some(Ok(())),
                all.is_empty(),
                hits.len() as i64,
                page_slice(&hits, page()),
            )
        }
        Some(Err(err)) => (Some(Err(err.clone())), false, 0, Vec::new()),
    };

    let toggle = move |id: String, paused: bool| {
        spawn(async move {
            match api::mailboxes::set_paused(&id, paused).await {
                Ok(_) => reload.with_mut(|r| *r += 1),
                Err(err) => toasts::error(err),
            }
        });
    };
    let run_test = move |m: Mailbox| {
        spawn(async move {
            let result = api::mailboxes::test(&m.id).await;
            tested.set(Some((m.name.clone(), result)));
            reload.with_mut(|r| *r += 1);
        });
    };

    let owner_teams = team_list.clone();
    let columns = vec![
        Column::new(t!("mailboxes-col-name").to_string(), |m: &Mailbox| {
            rsx! {
                if m.readable {
                    Link {
                        to: crate::app::Route::MailboxMessages { id: m.id.clone() },
                        class: "text-sm font-medium text-signal-700 hover:underline",
                        {m.name.clone()}
                    }
                } else {
                    div { class: "text-sm font-medium text-gray-900", {m.name.clone()} }
                }
                div { class: "text-xs text-gray-500 break-all", {m.address.clone()} }
            }
        }),
        Column::new(t!("mailboxes-col-owner").to_string(), move |m: &Mailbox| {
            let label = owner_label(m, me, &owner_teams);
            rsx! {
                span { class: "text-sm text-gray-700", {label} }
            }
        })
        .secondary(),
        Column::new(t!("mailboxes-col-state").to_string(), |m: &Mailbox| {
            rsx! {
                MailboxStateCell { mailbox: m.clone() }
            }
        }),
        Column::new(t!("common-actions").to_string(), move |m: &Mailbox| {
            let (edit, test, del) = (m.clone(), m.clone(), (m.id.clone(), m.name.clone()));
            let (id, paused) = (m.id.clone(), m.state == MailboxState::Paused);
            let can_manage = can_write && manages(m, me, shared_write);
            let can_moderate = can_write && moderates(m, me, shared_write);
            rsx! {
                div { class: "flex flex-wrap gap-2",
                    if can_manage {
                        button {
                            class: ROW_BTN,
                            onclick: move |_| editing.set(Some(Some(edit.clone()))),
                            {t!("mailboxes-edit")}
                        }
                        button { class: ROW_BTN, onclick: move |_| run_test(test.clone()),
                            {t!("mailboxes-test")}
                        }
                    }
                    if can_moderate {
                        button {
                            class: ROW_BTN,
                            onclick: move |_| toggle(id.clone(), !paused),
                            if paused {
                                {t!("mailboxes-resume")}
                            } else {
                                {t!("mailboxes-pause")}
                            }
                        }
                        button {
                            class: DANGER_BTN,
                            onclick: move |_| delete_target.set(Some(del.clone())),
                            icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                            {t!("common-delete")}
                        }
                    }
                }
            }
        })
        .actions(),
    ];

    let form = editing.read().clone();
    let delete_modal = delete_target.read().clone();
    let test_modal = tested.read().clone();

    rsx! {
        ListLayout {
            title: t!("mailboxes-title").to_string(),
            subtitle: Some(t!("mailboxes-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write,
            add_label: Some(t!("mailboxes-new").to_string()),
            on_add: move |_| editing.set(Some(None)),
            if org::current().is_none() {
                p { class: "text-gray-500 text-center py-12", {t!("orgs-empty")} }
            } else {
                FilterBar {
                    SearchInput {
                        placeholder: t!("mailboxes-search-placeholder").to_string(),
                        value: search,
                        page: Some(page),
                        on_submit: move |_| page.set(0),
                    }
                }
                ListStates {
                    state,
                    is_empty,
                    empty_message: t!("mailboxes-empty").to_string(),
                    empty_detail: rsx! {
                        p { class: "text-sm text-gray-500 mt-2", {t!("mailboxes-empty-hint")} }
                    },
                    div { class: "space-y-4",
                        DataTable {
                            columns,
                            rows,
                            row_key: RowKey::new(|m: &Mailbox| m.id.clone()),
                        }
                        ListPager { count, page }
                    }
                }
            }
            if let Some(existing) = form {
                MailboxForm {
                    key: "{existing.as_ref().map(|m| m.id.clone()).unwrap_or_default()}",
                    existing,
                    shared_write,
                    teams: team_list.clone(),
                    on_close: move |_| editing.set(None),
                    on_saved: move |_| {
                        editing.set(None);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
            if let Some((name, result)) = test_modal {
                TestResultDialog {
                    name,
                    result,
                    on_close: move |_| tested.set(None),
                }
            }
            if let Some((id, name)) = delete_modal {
                ConfirmDialog {
                    title: t!("mailboxes-confirm-delete-title"),
                    message: t!(
                        "common-quoted-message", name : name.clone(), message :
                        t!("mailboxes-confirm-delete-message")
                    ),
                    confirm_label: t!("common-delete"),
                    on_confirm: move |_| {
                        let id = id.clone();
                        delete_target.set(None);
                        spawn(async move {
                            match api::mailboxes::delete(&id).await {
                                Ok(()) => {
                                    toasts::success("toast-mailboxes-deleted");
                                    reload.with_mut(|r| *r += 1);
                                }
                                Err(err) => toasts::error(err),
                            }
                        });
                    },
                    on_cancel: move |_| delete_target.set(None),
                }
            }
        }
    }
}

/// Render-scope owner label: me, a member, a service account, or the teams.
fn owner_label(m: &Mailbox, me: i64, teams: &[Team]) -> String {
    match &m.owner {
        Some(o) if o.user_id == me => t!("mailboxes-owner-me").to_string(),
        Some(o) if o.service => t!("mailboxes-owner-service", name : o.name.clone()).to_string(),
        Some(o) => o.name.clone(),
        None => {
            let names: Vec<String> = m
                .teams
                .iter()
                .map(|id| {
                    teams
                        .iter()
                        .find(|t| &t.id == id)
                        .map(|t| t.name.clone())
                        .unwrap_or_else(|| id.clone())
                })
                .collect();
            if names.is_empty() {
                t!("mailboxes-owner-shared-admins").to_string()
            } else {
                t!("mailboxes-owner-shared", teams : names.join(", ")).to_string()
            }
        }
    }
}

#[component]
fn MailboxStateCell(mailbox: Mailbox) -> Element {
    let (mark, label) = match mailbox.state {
        MailboxState::Active => (State::Ok, t!("mailboxes-state-active")),
        MailboxState::Paused => (State::Idle, t!("mailboxes-state-paused")),
        MailboxState::Error => (State::Alarm, t!("mailboxes-state-error")),
    };
    let error = mailbox
        .last_error
        .as_ref()
        .map(|e| crate::api::error_i18n::localize_code(&e.code, Some(&e.args), &e.code));
    let synced = mailbox.last_sync_at.as_deref().map(date_label);
    rsx! {
        StateMark { state: mark, label: label.to_string() }
        if let Some(error) = error {
            div { class: "text-xs text-alarm mt-0.5", {error} }
        }
        if let Some(synced) = synced {
            div { class: "text-xs text-gray-500 mt-0.5", {t!("mailboxes-last-sync", date : synced)} }
        } else if !mailbox.has_password {
            div { class: "text-xs text-gray-500 mt-0.5", {t!("err-mail-no-password")} }
        }
    }
}

#[component]
fn TestResultDialog(
    name: String,
    result: Result<MailboxTestResult, ApiError>,
    on_close: Callback<()>,
) -> Element {
    let body = match &result {
        Ok(r) if r.ok => rsx! {
            StateMark {
                state: State::Ok,
                label: t!("mailboxes-test-ok", ms : r.latency_ms as i64).to_string(),
            }
            ul { class: "mt-3 space-y-1 text-sm text-gray-700",
                for f in r.folders.clone() {
                    li { key: "{f.folder}",
                        {
                            t!("mailboxes-test-folder", folder : f.folder.clone(), count : f.messages as i64)
                        }
                    }
                }
            }
            if let Some(junk) = r.junk_folder.clone() {
                p { class: "mt-3 text-sm text-warn-text",
                    {t!("mailboxes-test-junk", folder : junk)}
                }
            }
        },
        Ok(r) => {
            let reason = r
                .error
                .as_ref()
                .map(|e| crate::api::error_i18n::localize_code(&e.code, Some(&e.args), &e.code))
                .unwrap_or_default();
            rsx! {
                StateMark {
                    state: State::Alarm,
                    label: t!("mailboxes-test-failed").to_string(),
                }
                p { class: "mt-2 text-sm text-gray-700", {reason} }
            }
        }
        Err(err) => rsx! {
            StateMark {
                state: State::Alarm,
                label: t!("mailboxes-test-failed").to_string(),
            }
            p { class: "mt-2 text-sm text-gray-700", {crate::api::error_i18n::localize(err)} }
        },
    };
    rsx! {
        Modal {
            title: t!("mailboxes-test-title", name : name.clone()),
            max_width: "max-w-md".to_string(),
            on_close,
            div { class: "space-y-4",
                div { {body} }
                div { class: MODAL_FOOTER,
                    button {
                        class: BTN_SECONDARY,
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {t!("common-close")}
                    }
                }
            }
        }
    }
}

/// Create or edit a mailbox. The owner is fixed once created.
#[component]
fn MailboxForm(
    existing: Option<Mailbox>,
    shared_write: bool,
    teams: Vec<Team>,
    on_close: Callback<()>,
    on_saved: Callback<()>,
) -> Element {
    let edit = existing.clone();
    let is_edit = edit.is_some();
    let field = move |f: fn(&Mailbox) -> String| edit.as_ref().map(f).unwrap_or_default();
    let name = use_signal(|| field(|m| m.name.clone()));
    let address = use_signal(|| field(|m| m.address.clone()));
    let host = use_signal(|| field(|m| m.imap_host.clone()));
    let port = use_signal(|| {
        existing
            .as_ref()
            .map(|m| m.imap_port)
            .unwrap_or(IMAPS_PORT)
            .to_string()
    });
    let user = use_signal(|| field(|m| m.imap_user.clone()));
    let folders = use_signal(|| {
        existing
            .as_ref()
            .map(|m| m.folders.join(", "))
            .unwrap_or_else(|| "INBOX".into())
    });
    let retention = use_signal(|| {
        existing
            .as_ref()
            .map(|m| m.retention_days)
            .unwrap_or(pnex_core::mail::RETENTION_DAYS_DEFAULT)
            .to_string()
    });
    let ownership0 = match existing.as_ref().and_then(|m| m.owner.as_ref()) {
        None if existing.is_some() => Ownership::Shared,
        Some(o) if o.service => Ownership::Service,
        _ => Ownership::Mine,
    };
    let mut ownership = use_signal(|| ownership0);
    let mut service = use_signal(|| {
        existing
            .as_ref()
            .and_then(|m| m.owner.as_ref())
            .filter(|o| o.service)
            .map(|o| o.user_id)
    });
    let picked_teams = use_signal(|| {
        existing
            .as_ref()
            .map(|m| m.teams.clone())
            .unwrap_or_default()
    });
    let password = use_signal(SecretDraft::default);
    let has_password = existing.as_ref().is_some_and(|m| m.has_password);
    let can_manage = can_manage_secrets();
    let mut busy = use_signal(|| false);
    let mut form_error = use_signal(|| None::<ApiError>);
    let accounts = use_resource(move || async move {
        if shared_write {
            api::iam::service_accounts().await.unwrap_or_default()
        } else {
            Vec::new()
        }
    });
    let account_list: Vec<ServiceAccount> = accounts.cloned().unwrap_or_default();

    let valid = !name().trim().is_empty()
        && !address().trim().is_empty()
        && !host().trim().is_empty()
        && !user().trim().is_empty()
        && (ownership() != Ownership::Service || service().is_some());
    let edit_id = existing.as_ref().map(|m| m.id.clone());
    let version = existing.as_ref().map(|m| m.version);
    let paused = existing
        .as_ref()
        .is_some_and(|m| m.state == MailboxState::Paused);

    let submit = move |_| {
        let input = MailboxInput {
            name: name().trim().to_string(),
            address: address().trim().to_string(),
            shared: ownership() == Ownership::Shared,
            owner_user_id: service().filter(|_| ownership() == Ownership::Service),
            teams: if ownership() == Ownership::Shared {
                picked_teams()
            } else {
                Vec::new()
            },
            imap_host: host().trim().to_string(),
            imap_port: port().trim().parse().ok(),
            imap_user: user().trim().to_string(),
            password: password().to_input(),
            folders: folders()
                .split(',')
                .map(|f| f.trim().to_string())
                .filter(|f| !f.is_empty())
                .collect(),
            retention_days: retention().trim().parse().ok(),
            paused,
            expected_version: version,
        };
        let id = edit_id.clone();
        busy.set(true);
        spawn(async move {
            let result = match id {
                Some(id) => api::mailboxes::update(&id, &input).await,
                None => api::mailboxes::create(&input).await,
            };
            busy.set(false);
            match result {
                Ok(_) => {
                    toasts::success("toast-saved");
                    on_saved.call(());
                }
                Err(err) => form_error.set(Some(err)),
            }
        });
    };

    rsx! {
        FormDialog {
            title: if is_edit { t!("mailboxes-edit-title").to_string() } else { t!("mailboxes-new").to_string() },
            submit_label: if is_edit { t!("common-save").to_string() } else { t!("common-add").to_string() },
            on_close,
            on_submit: submit,
            busy: busy(),
            valid,
            max_width: "max-w-lg".to_string(),
            TextField {
                id: "mailbox-name",
                label: t!("mailboxes-field-name"),
                value: name,
            }
            TextField {
                id: "mailbox-address",
                label: t!("mailboxes-field-address"),
                value: address,
                kind: "email",
            }
            if shared_write && !is_edit {
                div {
                    label { r#for: "mailbox-ownership", class: FIELD_LABEL,
                        {t!("mailboxes-field-owner")}
                    }
                    select {
                        id: "mailbox-ownership",
                        class: "{INPUT} w-full",
                        onchange: move |e| {
                            ownership
                                .set(
                                    match e.value().as_str() {
                                        "service" => Ownership::Service,
                                        "shared" => Ownership::Shared,
                                        _ => Ownership::Mine,
                                    },
                                )
                        },
                        option {
                            value: "mine",
                            selected: ownership() == Ownership::Mine,
                            {t!("mailboxes-owner-me")}
                        }
                        option {
                            value: "service",
                            selected: ownership() == Ownership::Service,
                            {t!("mailboxes-owner-kind-service")}
                        }
                        option {
                            value: "shared",
                            selected: ownership() == Ownership::Shared,
                            {t!("mailboxes-owner-kind-shared")}
                        }
                    }
                }
            }
            if ownership() == Ownership::Service && !is_edit {
                div {
                    label { r#for: "mailbox-service", class: FIELD_LABEL,
                        {t!("mailboxes-field-service")}
                    }
                    select {
                        id: "mailbox-service",
                        class: "{INPUT} w-full",
                        onchange: move |e| service.set(e.value().parse().ok()),
                        option { value: "", selected: service().is_none(),
                            {t!("mailboxes-pick-service")}
                        }
                        for a in account_list {
                            option {
                                key: "{a.user_id}",
                                value: "{a.user_id}",
                                selected: service() == Some(a.user_id),
                                {a.name.clone()}
                            }
                        }
                    }
                }
            }
            if ownership() == Ownership::Shared {
                TeamPicker { teams, picked: picked_teams }
            }
            div { class: "grid grid-cols-3 gap-3",
                div { class: "col-span-2",
                    TextField {
                        id: "mailbox-host",
                        label: t!("mailboxes-field-host"),
                        value: host,
                    }
                }
                TextField {
                    id: "mailbox-port",
                    label: t!("mailboxes-field-port"),
                    value: port,
                    kind: "number",
                }
            }
            TextField {
                id: "mailbox-user",
                label: t!("mailboxes-field-user"),
                value: user,
            }
            SecretField {
                label: t!("mailboxes-field-password").to_string(),
                draft: password,
                can_manage,
            }
            if has_password {
                p { class: FIELD_HELP, {t!("mailboxes-password-keep")} }
            }
            TextField {
                id: "mailbox-folders",
                label: t!("mailboxes-field-folders"),
                value: folders,
                help: t!("mailboxes-field-folders-help"),
            }
            TextField {
                id: "mailbox-retention",
                label: t!("mailboxes-field-retention"),
                value: retention,
                kind: "number",
            }
            if let Some(err) = form_error() {
                div { class: "bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                    {crate::api::error_i18n::localize(&err)}
                }
            }
        }
    }
}

#[component]
fn TextField(
    id: &'static str,
    label: String,
    mut value: Signal<String>,
    #[props(default = "text")] kind: &'static str,
    help: Option<String>,
) -> Element {
    rsx! {
        div {
            label { r#for: id, class: FIELD_LABEL, {label} }
            input {
                id,
                class: "{INPUT} w-full",
                r#type: kind,
                value: "{value}",
                oninput: move |e| value.set(e.value()),
            }
            if let Some(help) = help {
                p { class: FIELD_HELP, {help} }
            }
        }
    }
}

/// Teams that read a shared mailbox; none = the admins only.
#[component]
fn TeamPicker(teams: Vec<Team>, mut picked: Signal<Vec<String>>) -> Element {
    rsx! {
        fieldset {
            legend { class: FIELD_LABEL, {t!("mailboxes-field-teams")} }
            if teams.is_empty() {
                p { class: FIELD_HELP, {t!("mailboxes-no-teams")} }
            }
            div { class: "space-y-1",
                for team in teams {
                    label {
                        key: "{team.id}",
                        class: "flex items-center gap-2 text-sm text-gray-700",
                        input {
                            r#type: "checkbox",
                            checked: picked().contains(&team.id),
                            onchange: {
                                let id = team.id.clone();
                                move |e: FormEvent| {
                                    let on = e.checked();
                                    picked
                                        .with_mut(|p| {
                                            p.retain(|t| t != &id);
                                            if on {
                                                p.push(id.clone());
                                            }
                                        });
                                }
                            },
                        }
                        {team.name.clone()}
                    }
                }
            }
            p { class: FIELD_HELP, {t!("mailboxes-field-teams-help")} }
        }
    }
}
