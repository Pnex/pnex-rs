//! Pages (pages.md #21): collaborative documents of the org. `/pages`
//! lists them; `/pages?id=<uuid>` opens the live editor ([`editor`]).

mod comments;
mod editor;
mod versions;

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api;
use crate::api::error::ApiError;
use crate::app::Route;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;
use pnex_core::pages::DocSummary;

/// `2026-10-01T13:44:11+02:00` → `2026-10-01 13:44`.
pub(crate) fn short_date(rfc3339: &str) -> String {
    rfc3339.get(..16).unwrap_or(rfc3339).replace('T', " ")
}

#[component]
pub fn Docs(id: String) -> Element {
    let parsed = Uuid::parse_str(&id).ok();
    rsx! {
        if let Some(id) = parsed {
            editor::DocEditor { key: "{id}", id }
        } else {
            DocList {}
        }
    }
}

#[component]
fn DocList() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut creating = use_signal(|| false);
    let mut title = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut form_error = use_signal(|| Option::<ApiError>::None);
    let mut error = use_signal(|| Option::<ApiError>::None);
    let mut confirm_delete = use_signal(|| Option::<Uuid>::None);
    let navigator = use_navigator();
    let can_write = crate::state::org::current_can_write();

    let list = use_resource(move || async move {
        let _ = reload();
        api::docs::list().await
    });
    let (state, rows) = match &*list.value().read() {
        None => (None, Vec::new()),
        Some(Ok(rows)) => (Some(Ok(())), rows.clone()),
        Some(Err(err)) => (Some(Err(err.clone())), Vec::new()),
    };
    let is_empty = matches!(state, Some(Ok(()))) && rows.is_empty();

    let delete = move |id: Uuid| {
        spawn(async move {
            match api::docs::delete(id).await {
                Ok(()) => {
                    error.set(None);
                    reload.with_mut(|r| *r += 1);
                }
                Err(e) => error.set(Some(e)),
            }
            confirm_delete.set(None);
        });
    };
    let submit = move |_| {
        let name = title().trim().to_string();
        busy.set(true);
        spawn(async move {
            let result = api::docs::create(name).await;
            busy.set(false);
            match result {
                Ok(doc) => {
                    creating.set(false);
                    navigator.push(Route::Docs {
                        id: doc.id.to_string(),
                    });
                }
                Err(e) => form_error.set(Some(e)),
            }
        });
    };
    let mut columns = vec![
        Column::new(t!("docs-col-title").to_string(), |d: &DocSummary| {
            let id = d.id.to_string();
            rsx! {
                Link {
                    to: Route::Docs { id },
                    class: "flex items-center gap-2 text-sm font-medium text-blue-700 hover:underline",
                    icons::BookOpen { class: "h-4 w-4 text-gray-400" }
                    {d.title.clone()}
                }
            }
        }),
        Column::new(t!("docs-col-updated").to_string(), |d: &DocSummary| {
            rsx! {
                span { class: "text-sm text-gray-600", {short_date(&d.updated_at)} }
            }
        })
        .secondary(),
    ];
    if can_write {
        columns.push(
            Column::new(t!("common-actions").to_string(), move |d: &DocSummary| {
                let id = d.id;
                rsx! {
                    if confirm_delete() == Some(id) {
                        button { class: DANGER_BTN, onclick: move |_| delete(id),
                            {t!("docs-confirm-delete")}
                        }
                    } else {
                        button {
                            class: DANGER_BTN,
                            onclick: move |_| confirm_delete.set(Some(id)),
                            icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                            {t!("docs-delete")}
                        }
                    }
                }
            })
            .actions(),
        );
    }

    rsx! {
        ListLayout {
            title: t!("docs-title").to_string(),
            subtitle: Some(t!("docs-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write,
            add_label: Some(t!("docs-new").to_string()),
            on_add: move |_| {
                title.set(String::new());
                form_error.set(None);
                creating.set(true);
            },
            if creating() {
                FormDialog {
                    title: t!("docs-new-title").to_string(),
                    submit_label: t!("docs-create").to_string(),
                    on_close: move |_| creating.set(false),
                    on_submit: submit,
                    busy: busy(),
                    valid: !title().trim().is_empty(),
                    div { class: "space-y-4",
                        div {
                            label {
                                r#for: "docs-field-title",
                                class: "block text-sm font-medium text-gray-700 mb-1",
                                {t!("docs-field-title")}
                            }
                            input {
                                id: "docs-field-title",
                                class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                value: "{title}",
                                oninput: move |e| title.set(e.value()),
                            }
                        }
                        if let Some(err) = form_error() {
                            div { class: "bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                                {crate::api::error_i18n::localize(&err)}
                            }
                        }
                    }
                }
            }
            if let Some(err) = error() {
                div { class: "bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-700 mb-6",
                    {crate::api::error_i18n::localize(&err)}
                }
            }
            ListStates {
                state,
                is_empty,
                empty_message: t!("docs-empty").to_string(),
                empty_detail: rsx! {
                    p { class: "text-sm text-gray-400 mt-2", {t!("docs-empty-hint")} }
                },
                DataTable {
                    columns,
                    rows,
                    row_key: RowKey::new(|d: &DocSummary| d.id.to_string()),
                }
            }
        }
    }
}
