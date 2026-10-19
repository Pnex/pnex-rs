//! Assignments tab: which role each member, team or service account holds
//! on top of its membership.

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::roles::use_add_request;
use super::{member_label, role_label, AddRequest};
use crate::api;
use crate::api::iam::{Assignment, Holder, Role, ServiceAccount, Team};
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::DANGER_BTN;
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ui::{FIELD_HELP, FIELD_LABEL, INPUT};
use crate::state::{org, toasts};

/// Everything an assignment row or form names.
#[derive(Clone, PartialEq, Default)]
struct Directory {
    /// `(user id, display name)` of the members.
    members: Vec<(i64, String)>,
    teams: Vec<Team>,
    accounts: Vec<ServiceAccount>,
    roles: Vec<Role>,
}

impl Directory {
    fn holder(&self, a: &Assignment) -> String {
        if let Some(sa) = &a.service_account_id {
            let name = self
                .accounts
                .iter()
                .find(|s| &s.id == sa)
                .map(|s| s.name.clone());
            return t!("access-holder-service", name : name.unwrap_or_default()).to_string();
        }
        if let Some(team) = &a.team_id {
            let name = self
                .teams
                .iter()
                .find(|t| &t.id == team)
                .map(|t| t.name.clone());
            return t!("access-holder-team", name : name.unwrap_or_default()).to_string();
        }
        let id = a.user_id.unwrap_or_default();
        self.members
            .iter()
            .find(|(m, _)| *m == id)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| format!("#{id}"))
    }

    fn role(&self, key: &str) -> String {
        self.roles
            .iter()
            .find(|r| r.key == key)
            .map(role_label)
            .unwrap_or_else(|| key.to_string())
    }
}

async fn directory() -> Result<Directory, api::error::ApiError> {
    let members = match org::current() {
        Some(id) => api::orgs::detail(id)
            .await?
            .members
            .iter()
            .map(|m| (m.user_id, member_label(m)))
            .collect(),
        None => Vec::new(),
    };
    Ok(Directory {
        members,
        teams: api::iam::teams().await?,
        accounts: api::iam::service_accounts().await?,
        roles: api::iam::roles().await?,
    })
}

#[component]
pub(crate) fn AssignmentsTab(
    reload: Signal<u32>,
    add: Signal<AddRequest>,
    can_write: bool,
) -> Element {
    let mut creating = use_signal(|| false);
    let mut confirm = use_signal(|| None::<Assignment>);
    use_add_request(add, move || creating.set(true));
    let data = use_resource(move || {
        let _ = reload();
        async move {
            let dir = directory().await?;
            let rows = api::iam::assignments().await?;
            Ok::<_, api::error::ApiError>((dir, rows))
        }
    });
    let (state, is_empty, dir, rows) = match &*data.value().read() {
        None => (None, false, Directory::default(), Vec::new()),
        Some(Ok((d, r))) => (Some(Ok(())), r.is_empty(), d.clone(), r.clone()),
        Some(Err(e)) => (
            Some(Err(e.clone())),
            false,
            Directory::default(),
            Vec::new(),
        ),
    };
    let confirm_now = confirm.read().clone();
    let confirm_message = confirm_now.as_ref().map(|a| {
        t!("access-take-back-message", holder : dir.holder(a), role : dir.role(& a.role_key))
            .to_string()
    });
    let (d1, d2) = (dir.clone(), dir.clone());

    let columns = vec![
        Column::new(
            t!("access-col-holder").to_string(),
            move |a: &Assignment| {
                rsx! {
                    span { class: "text-sm font-medium text-ink", {d1.holder(a)} }
                }
            },
        ),
        Column::new(t!("access-col-role").to_string(), move |a: &Assignment| {
            rsx! {
                span { class: "text-sm text-ink", {d2.role(&a.role_key)} }
            }
        }),
        Column::new(t!("access-col-since").to_string(), |a: &Assignment| {
            rsx! {
                span { class: "text-sm text-slate", {date_label(&a.created_at)} }
            }
        }),
        Column::new(String::new(), move |a: &Assignment| {
            let remove = a.clone();
            rsx! {
                if can_write {
                    div { class: "flex justify-end",
                        button {
                            class: DANGER_BTN,
                            r#type: "button",
                            onclick: move |_| confirm.set(Some(remove.clone())),
                            {t!("access-take-back")}
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
            empty_message: t!("access-assignments-empty").to_string(),
            DataTable {
                columns,
                rows,
                row_key: RowKey::new(|a: &Assignment| a.id.clone()),
            }
        }
        if creating() {
            AssignDialog {
                dir: dir.clone(),
                on_close: move |_| creating.set(false),
                on_saved: move |_| {
                    creating.set(false);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
        if let Some(a) = confirm_now {
            ConfirmDialog {
                title: t!("access-take-back-title").to_string(),
                message: confirm_message.clone().unwrap_or_default(),
                confirm_label: t!("access-take-back").to_string(),
                on_cancel: move |_| confirm.set(None),
                on_confirm: move |_| {
                    let id = a.id.clone();
                    confirm.set(None);
                    spawn(async move {
                        match api::iam::delete_assignment(&id).await {
                            Ok(()) => {
                                toasts::success("toast-access-assignment-deleted");
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

/// `kind:id` value of a holder option (`user:12`, `team:<uuid>`, `service:<uuid>`).
fn parse_holder(value: &str) -> Option<Holder> {
    let (kind, id) = value.split_once(':')?;
    match kind {
        "user" => id.parse().ok().map(Holder::User),
        "team" => Some(Holder::Team(id.to_string())),
        "service" => Some(Holder::Service(id.to_string())),
        _ => None,
    }
}

#[component]
fn AssignDialog(dir: Directory, on_close: EventHandler<()>, on_saved: EventHandler<()>) -> Element {
    let mut holder = use_signal(String::new);
    let mut role = use_signal(String::new);
    let mut busy = use_signal(|| false);
    // Owner stays a membership role; never admin to a service account.
    let to_service = holder().starts_with("service:");
    let roles: Vec<Role> = dir
        .roles
        .iter()
        .filter(|r| r.key != "owner" && !(to_service && r.key == "admin"))
        .cloned()
        .collect();
    let chosen = roles.iter().find(|r| r.key == role()).cloned();
    let valid = parse_holder(&holder()).is_some() && chosen.is_some();
    let save = move |_| {
        let (Some(h), Some(r)) = (parse_holder(&holder()), chosen.clone()) else {
            return;
        };
        busy.set(true);
        spawn(async move {
            let res = api::iam::create_assignment(&h, &r).await;
            busy.set(false);
            match res {
                Ok(_) => {
                    toasts::success("toast-access-assignment-saved");
                    on_saved.call(());
                }
                Err(e) => toasts::error(e),
            }
        });
    };
    rsx! {
        FormDialog {
            title: t!("access-new-assignment").to_string(),
            submit_label: t!("access-give").to_string(),
            on_close: move |_| on_close.call(()),
            on_submit: save,
            busy: busy(),
            valid,
            div {
                label { class: FIELD_LABEL, r#for: "access-holder", {t!("access-col-holder")} }
                select {
                    id: "access-holder",
                    class: INPUT,
                    value: "{holder}",
                    onchange: move |e| holder.set(e.value()),
                    option { value: "", {t!("access-pick-holder")} }
                    optgroup { label: t!("access-holders-members").to_string(),
                        for (id, name) in dir.members.iter() {
                            option { key: "user:{id}", value: "user:{id}", {name.clone()} }
                        }
                    }
                    optgroup { label: t!("access-tab-teams").to_string(),
                        for team in dir.teams.iter() {
                            option {
                                key: "team:{team.id}",
                                value: "team:{team.id}",
                                {team.name.clone()}
                            }
                        }
                    }
                    optgroup { label: t!("access-tab-service-accounts").to_string(),
                        for sa in dir.accounts.iter() {
                            option {
                                key: "service:{sa.id}",
                                value: "service:{sa.id}",
                                {sa.name.clone()}
                            }
                        }
                    }
                }
            }
            div {
                label { class: FIELD_LABEL, r#for: "access-role", {t!("access-col-role")} }
                select {
                    id: "access-role",
                    class: INPUT,
                    value: "{role}",
                    onchange: move |e| role.set(e.value()),
                    option { value: "", {t!("access-pick-role")} }
                    for r in roles.iter() {
                        option { key: "{r.key}", value: "{r.key}", {role_label(r)} }
                    }
                }
                p { class: FIELD_HELP, {t!("access-assign-help")} }
            }
        }
    }
}
