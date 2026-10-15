//! `/cases` — investigation cases (docs/architecture/cases-workspace.md): a
//! case holds dated events and undated pieces. The list creates a case from
//! a title dialog, or from an exported case file; the workspace (`workspace/`) is the timeline, the map,
//! the links and the inspector on one screen.

pub mod workspace;

use dioxus::prelude::*;
use dioxus_i18n::prelude::i18n;
use dioxus_i18n::t;
use pnex_core::ontology::api::{
    CaseExport, CaseSnapshotContent, FilterOp, ObjectInput, ObjectView, OntologyQuery,
    PropertyFilter,
};
use pnex_core::ontology::{CASE_QUESTION, CASE_STATUS, CASE_STATUSES, KIND_CASE};
use serde_json::{Map, Value};

use crate::api;
use crate::app::Route;
use crate::components::crud::filters::FilterBar;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ontology::short_time;
use crate::components::ui::{BTN_SECONDARY_SM, FIELD_LABEL, INPUT};
use crate::state::{org, toasts};

pub use workspace::CaseWorkspace;

/// Translated case status (`case-status-<status>`).
pub fn status_label(status: &str) -> String {
    i18n()
        .try_translate(&format!("case-status-{status}"))
        .unwrap_or_else(|_| status.to_string())
}

pub fn status_class(status: &str) -> &'static str {
    match status {
        "open" => "rounded px-2 py-0.5 text-xs bg-green-50 text-green-700",
        "concluded" => "rounded px-2 py-0.5 text-xs bg-signal-50 text-signal-700",
        _ => "rounded px-2 py-0.5 text-xs bg-gray-100 text-gray-600",
    }
}

fn prop_str(o: &ObjectView, key: &str) -> String {
    o.properties
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[component]
pub fn Cases() -> Element {
    let mut reload = use_signal(|| 0u32);
    // Empty = every status.
    let mut status = use_signal(|| "open".to_string());
    let mut draft = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut creating = use_signal(|| false);
    let mut page = use_signal(|| 0i64);
    let can_write = org::current_can_write();
    let navigator = use_navigator();

    let list = use_resource(move || {
        let st = status();
        let q = OntologyQuery {
            type_key: Some(KIND_CASE.to_string()),
            filters: if st.is_empty() {
                Vec::new()
            } else {
                vec![PropertyFilter {
                    key: CASE_STATUS.to_string(),
                    op: FilterOp::Eq,
                    value: Value::String(st),
                }]
            },
            limit: Some(PAGE_SIZE as u64),
            offset: Some((page() * PAGE_SIZE) as u64),
            ..Default::default()
        };
        async move {
            let _ = reload();
            api::ontology::query(&q).await
        }
    });
    let mut create = move || {
        let title = draft().trim().to_string();
        if title.is_empty() || busy() {
            return;
        }
        let mut props = Map::new();
        props.insert(CASE_STATUS.into(), Value::String("open".into()));
        let input = ObjectInput {
            type_key: KIND_CASE.to_string(),
            title,
            properties: props,
            expected_version: None,
        };
        busy.set(true);
        spawn(async move {
            let res = api::ontology::create_object(&input).await;
            busy.set(false);
            match res {
                Ok(o) => {
                    creating.set(false);
                    navigator.push(Route::CaseWorkspace { id: o.id });
                }
                Err(e) => toasts::error(e),
            }
        });
    };
    // A `pnex.case/1` export, or a frozen state holding one.
    let import = move |evt: FormEvent| async move {
        let Some(file) = evt.files().first().cloned() else {
            return;
        };
        let text = file.read_string().await.unwrap_or_default();
        let doc = serde_json::from_str::<CaseExport>(&text)
            .or_else(|_| serde_json::from_str::<CaseSnapshotContent>(&text).map(|c| c.export));
        let Ok(doc) = doc else {
            toasts::error(t!("cases-import-unreadable").to_string());
            return;
        };
        match api::cases::import_case(&doc).await {
            Ok(r) => {
                if !r.missing.is_empty() {
                    toasts::info(t!("cases-import-missing", count : r.missing.len()).to_string());
                }
                navigator.push(Route::CaseWorkspace { id: r.case.id });
            }
            Err(e) => toasts::error(e),
        }
    };
    let (state, count, rows) = match &*list.read() {
        None => (None, 0, Vec::new()),
        Some(Ok(r)) => (
            Some(Ok(())),
            r.count,
            r.rows
                .iter()
                .map(|r| r.object.clone())
                .collect::<Vec<ObjectView>>(),
        ),
        Some(Err(e)) => (Some(Err(e.clone())), 0, Vec::new()),
    };
    let is_empty = rows.is_empty();
    let columns = vec![
        Column::new(t!("cases-col-title").to_string(), |c: &ObjectView| {
            rsx! {
                span { class: "font-medium text-gray-900", "{c.title}" }
            }
        }),
        Column::new(t!("case-question").to_string(), |c: &ObjectView| {
            let question = prop_str(c, CASE_QUESTION);
            rsx! {
                span { class: "text-sm text-gray-600 line-clamp-2", "{question}" }
            }
        })
        .secondary(),
        Column::new(t!("cases-col-status").to_string(), |c: &ObjectView| {
            let st = prop_str(c, CASE_STATUS);
            rsx! {
                span { class: status_class(&st), {status_label(&st)} }
            }
        }),
        Column::new(t!("onto-col-updated").to_string(), |c: &ObjectView| {
            let at = short_time(&c.updated_at);
            rsx! {
                span { class: "text-xs text-gray-500", "{at}" }
            }
        })
        .secondary(),
    ];
    let mut filters: Vec<(String, String)> = CASE_STATUSES
        .iter()
        .map(|s| (s.to_string(), status_label(s)))
        .collect();
    filters.push((String::new(), t!("case-filter-all").to_string()));

    rsx! {
        ListLayout {
            title: t!("cases-title").to_string(),
            subtitle: t!("cases-subtitle").to_string(),
            can_write,
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            add_label: Some(t!("cases-new").to_string()),
            on_add: move |_| {
                draft.set(String::new());
                creating.set(true);
            },
            if creating() {
                FormDialog {
                    title: t!("cases-new").to_string(),
                    submit_label: t!("common-create").to_string(),
                    on_close: move |_| creating.set(false),
                    on_submit: move |_| create(),
                    busy: busy(),
                    valid: !draft().trim().is_empty(),
                    form {
                        onsubmit: move |e| {
                            e.prevent_default();
                            create();
                        },
                        label { r#for: "case-new-title", class: FIELD_LABEL, {t!("case-field-title")} }
                        input {
                            id: "case-new-title",
                            class: INPUT,
                            "data-testid": "case-new-title",
                            placeholder: t!("case-new-placeholder"),
                            maxlength: 255,
                            autofocus: true,
                            value: "{draft}",
                            oninput: move |e| draft.set(e.value()),
                        }
                    }
                }
            }
            FilterBar {
                if can_write {
                    label { class: "{BTN_SECONDARY_SM} cursor-pointer",
                        {t!("cases-import")}
                        input {
                            class: "hidden",
                            r#type: "file",
                            accept: ".json,application/json",
                            "data-testid": "case-import-file",
                            onchange: import,
                        }
                    }
                }
                select {
                    class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                    "data-testid": "case-status-filter",
                    aria_label: t!("case-filter-status"),
                    onchange: move |e| {
                        status.set(e.value());
                        page.set(0);
                    },
                    for (key, label) in filters {
                        option {
                            key: "{key}",
                            value: "{key}",
                            selected: status() == key,
                            "{label}"
                        }
                    }
                }
            }
            ListStates {
                state,
                is_empty,
                empty_message: t!("cases-empty").to_string(),
                div { class: "relative",
                    DataTable {
                        columns,
                        rows,
                        row_key: RowKey::new(|c: &ObjectView| c.id.clone()),
                        on_row_click: move |id: String| {
                            navigator.push(Route::CaseWorkspace { id });
                        },
                    }
                    ListPager { count, page }
                }
            }
        }
    }
}
