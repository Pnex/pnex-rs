//! Roles tab: the four managed roles (read-only) and the custom roles of
//! the org, edited with the permission grid.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::grid::{split_patterns, Mark, PermissionGrid};
use super::{role_label, AddRequest};
use crate::api;
use crate::api::iam::Role;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::DANGER_BTN;
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ui::{BTN_SECONDARY_SM, FIELD_HELP, FIELD_LABEL, INPUT};
use crate::state::toasts;

/// Opens the create form when the header button is pressed.
pub(crate) fn use_add_request(add: Signal<AddRequest>, mut open: impl FnMut() + 'static) {
    let mut seen = use_signal(|| add.peek().0);
    use_effect(move || {
        let a = add().0;
        if a != *seen.peek() {
            seen.set(a);
            open();
        }
    });
}

#[component]
pub(crate) fn RolesTab(reload: Signal<u32>, add: Signal<AddRequest>, can_write: bool) -> Element {
    let mut editing = use_signal(|| None::<Option<Role>>);
    let mut confirm = use_signal(|| None::<Role>);
    use_add_request(add, move || editing.set(Some(None)));
    let roles = use_resource(move || {
        let _ = reload();
        async move { api::iam::roles().await }
    });
    let (state, rows) = match &*roles.value().read() {
        None => (None, Vec::new()),
        Some(Ok(r)) => (Some(Ok(())), r.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), Vec::new()),
    };
    let editing_now = editing.read().clone();
    let confirm_now = confirm.read().clone();

    let columns = vec![
        Column::new(t!("access-col-role").to_string(), |r: &Role| {
            rsx! {
                div { class: "text-sm font-medium text-ink", {role_label(r)} }
                div { class: "font-mono text-xs text-slate", {r.key.clone()} }
            }
        }),
        Column::new(t!("access-col-kind").to_string(), |r: &Role| {
            let label = if r.managed {
                t!("access-role-managed")
            } else {
                t!("access-role-custom")
            };
            rsx! {
                span { class: "rounded-full border border-line px-2 py-0.5 text-xs text-slate", {label} }
            }
        }),
        Column::new(t!("access-col-permissions").to_string(), |r: &Role| {
            let (allow, deny) = (r.patterns("allow").len(), r.patterns("deny").len());
            rsx! {
                span { class: "text-sm text-ink",
                    {t!("access-role-counts", allow : allow, deny : deny)}
                }
            }
        }),
        Column::new(String::new(), move |r: &Role| {
            let open = r.clone();
            let remove = r.clone();
            let custom = !r.managed;
            rsx! {
                div { class: "flex justify-end gap-2",
                    button {
                        class: BTN_SECONDARY_SM,
                        r#type: "button",
                        onclick: move |_| editing.set(Some(Some(open.clone()))),
                        if custom && can_write {
                            {t!("access-edit")}
                        } else {
                            {t!("access-view")}
                        }
                    }
                    if custom && can_write {
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
        ListStates { state, is_empty: false, empty_message: String::new(),
            DataTable {
                columns,
                rows,
                row_key: RowKey::new(|r: &Role| r.key.clone()),
            }
        }
        if let Some(role) = editing_now {
            RoleDialog {
                role,
                can_write,
                on_close: move |_| editing.set(None),
                on_saved: move |_| {
                    editing.set(None);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
        if let Some(role) = confirm_now {
            ConfirmDialog {
                title: t!("access-delete-role-title").to_string(),
                message: t!("access-delete-role-message", name : role.name.clone()).to_string(),
                confirm_label: t!("common-delete").to_string(),
                on_cancel: move |_| confirm.set(None),
                on_confirm: move |_| {
                    let id = role.id.clone().unwrap_or_default();
                    confirm.set(None);
                    spawn(async move {
                        match api::iam::delete_role(&id).await {
                            Ok(()) => {
                                toasts::success("toast-access-role-deleted");
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

/// Marks of a role's exact permissions; its wildcards apart.
fn marks_of(role: Option<&Role>) -> (BTreeMap<String, Mark>, Vec<String>, Vec<String>) {
    let Some(role) = role else {
        return (BTreeMap::new(), Vec::new(), Vec::new());
    };
    let (allow, extra_allow) = split_patterns(&role.patterns("allow"));
    let (deny, extra_deny) = split_patterns(&role.patterns("deny"));
    let mut marks = BTreeMap::new();
    for p in allow {
        marks.insert(p, Mark::Allow);
    }
    for p in deny {
        marks.insert(p, Mark::Deny);
    }
    (marks, extra_allow, extra_deny)
}

#[component]
fn RoleDialog(
    /// `None` = new custom role.
    role: Option<Role>,
    can_write: bool,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let (initial, extra_allow, extra_deny) = marks_of(role.as_ref());
    let marks = use_signal(|| initial);
    let mut key = use_signal(|| role.as_ref().map(|r| r.key.clone()).unwrap_or_default());
    let mut name = use_signal(|| role.as_ref().map(role_label).unwrap_or_default());
    let mut description = use_signal(|| {
        role.as_ref()
            .map(|r| r.description.clone())
            .unwrap_or_default()
    });
    let mut busy = use_signal(|| false);
    let readonly = !can_write || role.as_ref().is_some_and(|r| r.managed);
    let is_new = role.is_none();
    let title = match &role {
        None => t!("access-new-role").to_string(),
        Some(r) => role_label(r),
    };
    let wildcards: Vec<String> = extra_allow
        .iter()
        .map(|p| format!("+ {p}"))
        .chain(extra_deny.iter().map(|p| format!("− {p}")))
        .collect();
    let valid = !name().trim().is_empty() && (!is_new || !key().trim().is_empty());

    let save = move |_| {
        if readonly || busy() {
            return;
        }
        let pick = |want: Mark| -> Vec<String> {
            marks
                .read()
                .iter()
                .filter(|(_, m)| **m == want)
                .map(|(p, _)| p.clone())
                .collect()
        };
        let mut allow = pick(Mark::Allow);
        allow.extend(extra_allow.iter().cloned());
        let mut deny = pick(Mark::Deny);
        deny.extend(extra_deny.iter().cloned());
        let (k, n, d) = (key(), name(), description());
        let target = role.clone();
        busy.set(true);
        spawn(async move {
            let res = match &target {
                None => api::iam::create_role(k.trim(), &n, &d, &allow, &deny).await,
                Some(r) => {
                    let id = r.id.clone().unwrap_or_default();
                    let version = r.version.unwrap_or(1);
                    api::iam::update_role(&id, &n, &d, &allow, &deny, version).await
                }
            };
            busy.set(false);
            match res {
                Ok(_) => {
                    toasts::success("toast-access-role-saved");
                    on_saved.call(());
                }
                Err(e) => toasts::error(e),
            }
        });
    };

    rsx! {
        FormDialog {
            title,
            submit_label: t!("common-save").to_string(),
            on_close: move |_| on_close.call(()),
            on_submit: save,
            busy: busy() || readonly,
            valid,
            max_width: "max-w-3xl".to_string(),
            if is_new {
                div {
                    label { class: FIELD_LABEL, r#for: "access-field-key", {t!("access-field-key")} }
                    input {
                        id: "access-field-key",
                        class: "{INPUT} font-mono",
                        value: "{key}",
                        placeholder: "site-reader",
                        oninput: move |e| key.set(e.value()),
                    }
                    p { class: FIELD_HELP, {t!("access-field-key-help")} }
                }
            }
            div {
                label { class: FIELD_LABEL, r#for: "access-field-name", {t!("access-field-name")} }
                input {
                    id: "access-field-name",
                    class: INPUT,
                    value: "{name}",
                    disabled: readonly,
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
                    disabled: readonly,
                    oninput: move |e| description.set(e.value()),
                }
            }
            div {
                label { class: FIELD_LABEL, {t!("access-field-permissions")} }
                p { class: "{FIELD_HELP} mb-2", {t!("access-grid-help")} }
                PermissionGrid { marks, readonly }
                if !wildcards.is_empty() {
                    p { class: FIELD_HELP, {t!("access-wildcards", list : wildcards.join(", "))} }
                }
            }
        }
    }
}
