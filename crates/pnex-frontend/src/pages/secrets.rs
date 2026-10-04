//! Org secrets vault — central CRUD page (secrets.md §6, D110–D117).
//!
//! Lists every secret of the org with its consumers ("used by"); never
//! shows a value. Owner/admin create, rename, replace a value, delete
//! (refused while used). Members and viewers only see the list.

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api;
use crate::api::error::ApiError;
use crate::app::Route;
use crate::components::crud::filters::{FilterBar, RefreshButton, SearchInput};
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;
use crate::components::secret_field::can_manage_secrets;
use pnex_core::{OrgSecret, OrgSecretInput, SecretConsumerKind, SecretUsage};

/// `2026-10-01T13:44:11+02:00` → `2026-10-01 13:44`.
fn short_date(rfc3339: &str) -> String {
    rfc3339.get(..16).unwrap_or(rfc3339).replace('T', " ")
}

/// Page where a consumer of the given kind is edited.
fn usage_route(kind: SecretConsumerKind) -> Route {
    match kind {
        SecretConsumerKind::NotifyChannel => Route::Notifications {},
        SecretConsumerKind::Flow => Route::Flows {},
        SecretConsumerKind::Wifi => Route::EdgeRefs {},
        SecretConsumerKind::LlmProvider => Route::OrgsCurrent {},
    }
}

/// Render-scope label of a usage (`Notification channel · oncall`).
fn usage_label(u: &SecretUsage) -> String {
    let kind = match u.kind {
        SecretConsumerKind::NotifyChannel => t!("secrets-usage-notify-channel"),
        SecretConsumerKind::Flow => t!("secrets-usage-flow"),
        SecretConsumerKind::Wifi => t!("secrets-usage-wifi"),
        SecretConsumerKind::LlmProvider => t!("secrets-usage-llm-provider"),
    };
    let target = u.label.clone().unwrap_or_else(|| u.consumer_id.clone());
    format!("{kind} · {target}")
}

/// Form target: `None` = closed, `Some(None)` = create, `Some(Some(id))` = edit.
type Editing = Option<Option<Uuid>>;

#[component]
pub fn Secrets() -> Element {
    let mut reload = use_signal(|| 0u32);
    let search = use_signal(String::new);
    let page = use_signal(|| 0i64);
    let mut editing = use_signal(|| Editing::None);
    let mut name = use_signal(String::new);
    let mut description = use_signal(String::new);
    let mut value = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut form_error = use_signal(|| Option::<ApiError>::None);
    let mut error = use_signal(|| Option::<ApiError>::None);
    let mut confirm_delete = use_signal(|| Option::<Uuid>::None);

    let can_manage = can_manage_secrets();

    let list = use_resource(move || {
        let term = search().trim().to_string();
        let offset = page() * PAGE_SIZE;
        async move {
            let _ = reload();
            let term = (!term.is_empty()).then_some(term);
            api::secrets::list(term.as_deref(), PAGE_SIZE, offset).await
        }
    });

    let (state, is_empty, count, rows) = match &*list.value().read() {
        None => (None, false, 0, Vec::new()),
        Some(Ok(paged)) => (
            Some(Ok(())),
            paged.count == 0,
            paged.count,
            paged.results.clone(),
        ),
        Some(Err(err)) => (Some(Err(err.clone())), false, 0, Vec::new()),
    };

    let is_edit = matches!(editing(), Some(Some(_)));
    let form_open = editing().is_some();
    let form_valid = !name().trim().is_empty() && (is_edit || !value().is_empty());

    let mut columns = vec![
        Column::new(t!("secrets-col-name").to_string(), |s: &OrgSecret| {
            let desc = s.description.clone();
            rsx! {
                div { class: "flex items-center gap-2 text-sm font-medium text-gray-900",
                    icons::Key { class: "h-4 w-4 text-gray-400" }
                    span { class: "font-mono break-all", {s.name.clone()} }
                }
                if let Some(desc) = desc {
                    div { class: "text-xs text-gray-500 mt-0.5", {desc} }
                }
            }
        }),
        Column::new(t!("secrets-col-used-by").to_string(), |s: &OrgSecret| {
            let usages: Vec<(Route, String)> = s
                .usages
                .iter()
                .map(|u| (usage_route(u.kind), usage_label(u)))
                .collect();
            rsx! {
                if usages.is_empty() {
                    span { class: "text-sm text-gray-400", {t!("secrets-unused")} }
                } else {
                    div { class: "flex flex-wrap gap-1",
                        for (route, label) in usages {
                            Link {
                                to: route,
                                class: "rounded bg-gray-100 px-2 py-0.5 text-xs text-gray-700 hover:bg-gray-200",
                                {label}
                            }
                        }
                    }
                }
            }
        }),
        Column::new(t!("secrets-col-updated").to_string(), |s: &OrgSecret| {
            let by = s.updated_by.clone();
            rsx! {
                div { class: "text-sm text-gray-600", {short_date(&s.updated_at)} }
                if let Some(by) = by {
                    div { class: "text-xs text-gray-400", {by} }
                }
            }
        })
        .secondary(),
    ];
    if can_manage {
        columns.push(Column::new(
            t!("common-actions").to_string(),
            move |s: &OrgSecret| {
                let id = s.id;
                let edit_name = s.name.clone();
                let edit_desc = s.description.clone().unwrap_or_default();
                rsx! {
                    div { class: "flex gap-1",
                        button {
                            class: "px-3 py-1 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
                            onclick: move |_| {
                                name.set(edit_name.clone());
                                description.set(edit_desc.clone());
                                value.set(String::new());
                                form_error.set(None);
                                editing.set(Some(Some(id)));
                            },
                            {t!("secrets-edit")}
                        }
                        if confirm_delete() == Some(id) {
                            button {
                                class: DANGER_BTN,
                                onclick: move |_| {
                                    spawn(async move {
                                        match api::secrets::delete(id).await {
                                            Ok(()) => {
                                                error.set(None);
                                                reload.with_mut(|r| *r += 1);
                                            }
                                            Err(e) => error.set(Some(e)),
                                        }
                                        confirm_delete.set(None);
                                    });
                                },
                                {t!("secrets-confirm-delete")}
                            }
                        } else {
                            button {
                                class: DANGER_BTN,
                                onclick: move |_| confirm_delete.set(Some(id)),
                                icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                                {t!("secrets-delete")}
                            }
                        }
                    }
                }
            },
        ).actions());
    }

    rsx! {
        ListLayout {
            title: t!("secrets-title").to_string(),
            subtitle: Some(t!("secrets-subtitle").to_string()),
            can_write: can_manage,
            add_label: Some(t!("secrets-new").to_string()),
            on_add: move |_| {
                name.set(String::new());
                description.set(String::new());
                value.set(String::new());
                form_error.set(None);
                editing.set(Some(None));
            },
            if form_open {
                FormDialog {
                    title: if is_edit { t!("secrets-edit-title").to_string() } else { t!("secrets-new-title").to_string() },
                    submit_label: t!("secrets-save").to_string(),
                    on_close: move |_| editing.set(None),
                    on_submit: move |_| {
                        let target = editing().flatten();
                        let input = OrgSecretInput {
                            name: name().trim().to_string(),
                            description: Some(description().trim().to_string())
                                .filter(|d| !d.is_empty()),
                            value: Some(value()).filter(|v| !v.is_empty()),
                        };
                        busy.set(true);
                        spawn(async move {
                            let result = match target {
                                Some(id) => api::secrets::update(id, &input).await,
                                None => api::secrets::create(&input).await,
                            };
                            busy.set(false);
                            match result {
                                Ok(_) => {
                                    value.set(String::new());
                                    editing.set(None);
                                    reload.with_mut(|r| *r += 1);
                                }
                                Err(e) => form_error.set(Some(e)),
                            }
                        });
                    },
                    busy: busy(),
                    valid: form_valid,
                    div { class: "space-y-4",
                        div {
                            label { class: "block text-sm font-medium text-gray-700 mb-1",
                                {t!("secrets-name")}
                            }
                            input {
                                class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm font-mono",
                                placeholder: t!("secrets-name-placeholder"),
                                value: "{name}",
                                oninput: move |e| name.set(e.value()),
                            }
                        }
                        div {
                            label { class: "block text-sm font-medium text-gray-700 mb-1",
                                {t!("secrets-description")}
                            }
                            input {
                                class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                value: "{description}",
                                oninput: move |e| description.set(e.value()),
                            }
                        }
                        div {
                            label { class: "block text-sm font-medium text-gray-700 mb-1",
                                if is_edit {
                                    {t!("secrets-new-value")}
                                } else {
                                    {t!("secrets-value")}
                                }
                            }
                            input {
                                class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm font-mono",
                                r#type: "password",
                                autocomplete: "new-password",
                                placeholder: if is_edit { t!("secrets-value-keep") } else { String::new() },
                                value: "{value}",
                                oninput: move |e| value.set(e.value()),
                            }
                            p { class: "text-xs text-gray-500 mt-1", {t!("secrets-value-hint")} }
                        }
                        if let Some(err) = form_error() {
                            div { class: "bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                                {crate::api::error_i18n::localize(&err)}
                            }
                        }
                    }
                }
            }
            FilterBar {
                SearchInput {
                    placeholder: t!("secrets-search-placeholder").to_string(),
                    value: search,
                    on_submit: move |_| reload.with_mut(|r| *r += 1),
                }
                RefreshButton { on_click: move |_| reload.with_mut(|r| *r += 1) }
            }
            if let Some(err) = error() {
                div { class: "bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-700 mb-6",
                    {crate::api::error_i18n::localize(&err)}
                }
            }
            ListStates {
                state,
                is_empty,
                empty_message: t!("secrets-empty").to_string(),
                empty_detail: rsx! {
                    p { class: "text-sm text-gray-400 mt-2", {t!("secrets-empty-hint")} }
                },
                div { class: "space-y-4",
                    DataTable {
                        columns,
                        rows,
                        row_key: RowKey::new(|s: &OrgSecret| s.id.to_string()),
                    }
                    ListPager { count, page }
                }
            }
        }
    }
}
