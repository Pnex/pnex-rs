//! Markings and clearances tab (governance.md D798–D803): the compartments of the
//! org, who is cleared for which, for what purpose and until when, and the
//! journal. The server checks every right; the tab hides what the caller may not do.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::governance::{Clearance, ClearanceInput, Marking, MarkingInput};

use super::roles::use_add_request;
use super::{member_label, AddRequest};
use crate::api;
use crate::api::error_i18n::localize;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::DANGER_BTN;
use crate::components::crud::pager::{page_slice, ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ui::{BTN_SECONDARY_SM, FIELD_ERROR, FIELD_HELP, FIELD_LABEL, INPUT};
use crate::state::{access, org, toasts};

const SECTION_TITLE: &str = "text-base font-semibold text-ink";

#[component]
pub(crate) fn GovernanceTab(reload: Signal<u32>, add: Signal<AddRequest>) -> Element {
    let mut creating = use_signal(|| false);
    use_add_request(add, move || creating.set(true));
    rsx! {
        div { class: "space-y-10",
            MarkingsSection { reload }
            if access::can("governance:write") {
                ClearancesSection { reload }
            }
        }
        if creating() {
            MarkingForm {
                on_close: move |_| creating.set(false),
                on_saved: move |_| {
                    creating.set(false);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
    }
}

#[component]
fn MarkingsSection(reload: Signal<u32>) -> Element {
    let mut removing = use_signal(|| None::<Marking>);
    let page = use_signal(|| 0i64);
    let list = use_resource(move || {
        let _ = reload();
        async move { api::governance::markings().await }
    });
    let (state, all) = match &*list.read() {
        None => (None, Vec::new()),
        Some(Ok(v)) => (Some(Ok(())), v.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), Vec::new()),
    };
    let count = all.len() as i64;
    let rows = page_slice(&all, page().clamp(0, (count - 1).max(0) / PAGE_SIZE));
    let can_delete = access::can("governance:delete");
    let columns = vec![
        Column::new(t!("pipe-col-name").to_string(), |m: &Marking| {
            rsx! {
                div { class: "text-sm font-medium text-ink break-words", "{m.name}" }
                if !m.description.is_empty() {
                    div { class: "text-xs text-slate break-words", "{m.description}" }
                }
            }
        }),
        // The name tells a marking apart on a phone; the key waits for a wider screen.
        Column::new(t!("gov-col-key").to_string(), |m: &Marking| {
            rsx! {
                code { class: "text-xs", "{m.key}" }
            }
        })
        .secondary(),
        Column::new(t!("gov-col-uses").to_string(), |m: &Marking| {
            rsx! {
                span { class: "tabular-nums", "{m.uses}" }
            }
        })
        .secondary(),
        Column::new(String::new(), move |m: &Marking| {
            let target = m.clone();
            // A carried marking cannot go: the server says so too.
            let free = m.uses == 0;
            rsx! {
                if can_delete && free {
                    button {
                        class: DANGER_BTN,
                        r#type: "button",
                        onclick: move |_| removing.set(Some(target.clone())),
                        {t!("common-delete")}
                    }
                }
            }
        })
        .header_sr_only()
        .actions(),
    ];
    rsx! {
        section { class: "space-y-3",
            h2 { class: SECTION_TITLE, {t!("gov-markings")} }
            p { class: FIELD_HELP, {t!("gov-markings-help")} }
            ListStates {
                state,
                is_empty: all.is_empty(),
                empty_message: t!("gov-empty-markings").to_string(),
                div { class: "space-y-4",
                    DataTable {
                        columns,
                        rows,
                        row_key: RowKey::new(|m: &Marking| m.id.clone()),
                    }
                    ListPager { count, page }
                }
            }
        }
        if let Some(m) = removing() {
            ConfirmDialog {
                title: t!("gov-delete-title").to_string(),
                message: t!("gov-delete-message", name : m.name.clone()).to_string(),
                confirm_label: t!("common-delete").to_string(),
                on_cancel: move |_| removing.set(None),
                on_confirm: move |_| {
                    let id = m.id.clone();
                    removing.set(None);
                    spawn(async move {
                        match api::governance::delete_marking(&id).await {
                            Ok(()) => reload.with_mut(|r| *r += 1),
                            Err(e) => toasts::error(localize(&e)),
                        }
                    });
                },
            }
        }
    }
}

/// The state of a clearance, in words.
fn state_label(c: &Clearance) -> String {
    if c.active {
        t!("gov-state-active")
    } else if c.revoked_at.is_some() {
        t!("gov-state-revoked")
    } else {
        t!("gov-state-expired")
    }
    .to_string()
}

#[component]
fn ClearancesSection(reload: Signal<u32>) -> Element {
    let mut granting = use_signal(|| false);
    let mut revoking = use_signal(|| None::<Clearance>);
    let page = use_signal(|| 0i64);
    let list = use_resource(move || {
        let _ = reload();
        async move { api::governance::clearances().await }
    });
    let (state, all) = match &*list.read() {
        None => (None, Vec::new()),
        Some(Ok(v)) => (Some(Ok(())), v.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), Vec::new()),
    };
    let count = all.len() as i64;
    let rows = page_slice(&all, page().clamp(0, (count - 1).max(0) / PAGE_SIZE));
    let columns = vec![
        Column::new(t!("gov-col-member").to_string(), |c: &Clearance| {
            rsx! {
                span { class: "break-words", "{c.user_email}" }
            }
        }),
        Column::new(t!("gov-col-marking").to_string(), |c: &Clearance| {
            rsx! {
                code { class: "text-xs break-words", "{c.marking_key}" }
            }
        }),
        Column::new(t!("gov-col-purpose").to_string(), |c: &Clearance| {
            rsx! {
                span { class: "break-words", "{c.purpose}" }
            }
        })
        .secondary(),
        Column::new(t!("gov-col-until").to_string(), |c: &Clearance| {
            let day = c.expires_at.get(..10).unwrap_or(&c.expires_at).to_string();
            rsx! {
                span { class: "tabular-nums whitespace-nowrap", "{day}" }
            }
        })
        .secondary(),
        // On a phone the Revoke button says it is active; the word waits for more room.
        Column::new(t!("gov-col-state").to_string(), |c: &Clearance| {
            rsx! { "{state_label(c)}" }
        })
        .secondary(),
        Column::new(String::new(), move |c: &Clearance| {
            let target = c.clone();
            let active = c.active;
            rsx! {
                if active {
                    button {
                        class: BTN_SECONDARY_SM,
                        r#type: "button",
                        onclick: move |_| revoking.set(Some(target.clone())),
                        {t!("gov-revoke")}
                    }
                }
            }
        })
        .header_sr_only()
        .actions(),
    ];
    let export = move |_| {
        spawn(async move {
            match api::governance::journal().await {
                Ok(bytes) => crate::util::save_blob("governance-journal.csv", &bytes),
                Err(e) => toasts::error(localize(&e)),
            }
        });
    };
    rsx! {
        section { class: "space-y-3",
            div { class: "flex flex-wrap items-center justify-between gap-3",
                h2 { class: SECTION_TITLE, {t!("gov-clearances")} }
                div { class: "flex flex-wrap gap-2",
                    button {
                        class: BTN_SECONDARY_SM,
                        r#type: "button",
                        onclick: export,
                        {t!("gov-export-journal")}
                    }
                    button {
                        class: BTN_SECONDARY_SM,
                        r#type: "button",
                        onclick: move |_| granting.set(true),
                        {t!("gov-grant")}
                    }
                }
            }
            p { class: FIELD_HELP, {t!("gov-clearances-help")} }
            ListStates {
                state,
                is_empty: all.is_empty(),
                empty_message: t!("gov-empty-clearances").to_string(),
                div { class: "space-y-4",
                    DataTable {
                        columns,
                        rows,
                        row_key: RowKey::new(|c: &Clearance| c.id.clone()),
                    }
                    ListPager { count, page }
                }
            }
        }
        if granting() {
            ClearanceForm {
                on_close: move |_| granting.set(false),
                on_saved: move |_| {
                    granting.set(false);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
        if let Some(c) = revoking() {
            ConfirmDialog {
                title: t!("gov-revoke-title").to_string(),
                message: t!(
                    "gov-revoke-message", member : c.user_email.clone(), marking : c.marking_key
                    .clone()
                )
                    .to_string(),
                confirm_label: t!("gov-revoke").to_string(),
                on_cancel: move |_| revoking.set(None),
                on_confirm: move |_| {
                    let id = c.id.clone();
                    revoking.set(None);
                    spawn(async move {
                        match api::governance::revoke(&id).await {
                            Ok(()) => reload.with_mut(|r| *r += 1),
                            Err(e) => toasts::error(localize(&e)),
                        }
                    });
                },
            }
        }
    }
}

#[component]
fn MarkingForm(on_close: EventHandler<()>, on_saved: EventHandler<()>) -> Element {
    let mut key = use_signal(String::new);
    let mut name = use_signal(String::new);
    let mut description = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let input = move || MarkingInput {
        key: key().trim().to_string(),
        name: name().trim().to_string(),
        description: description().trim().to_string(),
    };
    let submit = move |_| {
        let body = input();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match api::governance::create_marking(&body).await {
                Ok(_) => on_saved.call(()),
                Err(e) => error.set(Some(localize(&e))),
            }
            busy.set(false);
        });
    };
    rsx! {
        FormDialog {
            title: t!("gov-new-marking").to_string(),
            submit_label: t!("pipe-save").to_string(),
            on_close: move |_| on_close.call(()),
            on_submit: submit,
            busy: busy(),
            valid: input().violation().is_none(),
            div { class: "space-y-3",
                div {
                    label { class: FIELD_LABEL, r#for: "gov-name", {t!("pipe-col-name")} }
                    input {
                        id: "gov-name",
                        class: INPUT,
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                div {
                    label { class: FIELD_LABEL, r#for: "gov-key", {t!("gov-col-key")} }
                    input {
                        id: "gov-key",
                        class: "{INPUT} font-mono",
                        value: "{key}",
                        oninput: move |e| key.set(e.value()),
                    }
                    p { class: FIELD_HELP, {t!("gov-key-help")} }
                }
                div {
                    label { class: FIELD_LABEL, r#for: "gov-description",
                        {t!("gov-marking-description")}
                    }
                    input {
                        id: "gov-description",
                        class: INPUT,
                        value: "{description}",
                        oninput: move |e| description.set(e.value()),
                    }
                }
                if let Some(e) = error() {
                    p { class: FIELD_ERROR, role: "alert", "{e}" }
                }
            }
        }
    }
}

/// Members of the org and the markings, for the clearance form.
async fn choices() -> Result<(Vec<(i64, String)>, Vec<Marking>), api::error::ApiError> {
    let members = match org::current() {
        Some(id) => api::orgs::detail(id)
            .await?
            .members
            .iter()
            .map(|m| (m.user_id, member_label(m)))
            .collect(),
        None => Vec::new(),
    };
    Ok((members, api::governance::markings().await?))
}

#[component]
fn ClearanceForm(on_close: EventHandler<()>, on_saved: EventHandler<()>) -> Element {
    let mut user = use_signal(|| None::<i64>);
    let mut marking = use_signal(String::new);
    let mut purpose = use_signal(String::new);
    let mut days = use_signal(|| "30".to_string());
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let loaded = use_resource(choices);
    let (members, markings) = match &*loaded.read() {
        Some(Ok((m, k))) => (m.clone(), k.clone()),
        _ => (Vec::new(), Vec::new()),
    };
    let input = move || ClearanceInput {
        user_id: user().unwrap_or(0),
        marking_key: marking(),
        purpose: purpose().trim().to_string(),
        days: days().trim().parse().unwrap_or(0),
    };
    let submit = move |_| {
        let body = input();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match api::governance::grant(&body).await {
                Ok(_) => on_saved.call(()),
                Err(e) => error.set(Some(localize(&e))),
            }
            busy.set(false);
        });
    };
    let chosen_user = user();
    let chosen_marking = marking();
    rsx! {
        FormDialog {
            title: t!("gov-grant").to_string(),
            submit_label: t!("pipe-save").to_string(),
            on_close: move |_| on_close.call(()),
            on_submit: submit,
            busy: busy(),
            valid: user().is_some() && input().violation().is_none(),
            div { class: "space-y-3",
                div {
                    label { class: FIELD_LABEL, r#for: "gov-member", {t!("gov-col-member")} }
                    // `selected` per option: a select value set before its options exist
                    // would show the first member while none is chosen.
                    select {
                        id: "gov-member",
                        class: INPUT,
                        onchange: move |e| user.set(e.value().parse().ok()),
                        option { value: "", selected: chosen_user.is_none(), {t!("gov-choose")} }
                        for (id, label) in members {
                            option {
                                key: "{id}",
                                value: "{id}",
                                selected: chosen_user == Some(id),
                                "{label}"
                            }
                        }
                    }
                }
                div {
                    label { class: FIELD_LABEL, r#for: "gov-marking", {t!("gov-col-marking")} }
                    select {
                        id: "gov-marking",
                        class: INPUT,
                        onchange: move |e| marking.set(e.value()),
                        option { value: "", selected: chosen_marking.is_empty(), {t!("gov-choose")} }
                        for m in markings {
                            option {
                                key: "{m.key}",
                                value: "{m.key}",
                                selected: chosen_marking == m.key,
                                "{m.name}"
                            }
                        }
                    }
                }
                div {
                    label { class: FIELD_LABEL, r#for: "gov-purpose", {t!("gov-col-purpose")} }
                    input {
                        id: "gov-purpose",
                        class: INPUT,
                        value: "{purpose}",
                        oninput: move |e| purpose.set(e.value()),
                    }
                    p { class: FIELD_HELP, {t!("gov-purpose-help")} }
                }
                div {
                    label { class: FIELD_LABEL, r#for: "gov-days", {t!("gov-days")} }
                    input {
                        id: "gov-days",
                        class: "{INPUT} max-w-32 tabular-nums",
                        r#type: "number",
                        min: "1",
                        max: "365",
                        value: "{days}",
                        oninput: move |e| days.set(e.value()),
                    }
                }
                if let Some(e) = error() {
                    p { class: FIELD_ERROR, role: "alert", "{e}" }
                }
            }
        }
    }
}
