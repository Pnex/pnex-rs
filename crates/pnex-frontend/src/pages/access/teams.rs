//! Teams tab: members of the org grouped to give them roles together.

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::roles::use_add_request;
use super::{member_label, AddRequest};
use crate::api;
use crate::api::iam::Team;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::DANGER_BTN;
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::modal::Modal;
use crate::components::ui::{BTN_SECONDARY_SM, FIELD_LABEL, INPUT};
use crate::state::{org, toasts};

#[component]
pub(crate) fn TeamsTab(reload: Signal<u32>, add: Signal<AddRequest>, can_write: bool) -> Element {
    let mut editing = use_signal(|| None::<Option<Team>>);
    let mut members_of = use_signal(|| None::<Team>);
    let mut confirm = use_signal(|| None::<Team>);
    use_add_request(add, move || editing.set(Some(None)));
    let teams = use_resource(move || {
        let _ = reload();
        async move { api::iam::teams().await }
    });
    let (state, is_empty, rows) = match &*teams.value().read() {
        None => (None, false, Vec::new()),
        Some(Ok(r)) => (Some(Ok(())), r.is_empty(), r.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), false, Vec::new()),
    };
    let editing_now = editing.read().clone();
    let members_now = members_of.read().clone();
    let confirm_now = confirm.read().clone();

    let columns = vec![
        Column::new(t!("access-col-team").to_string(), |t: &Team| {
            rsx! {
                div { class: "text-sm font-medium text-ink", {t.name.clone()} }
                if !t.description.is_empty() {
                    div { class: "text-xs text-slate", {t.description.clone()} }
                }
            }
        }),
        Column::new(t!("access-col-members").to_string(), |t: &Team| {
            rsx! {
                span { class: "text-sm text-ink", {t.members.len().to_string()} }
            }
        }),
        Column::new(String::new(), move |t: &Team| {
            let (edit, members, remove) = (t.clone(), t.clone(), t.clone());
            rsx! {
                if can_write {
                    div { class: "flex justify-end gap-2",
                        button {
                            class: BTN_SECONDARY_SM,
                            r#type: "button",
                            onclick: move |_| members_of.set(Some(members.clone())),
                            {t!("access-team-members")}
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
        ListStates {
            state,
            is_empty,
            empty_message: t!("access-teams-empty").to_string(),
            DataTable {
                columns,
                rows,
                row_key: RowKey::new(|t: &Team| t.id.clone()),
            }
        }
        if let Some(team) = editing_now {
            TeamDialog {
                team,
                on_close: move |_| editing.set(None),
                on_saved: move |_| {
                    editing.set(None);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
        if let Some(team) = members_now {
            TeamMembers {
                team,
                on_close: move |_| {
                    members_of.set(None);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
        if let Some(team) = confirm_now {
            ConfirmDialog {
                title: t!("access-delete-team-title").to_string(),
                message: t!("access-delete-team-message", name : team.name.clone()).to_string(),
                confirm_label: t!("common-delete").to_string(),
                on_cancel: move |_| confirm.set(None),
                on_confirm: move |_| {
                    let id = team.id.clone();
                    confirm.set(None);
                    spawn(async move {
                        match api::iam::delete_team(&id).await {
                            Ok(()) => {
                                toasts::success("toast-access-team-deleted");
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
fn TeamDialog(
    team: Option<Team>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let mut name = use_signal(|| team.as_ref().map(|t| t.name.clone()).unwrap_or_default());
    let mut description = use_signal(|| {
        team.as_ref()
            .map(|t| t.description.clone())
            .unwrap_or_default()
    });
    let mut busy = use_signal(|| false);
    let title = if team.is_some() {
        t!("access-edit-team")
    } else {
        t!("access-new-team")
    };
    let save = move |_| {
        let (n, d, target) = (name(), description(), team.clone());
        busy.set(true);
        spawn(async move {
            let res = match &target {
                None => api::iam::create_team(&n, &d).await,
                Some(t) => api::iam::update_team(&t.id, &n, &d).await,
            };
            busy.set(false);
            match res {
                Ok(_) => {
                    toasts::success("toast-access-team-saved");
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
        }
    }
}

/// Members of the org, checked when in the team; a click adds or removes
/// at once.
#[component]
fn TeamMembers(team: Team, on_close: EventHandler<()>) -> Element {
    let current = use_signal(|| team.members.clone());
    let members = use_resource(move || async move {
        match org::current() {
            Some(id) => api::orgs::detail(id).await.map(|d| d.members),
            None => Ok(Vec::new()),
        }
    });
    let rows = match &*members.value().read() {
        Some(Ok(m)) => m.clone(),
        _ => Vec::new(),
    };
    let team_id = team.id.clone();
    rsx! {
        Modal {
            title: t!("access-team-members-title", name : team.name.clone()).to_string(),
            max_width: "max-w-md".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "max-h-96 overflow-y-auto divide-y divide-line",
                for m in rows {
                    MemberRow {
                        key: "{m.user_id}",
                        team_id: team_id.clone(),
                        user_id: m.user_id,
                        label: member_label(&m),
                        current,
                    }
                }
            }
        }
    }
}

#[component]
fn MemberRow(team_id: String, user_id: i64, label: String, current: Signal<Vec<i64>>) -> Element {
    let checked = current.read().contains(&user_id);
    let toggle = move |_| {
        let id = team_id.clone();
        spawn(async move {
            let res = if checked {
                api::iam::remove_team_member(&id, user_id).await
            } else {
                api::iam::add_team_member(&id, user_id).await
            };
            match res {
                Ok(team) => current.set(team.members),
                Err(e) => toasts::error(e),
            }
        });
    };
    rsx! {
        label { class: "flex items-center gap-3 px-1 py-2 text-sm text-ink cursor-pointer",
            input { r#type: "checkbox", checked, onchange: toggle }
            {label}
        }
    }
}
