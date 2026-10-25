//! Applications tab (external-apps.md D433): external programs of the org
//! built on the ontology. Each dresses a service account with the scope it
//! reads; the change feed keeps to that scope.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::apps::{AppInput, AppScope, AppView, MEDIA_KINDS};

use super::roles::use_add_request;
use super::AddRequest;
use crate::api;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::DANGER_BTN;
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ontology::{link_label, type_label};
use crate::components::ui::{BTN_SECONDARY_SM, FIELD_HELP, FIELD_LABEL, INPUT};
use crate::state::toasts;

const MB: i64 = 1024 * 1024;

#[component]
pub(crate) fn AppsTab(reload: Signal<u32>, add: Signal<AddRequest>, can_write: bool) -> Element {
    let mut editing = use_signal(|| None::<Option<AppView>>);
    let mut confirm = use_signal(|| None::<AppView>);
    use_add_request(add, move || editing.set(Some(None)));
    let apps = use_resource(move || {
        let _ = reload();
        async move { api::apps::list().await }
    });
    let (state, is_empty, rows) = match &*apps.value().read() {
        None => (None, false, Vec::new()),
        Some(Ok(r)) => (Some(Ok(())), r.is_empty(), r.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), false, Vec::new()),
    };
    let editing_now = editing.read().clone();
    let confirm_now = confirm.read().clone();
    let columns = vec![
        Column::new(t!("access-col-app").to_string(), |a: &AppView| {
            rsx! {
                div { class: "text-sm font-medium text-ink", {a.name.clone()} }
                code { class: "text-xs text-slate", {a.key.clone()} }
            }
        }),
        Column::new(t!("access-col-app-scope").to_string(), |a: &AppView| {
            let summary = t!(
                "access-app-scope-summary", types : a.scope.types.len(), links : a.scope
                .link_types.len()
            );
            rsx! {
                span { class: "text-sm text-ink", {summary} }
            }
        }),
        Column::new(String::new(), move |a: &AppView| {
            let (edit, remove) = (a.clone(), a.clone());
            rsx! {
                if can_write {
                    div { class: "flex justify-end gap-2",
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
        p { class: "{FIELD_HELP} mb-3", {t!("access-apps-help")} }
        ListStates {
            state,
            is_empty,
            empty_message: t!("access-apps-empty").to_string(),
            DataTable {
                columns,
                rows,
                row_key: RowKey::new(|a: &AppView| a.id.clone()),
            }
        }
        if let Some(app) = editing_now {
            AppDialog {
                app,
                on_close: move |_| editing.set(None),
                on_saved: move |_| {
                    editing.set(None);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
        if let Some(app) = confirm_now {
            ConfirmDialog {
                title: t!("access-delete-app-title").to_string(),
                message: t!("access-delete-app-message", name : app.name.clone()).to_string(),
                confirm_label: t!("common-delete").to_string(),
                on_cancel: move |_| confirm.set(None),
                on_confirm: move |_| {
                    let id = app.id.clone();
                    confirm.set(None);
                    spawn(async move {
                        match api::apps::delete(&id).await {
                            Ok(()) => {
                                toasts::success("toast-access-app-deleted");
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
fn AppDialog(
    app: Option<AppView>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let start = app.clone();
    let mut key = use_signal(|| start.as_ref().map(|a| a.key.clone()).unwrap_or_default());
    let mut name = use_signal(|| start.as_ref().map(|a| a.name.clone()).unwrap_or_default());
    let mut description = use_signal(|| {
        start
            .as_ref()
            .map(|a| a.description.clone())
            .unwrap_or_default()
    });
    let account = use_signal(|| {
        start
            .as_ref()
            .and_then(|a| a.service_account_id.clone())
            .unwrap_or_default()
    });
    let scope = use_signal(|| start.as_ref().map(|a| a.scope.clone()).unwrap_or_default());
    let mut max_mb = use_signal(|| {
        start
            .as_ref()
            .and_then(|a| a.scope.media_max_bytes)
            .map(|b| (b / MB).to_string())
            .unwrap_or_default()
    });
    let mut busy = use_signal(|| false);
    let title = if app.is_some() {
        t!("access-edit-app")
    } else {
        t!("access-new-app")
    };
    let save = move |_| {
        let mut s: AppScope = scope();
        s.media_max_bytes = max_mb().trim().parse::<i64>().ok().map(|m| m * MB);
        let input = AppInput {
            key: key().trim().to_string(),
            name: name(),
            description: description(),
            service_account_id: Some(account()).filter(|a| !a.is_empty()),
            scope: s,
            expected_version: app.as_ref().map(|a| a.version),
        };
        let id = app.as_ref().map(|a| a.id.clone());
        busy.set(true);
        spawn(async move {
            let res = api::apps::save(id.as_deref(), &input).await;
            busy.set(false);
            match res {
                Ok(_) => {
                    toasts::success("toast-access-app-saved");
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
            valid: !name().trim().is_empty() && !key().trim().is_empty(),
            div {
                label { class: FIELD_LABEL, r#for: "app-field-key", {t!("access-field-key")} }
                input {
                    id: "app-field-key",
                    class: INPUT,
                    value: "{key}",
                    placeholder: "reader",
                    oninput: move |e| key.set(e.value()),
                }
            }
            div {
                label { class: FIELD_LABEL, r#for: "app-field-name", {t!("access-field-name")} }
                input {
                    id: "app-field-name",
                    class: INPUT,
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                }
            }
            div {
                label { class: FIELD_LABEL, r#for: "app-field-description",
                    {t!("access-field-description")}
                }
                input {
                    id: "app-field-description",
                    class: INPUT,
                    value: "{description}",
                    oninput: move |e| description.set(e.value()),
                }
            }
            AccountPicker { account }
            ScopePicker { scope }
            div {
                label { class: FIELD_LABEL, r#for: "app-field-max", {t!("access-app-media-max")} }
                input {
                    id: "app-field-max",
                    class: INPUT,
                    r#type: "number",
                    min: "1",
                    value: "{max_mb}",
                    oninput: move |e| max_mb.set(e.value()),
                }
            }
        }
    }
}

/// The service account the application signs in with.
#[component]
fn AccountPicker(account: Signal<String>) -> Element {
    let accounts = use_resource(|| async { api::iam::service_accounts().await });
    let rows = match &*accounts.value().read() {
        Some(Ok(r)) => r.clone(),
        _ => Vec::new(),
    };
    rsx! {
        div {
            label { class: FIELD_LABEL, r#for: "app-field-account", {t!("access-field-app-account")} }
            select {
                id: "app-field-account",
                class: INPUT,
                value: "{account}",
                onchange: move |e| account.set(e.value()),
                option { value: "", selected: account().is_empty(), {t!("access-app-no-account")} }
                for a in rows {
                    option {
                        key: "{a.id}",
                        value: "{a.id}",
                        selected: account() == a.id,
                        {a.name.clone()}
                    }
                }
            }
            p { class: FIELD_HELP, {t!("access-app-account-help")} }
        }
    }
}

/// Which list of the scope a checkbox writes.
#[derive(Clone, Copy, PartialEq)]
enum Part {
    Types,
    Links,
    Media,
}

/// Types, link types and media kinds the application reads.
#[component]
fn ScopePicker(scope: Signal<AppScope>) -> Element {
    let types = use_resource(|| async { api::ontology::types().await });
    let links = use_resource(|| async { api::ontology::link_types().await });
    let type_rows: Vec<(String, String)> = match &*types.value().read() {
        Some(Ok(r)) => r
            .iter()
            .map(|t| (t.def.key.clone(), type_label(&t.def)))
            .collect(),
        _ => Vec::new(),
    };
    let link_rows: Vec<(String, String)> = match &*links.value().read() {
        Some(Ok(r)) => r
            .iter()
            .map(|l| {
                (
                    l.def.key.clone(),
                    link_label(Some(&l.def), &l.def.key, false),
                )
            })
            .collect(),
        _ => Vec::new(),
    };
    let media_rows: Vec<(String, String)> = MEDIA_KINDS
        .iter()
        .map(|k| (k.to_string(), t!(&format!("media-kind-{k}")).to_string()))
        .collect();
    rsx! {
        Choices {
            title: t!("access-app-types").to_string(),
            part: Part::Types,
            rows: type_rows,
            scope,
        }
        Choices {
            title: t!("access-app-link-types").to_string(),
            part: Part::Links,
            rows: link_rows,
            scope,
        }
        Choices {
            title: t!("access-app-media-kinds").to_string(),
            part: Part::Media,
            rows: media_rows,
            scope,
        }
    }
}

#[component]
fn Choices(
    title: String,
    part: Part,
    rows: Vec<(String, String)>,
    scope: Signal<AppScope>,
) -> Element {
    rsx! {
        fieldset {
            legend { class: FIELD_LABEL, {title} }
            div { class: "max-h-40 overflow-y-auto rounded-md border border-line px-2",
                for (k, label) in rows {
                    Choice {
                        key: "{k}",
                        value: k.clone(),
                        label,
                        part,
                        scope,
                    }
                }
            }
        }
    }
}

fn part_of(s: &mut AppScope, part: Part) -> &mut Vec<String> {
    match part {
        Part::Types => &mut s.types,
        Part::Links => &mut s.link_types,
        Part::Media => &mut s.media_kinds,
    }
}

#[component]
fn Choice(value: String, label: String, part: Part, scope: Signal<AppScope>) -> Element {
    let checked = part_of(&mut scope.read().clone(), part).contains(&value);
    let v = value.clone();
    let toggle = move |_| {
        scope.with_mut(|s| {
            let l = part_of(s, part);
            if let Some(i) = l.iter().position(|x| *x == v) {
                l.remove(i);
            } else {
                l.push(v.clone());
            }
        });
    };
    rsx! {
        label { class: "flex items-center gap-3 px-1 py-1.5 text-sm text-ink cursor-pointer",
            input { r#type: "checkbox", checked, onchange: toggle }
            span { {label} }
            code { class: "text-xs text-slate", {value} }
        }
    }
}
