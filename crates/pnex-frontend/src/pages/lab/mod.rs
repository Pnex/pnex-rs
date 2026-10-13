//! Page `/lab` — Vision Lab projects (media-vision-studio.md §4.6,
//! D198–D200): the datasets of custom object detectors. A project opens
//! its annotation workspace (`/lab/:id`, [`workspace`]).
//!
//! CRUD school (ListLayout + DataTable + Modal form + ConfirmDialog).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::lab::{LabProject, LabProjectInput};

use crate::api;
use crate::app::Route;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;
use crate::components::modal::Modal;
use crate::components::ui::{BTN_PRIMARY, BTN_SECONDARY, BTN_SECONDARY_SM, FIELD_LABEL, INPUT};
use crate::state::{org, toasts};

mod picker;
mod recording;
mod training;
mod video;
mod workspace;

pub use workspace::LabProjectPage;

/// One class per line, blank lines dropped.
pub(crate) fn parse_classes(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

#[component]
pub fn Lab() -> Element {
    let mut reload = use_signal(|| 0u32);
    // None = closed, Some(None) = creation, Some(Some(p)) = edition.
    let mut editing = use_signal(|| None::<Option<LabProject>>);
    let mut delete_target = use_signal(|| None::<LabProject>);
    let can_write = org::current_can_write();

    let list = use_resource(move || {
        let _ = reload();
        let has_org = org::current().is_some();
        async move {
            if !has_org {
                return None;
            }
            Some(api::lab::projects().await)
        }
    });
    let rows: Vec<LabProject> = match &*list.read() {
        Some(Some(Ok(rows))) => rows.clone(),
        _ => Vec::new(),
    };
    let list_state = match &*list.read() {
        None | Some(None) => None,
        Some(Some(Ok(_))) => Some(Ok(())),
        Some(Some(Err(e))) => Some(Err(e.clone())),
    };
    let is_empty = rows.is_empty();
    let modal = editing.read().clone();
    let deleting = delete_target.read().clone();

    let columns = vec![
        Column::new(t!("lab-col-project").to_string(), |p: &LabProject| {
            rsx! {
                div { class: "text-sm font-medium text-gray-900", "{p.name}" }
                if !p.description.is_empty() {
                    div { class: "text-xs text-gray-500 truncate max-w-md", "{p.description}" }
                }
            }
        }),
        Column::new(t!("lab-col-classes").to_string(), |p: &LabProject| {
            let classes = p.classes.join(", ");
            rsx! {
                span { class: "text-sm text-gray-700", "{classes}" }
            }
        })
        .secondary(),
        Column::new(t!("lab-col-images").to_string(), |p: &LabProject| {
            let text = t!("lab-images-count", annotated : p.annotated_count, total : p.image_count)
                .to_string();
            rsx! {
                span { class: "text-sm text-gray-700 tabular-nums", "{text}" }
            }
        }),
        Column::new(t!("lab-col-versions").to_string(), |p: &LabProject| {
            rsx! {
                span { class: "text-sm text-gray-700 tabular-nums", "{p.version_count}" }
            }
        })
        .secondary(),
        Column::new(t!("common-actions").to_string(), move |p: &LabProject| {
            let open = p.id.clone();
            let edit = p.clone();
            let del = p.clone();
            rsx! {
                div { class: "flex items-center justify-end gap-2",
                    Link {
                        class: BTN_SECONDARY_SM,
                        to: Route::LabProjectPage { id: open },
                        {t!("lab-open")}
                    }
                    if can_write {
                        button {
                            class: BTN_SECONDARY_SM,
                            onclick: move |_| editing.set(Some(Some(edit.clone()))),
                            icons::Wrench { class: "h-4 w-4 mr-1" }
                            {t!("lab-edit-short")}
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
        .with_td_class("text-right whitespace-nowrap")
        .actions()
        .header_sr_only(),
    ];

    rsx! {
        ListLayout {
            title: t!("nav-lab").to_string(),
            subtitle: Some(t!("lab-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write,
            add_label: Some(t!("lab-new").to_string()),
            on_add: move |_| editing.set(Some(None)),
            if org::current().is_none() {
                p { class: "text-gray-500 text-center py-12", {t!("orgs-empty")} }
            } else {
                ListStates {
                    state: list_state,
                    is_empty,
                    empty_message: t!("lab-empty").to_string(),
                    DataTable {
                        columns,
                        rows,
                        row_key: RowKey::new(|p: &LabProject| p.id.clone()),
                    }
                }
            }
            if let Some(initial) = modal {
                ProjectForm {
                    initial,
                    on_close: move |_| editing.set(None),
                    on_saved: move |_| {
                        editing.set(None);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
            if let Some(p) = deleting {
                ConfirmDialog {
                    title: t!("lab-delete").to_string(),
                    message: t!("lab-delete-confirm", name : p.name.clone()).to_string(),
                    confirm_label: t!("common-delete").to_string(),
                    on_confirm: move |_| {
                        let id = p.id.clone();
                        let done = t!("lab-deleted").to_string();
                        spawn(async move {
                            match api::lab::delete(&id).await {
                                Ok(_) => {
                                    toasts::success(done);
                                    reload.with_mut(|r| *r += 1);
                                }
                                Err(e) => toasts::error(e),
                            }
                            delete_target.set(None);
                        });
                    },
                    on_cancel: move |_| delete_target.set(None),
                }
            }
        }
    }
}

/// Creation / edition form: name, description, classes (one per line).
/// Classes are positional: rename in place, add at the end; only the last
/// ones can be removed, and only when no box uses them.
#[component]
pub(crate) fn ProjectForm(
    initial: Option<LabProject>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let editing_id = initial.as_ref().map(|p| p.id.clone());
    let init = initial.clone();
    let mut name = use_signal(|| init.as_ref().map(|p| p.name.clone()).unwrap_or_default());
    let mut description = use_signal(|| {
        init.as_ref()
            .map(|p| p.description.clone())
            .unwrap_or_default()
    });
    let mut classes = use_signal(|| {
        init.as_ref()
            .map(|p| p.classes.join("\n"))
            .unwrap_or_default()
    });
    let mut busy = use_signal(|| false);
    let input = LabProjectInput {
        name: name(),
        description: description(),
        classes: parse_classes(&classes()),
    };
    let ok = input.check().is_ok() && !busy();
    let title = if editing_id.is_some() {
        t!("lab-edit").to_string()
    } else {
        t!("lab-new").to_string()
    };

    let submit = move |_| {
        let input = LabProjectInput {
            name: name.peek().clone(),
            description: description.peek().clone(),
            classes: parse_classes(&classes.peek()),
        };
        let id = editing_id.clone();
        busy.set(true);
        let saved = t!("lab-saved").to_string();
        spawn(async move {
            let res = match id {
                Some(id) => api::lab::update(&id, &input).await,
                None => api::lab::create(&input).await,
            };
            busy.set(false);
            match res {
                Ok(_) => {
                    toasts::success(saved);
                    on_saved.call(());
                }
                Err(e) => toasts::error(e),
            }
        });
    };

    rsx! {
        Modal {
            title,
            max_width: "max-w-lg".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "space-y-3",
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("lab-form-name")} }
                    input {
                        class: "{INPUT} w-full",
                        maxlength: "120",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("lab-form-description")} }
                    textarea {
                        class: "{INPUT} w-full",
                        rows: "2",
                        value: "{description}",
                        oninput: move |e| description.set(e.value()),
                    }
                }
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("lab-form-classes")} }
                    textarea {
                        class: "{INPUT} w-full font-mono",
                        rows: "5",
                        placeholder: "red_valve\nsafety_helmet",
                        value: "{classes}",
                        oninput: move |e| classes.set(e.value()),
                    }
                }
                p { class: "-mt-2 text-xs text-gray-500", {t!("lab-form-classes-help")} }
                div { class: "flex justify-end gap-2 pt-2",
                    button {
                        class: BTN_SECONDARY,
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: BTN_PRIMARY,
                        r#type: "button",
                        disabled: !ok,
                        onclick: submit,
                        {t!("common-save")}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_one_per_line() {
        assert_eq!(
            parse_classes(" valve \n\nhelmet\n"),
            vec!["valve", "helmet"]
        );
    }
}
