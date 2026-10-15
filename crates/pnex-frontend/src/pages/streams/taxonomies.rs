//! "Taxonomies" tab of /streams (media-ingest.md D168): versioned topic
//! lists. Editing writes a NEW version on top of the one shown (409 when
//! someone saved in between); old versions stay in the history.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::taxonomy::{Taxonomy, TaxonomyInput, TaxonomyVersionInput, Topic};

use crate::api;
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::DANGER_BTN;
use crate::components::crud::pager::{page_slice, ListPager};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::modal::Modal;
use crate::state::toasts;

const INPUT: &str = crate::components::ui::INPUT;
const CELL_INPUT: &str = "w-full px-2 py-1 border border-gray-300 rounded text-sm bg-white";
const LABEL: &str = "block text-xs text-gray-600 mb-1";
const BTN: &str =
    "px-2 py-1 text-xs border border-gray-300 rounded-lg hover:bg-gray-50 disabled:opacity-40";

/// Which dialog is open.
#[derive(Clone, PartialEq)]
enum Dialog {
    Edit(Taxonomy),
    History(Taxonomy),
    Delete(Taxonomy),
}

#[component]
pub fn TaxonomiesTab(can_write: bool, reload: Signal<u32>, creating: Signal<bool>) -> Element {
    let mut reload = reload;
    let mut creating = creating;
    let mut dialog = use_signal(|| None::<Dialog>);
    let list = use_resource(move || async move {
        let _ = reload();
        api::taxonomies::list().await
    });
    let page = use_signal(|| 0i64);
    let (state, count, rows) = match &*list.value().read() {
        None => (None, 0, Vec::new()),
        Some(Ok(all)) => (Some(Ok(())), all.len() as i64, page_slice(all, page())),
        Some(Err(err)) => (Some(Err(err.clone())), 0, Vec::new()),
    };
    let columns = vec![
        Column::new(t!("taxonomies-col-name").to_string(), |tx: &Taxonomy| {
            rsx! {
                p { class: "font-medium text-gray-900", "{tx.name}" }
                if !tx.description.is_empty() {
                    p { class: "text-xs text-gray-500", "{tx.description}" }
                }
            }
        }),
        Column::new(t!("taxonomies-col-version").to_string(), |tx: &Taxonomy| {
            let version = tx.current_version.to_string();
            rsx! { {t!("taxonomies-version", version : version)} }
        })
        .with_td_class("text-gray-600"),
        Column::new(t!("taxonomies-col-topics").to_string(), |tx: &Taxonomy| {
            let topics = tx.current.as_ref().map_or(0, |v| v.topics.len());
            rsx! { "{topics}" }
        })
        .with_td_class("text-gray-600")
        .secondary(),
        Column::new(t!("common-actions").to_string(), move |tx: &Taxonomy| {
            let (t_edit, t_history, t_delete) = (tx.clone(), tx.clone(), tx.clone());
            rsx! {
                div { class: "flex justify-end gap-2",
                    button {
                        class: BTN,
                        r#type: "button",
                        onclick: move |_| dialog.set(Some(Dialog::History(t_history.clone()))),
                        {t!("taxonomies-history")}
                    }
                    if can_write {
                        button {
                            class: BTN,
                            r#type: "button",
                            onclick: move |_| dialog.set(Some(Dialog::Edit(t_edit.clone()))),
                            {t!("taxonomies-new-version")}
                        }
                        button {
                            class: DANGER_BTN,
                            r#type: "button",
                            onclick: move |_| dialog.set(Some(Dialog::Delete(t_delete.clone()))),
                            {t!("streams-delete")}
                        }
                    }
                }
            }
        })
        .actions(),
    ];

    rsx! {
        div { class: "space-y-2",
            p { class: "text-xs text-gray-500", {t!("taxonomies-help")} }
            ListStates {
                state,
                is_empty: count == 0,
                empty_message: t!("taxonomies-empty").to_string(),
                DataTable {
                    columns,
                    rows,
                    row_key: RowKey::new(|tx: &Taxonomy| tx.id.clone()),
                }
                ListPager { count, page }
            }
        }
        if creating() {
            CreateTaxonomy {
                on_close: move |_| creating.set(false),
                on_saved: move |_| {
                    creating.set(false);
                    reload.with_mut(|r| *r += 1);
                },
            }
        }
        match dialog() {
            Some(Dialog::Edit(tx)) => rsx! {
                VersionForm {
                    key: "edit-{tx.id}",
                    taxonomy: tx.clone(),
                    on_close: move |_| dialog.set(None),
                    on_saved: move |_| {
                        dialog.set(None);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            },
            Some(Dialog::History(tx)) => rsx! {
                History {
                    key: "history-{tx.id}",
                    taxonomy: tx.clone(),
                    on_close: move |_| dialog.set(None),
                }
            },
            Some(Dialog::Delete(tx)) => rsx! {
                ConfirmDialog {
                    title: t!("taxonomies-delete-title"),
                    message: t!("taxonomies-delete-message", name : tx.name.clone()),
                    confirm_label: t!("streams-delete-confirm"),
                    on_confirm: {
                        let id = tx.id.clone();
                        move |_| {
                            let id = id.clone();
                            dialog.set(None);
                            spawn(async move {
                                match api::taxonomies::delete(&id).await {
                                    Ok(_) => reload.with_mut(|r| *r += 1),
                                    Err(err) => toasts::error(err),
                                }
                            });
                        }
                    },
                    on_cancel: move |_| dialog.set(None),
                }
            },
            None => rsx! {},
        }
    }
}

#[component]
fn CreateTaxonomy(on_close: Callback<()>, on_saved: Callback<()>) -> Element {
    let mut name = use_signal(String::new);
    let mut description = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let save = move |_| {
        let input = TaxonomyInput {
            name: Some(name()),
            description: Some(description()),
        };
        busy.set(true);
        spawn(async move {
            let res = api::taxonomies::create(&input).await;
            busy.set(false);
            match res {
                Ok(_) => on_saved.call(()),
                Err(err) => toasts::error(err),
            }
        });
    };
    let valid = !name().trim().is_empty();
    rsx! {
        FormDialog {
            title: t!("taxonomies-create-title").to_string(),
            submit_label: t!("streams-save").to_string(),
            on_close,
            on_submit: save,
            busy: busy(),
            valid,
            div {
                label { r#for: "taxonomy-name", class: LABEL, {t!("taxonomies-name")} }
                input {
                    id: "taxonomy-name",
                    class: INPUT,
                    r#type: "text",
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                }
            }
            div {
                label { r#for: "taxonomy-description", class: LABEL, {t!("taxonomies-description")} }
                textarea {
                    id: "taxonomy-description",
                    class: INPUT,
                    rows: "2",
                    value: "{description}",
                    oninput: move |e| description.set(e.value()),
                }
            }
        }
    }
}

/// Editable row of the topics table (keywords comma-separated).
#[derive(Clone, Default, PartialEq)]
struct Draft {
    id: String,
    label: String,
    definition: String,
    keywords: String,
}

fn draft_of(t: &Topic) -> Draft {
    Draft {
        id: t.id.clone(),
        label: t.label.clone(),
        definition: t.definition.clone(),
        keywords: t.keywords.join(", "),
    }
}

fn topic_of(d: &Draft) -> Topic {
    Topic {
        id: d.id.trim().to_string(),
        label: d.label.clone(),
        definition: d.definition.clone(),
        keywords: d
            .keywords
            .split(',')
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(str::to_string)
            .collect(),
    }
}

/// New version of a taxonomy, prefilled with its current topics.
#[component]
fn VersionForm(taxonomy: Taxonomy, on_close: Callback<()>, on_saved: Callback<()>) -> Element {
    let initial: Vec<Draft> = taxonomy
        .current
        .as_ref()
        .map(|v| v.topics.iter().map(draft_of).collect())
        .filter(|d: &Vec<Draft>| !d.is_empty())
        .unwrap_or_else(|| vec![Draft::default()]);
    let mut rows = use_signal(move || initial);
    let mut note = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let id = taxonomy.id.clone();
    let expected = taxonomy.current_version;
    let save = move |_| {
        let input = TaxonomyVersionInput {
            topics: rows.peek().iter().map(topic_of).collect(),
            note: note(),
            expected_version: expected,
        };
        let id = id.clone();
        busy.set(true);
        spawn(async move {
            let res = api::taxonomies::add_version(&id, &input).await;
            busy.set(false);
            match res {
                Ok(_) => {
                    toasts::success(t!("taxonomies-saved").to_string());
                    on_saved.call(());
                }
                Err(err) => toasts::error(err),
            }
        });
    };
    let next = (expected + 1).to_string();
    let title =
        t!("taxonomies-edit-title", name : taxonomy.name.clone(), version : next).to_string();
    let drafts = rows();
    rsx! {
        FormDialog {
            title,
            submit_label: t!("taxonomies-save-version").to_string(),
            on_close,
            on_submit: save,
            busy: busy(),
            max_width: "max-w-5xl".to_string(),
            p { class: "text-xs text-gray-500", {t!("taxonomies-edit-help")} }
            div { class: "overflow-x-auto",
                table { class: "min-w-full text-sm",
                    thead {
                        tr {
                            th { class: "th", {t!("taxonomies-topic-id")} }
                            th { class: "th", {t!("taxonomies-topic-label")} }
                            th { class: "th", {t!("taxonomies-topic-definition")} }
                            th { class: "th", {t!("taxonomies-topic-keywords")} }
                            th { class: "th" }
                        }
                    }
                    tbody {
                        for (i, d) in drafts.into_iter().enumerate() {
                            tr { key: "{i}",
                                td { class: "py-1 pr-2 align-top w-32",
                                    input {
                                        class: CELL_INPUT,
                                        r#type: "text",
                                        value: "{d.id}",
                                        aria_label: t!("taxonomies-topic-id").to_string(),
                                        oninput: move |e| rows.with_mut(|r| r[i].id = e.value()),
                                    }
                                }
                                td { class: "py-1 pr-2 align-top w-40",
                                    input {
                                        class: CELL_INPUT,
                                        r#type: "text",
                                        value: "{d.label}",
                                        aria_label: t!("taxonomies-topic-label").to_string(),
                                        oninput: move |e| rows.with_mut(|r| r[i].label = e.value()),
                                    }
                                }
                                td { class: "py-1 pr-2 align-top",
                                    textarea {
                                        class: CELL_INPUT,
                                        rows: "2",
                                        value: "{d.definition}",
                                        aria_label: t!("taxonomies-topic-definition").to_string(),
                                        oninput: move |e| rows.with_mut(|r| r[i].definition = e.value()),
                                    }
                                }
                                td { class: "py-1 pr-2 align-top",
                                    textarea {
                                        class: CELL_INPUT,
                                        rows: "2",
                                        value: "{d.keywords}",
                                        aria_label: t!("taxonomies-topic-keywords").to_string(),
                                        oninput: move |e| rows.with_mut(|r| r[i].keywords = e.value()),
                                    }
                                }
                                td { class: "py-1 align-top",
                                    button {
                                        class: DANGER_BTN,
                                        r#type: "button",
                                        onclick: move |_| rows.with_mut(|r| _ = r.remove(i)),
                                        {t!("taxonomies-topic-remove")}
                                    }
                                }
                            }
                        }
                    }
                }
            }
            button {
                class: BTN,
                r#type: "button",
                onclick: move |_| rows.with_mut(|r| r.push(Draft::default())),
                {t!("taxonomies-topic-add")}
            }
            div {
                label { r#for: "taxonomy-note", class: LABEL, {t!("taxonomies-note")} }
                input {
                    id: "taxonomy-note",
                    class: INPUT,
                    r#type: "text",
                    value: "{note}",
                    oninput: move |e| note.set(e.value()),
                }
            }
        }
    }
}

/// Versions of a taxonomy, newest first, with their topics.
#[component]
fn History(taxonomy: Taxonomy, on_close: Callback<()>) -> Element {
    let id = taxonomy.id.clone();
    let versions = use_resource(move || {
        let id = id.clone();
        async move { api::taxonomies::versions(&id).await }
    });
    let rows: Vec<(pnex_core::taxonomy::TaxonomyVersion, String)> = match &*versions.value().read()
    {
        Some(Ok(v)) => v
            .iter()
            .map(|v| {
                let ids: Vec<&str> = v.topics.iter().map(|t| t.id.as_str()).collect();
                (v.clone(), ids.join(", "))
            })
            .collect(),
        _ => Vec::new(),
    };
    rsx! {
        Modal {
            title: t!("taxonomies-history-title", name : taxonomy.name.clone()).to_string(),
            max_width: "max-w-3xl".to_string(),
            on_close,
            if rows.is_empty() {
                p { class: "text-sm text-gray-500", {t!("taxonomies-history-empty")} }
            }
            ul { class: "divide-y divide-gray-100",
                for (v, ids) in rows {
                    li { key: "{v.version}", class: "py-2 space-y-1",
                        p { class: "text-sm font-medium text-gray-900",
                            {t!("taxonomies-version", version : v.version.to_string())}
                            span { class: "ml-2 text-xs text-gray-500", {date_label(&v.created_at)} }
                        }
                        if !v.note.is_empty() {
                            p { class: "text-xs text-gray-600", "{v.note}" }
                        }
                        p { class: "text-xs text-gray-500 break-words", "{ids}" }
                    }
                }
            }
        }
    }
}
