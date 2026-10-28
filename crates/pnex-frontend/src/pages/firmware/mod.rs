//! Custom firmware IDE (custom-firmware.md D87–D94) — projects list and
//! editor sub-view (signal `selected`, same pattern as `functions`). One
//! `main.cpp` per project, versioned server-side (append-only revisions),
//! PneX library + pinned catalog libraries, compile-only "Verify" checks.
//! Writes are owner/admin only (the server enforces, the UI hides).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::firmware::{CreateFirmwareProject, FirmwareProjectSummary, CHIP_FAMILIES};

use crate::api;
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::filters::{FilterBar, LabelFilterInput, SearchInput};
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::{page_slice, ListPager};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;
use crate::components::labels_editor::{use_row_labels, LabelChips, LabelsButton};
use crate::state::{org, toasts};

mod editor;
mod panels;

use crate::components::ui::{FIELD_LABEL, INPUT};
use editor::FirmwareEditor;

/// Chip family badge label (wire id → display name, not translated).
pub(crate) fn chip_label(family: &str) -> &'static str {
    match family {
        "esp32" => "ESP32",
        "esp32-c3" => "ESP32-C3",
        "esp32-c6" => "ESP32-C6",
        "esp32-s3" => "ESP32-S3",
        "esp8266" => "ESP8266",
        _ => "?",
    }
}

#[component]
pub fn Firmware() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut selected = use_signal(|| None::<i64>);
    let search = use_signal(String::new);
    // D218: own label filter ("name" or "name:value"), applied server-side.
    let filter_label = use_signal(String::new);
    let mut create_open = use_signal(|| false);
    let mut delete_target = use_signal(|| None::<(i64, String)>);
    let can_write = crate::state::access::can("firmware-projects:write");

    let list = use_resource(move || {
        let _ = reload();
        let label = Some(filter_label().trim().to_string());
        async move { api::firmware::list_labeled(label).await }
    });

    // D218: own labels of the loaded rows (whole list, paged client-side).
    let row_labels =
        use_row_labels(
            pnex_core::resources::KIND_FIRMWARE_PROJECT,
            move || match &*list.value().read() {
                Some(Ok(all)) => all.iter().map(|p| p.id.to_string()).collect(),
                _ => Vec::new(),
            },
        );

    let mut page = use_signal(|| 0i64);
    // A new search term restarts from the first page.
    use_effect(move || {
        let _ = search.read();
        page.set(0);
    });
    let (list_state, is_empty, rows) = match &*list.value().read() {
        Some(Ok(all)) => {
            let needle = search().trim().to_lowercase();
            let rows: Vec<FirmwareProjectSummary> = all
                .iter()
                .filter(|p| needle.is_empty() || p.name.to_lowercase().contains(&needle))
                .cloned()
                .collect();
            (Some(Ok(())), all.is_empty(), rows)
        }
        Some(Err(err)) => (Some(Err(err.clone())), false, Vec::new()),
        None => (None, false, Vec::new()),
    };
    let count = rows.len() as i64;
    let rows = page_slice(&rows, page());

    let columns = vec![
        Column::new(
            t!("firmware-col-name").to_string(),
            move |p: &FirmwareProjectSummary| {
                let labels = row_labels.get(&p.id.to_string()).cloned().unwrap_or_default();
                rsx! {
                    {p.name.clone()}
                    LabelChips { labels }
                }
            },
        )
        .with_td_class("font-medium text-gray-900"),
        Column::new(
            t!("firmware-col-chip").to_string(),
            |p: &FirmwareProjectSummary| {
                rsx! {
                    span { class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-slate-100 text-slate-800 w-fit",
                        {chip_label(&p.chip_family)}
                    }
                }
            },
        ).secondary(),
        Column::new(
            t!("firmware-col-revision").to_string(),
            |p: &FirmwareProjectSummary| {
                rsx! { {format!("r{}", p.current_revision_number)} }
            },
        )
        .with_td_class("text-sm text-gray-600").secondary(),
        Column::new(
            t!("firmware-col-devices").to_string(),
            |p: &FirmwareProjectSummary| {
                rsx! { {p.device_count.to_string()} }
            },
        )
        .with_td_class("text-sm text-gray-600"),
        Column::new(
            t!("firmware-col-updated").to_string(),
            |p: &FirmwareProjectSummary| {
                rsx! { {date_label(&p.updated_at)} }
            },
        )
        .with_td_class("text-gray-500 text-sm").secondary(),
        Column::new(
            t!("common-actions").to_string(),
            move |p: &FirmwareProjectSummary| {
                let pk = p.id;
                let name_delete = p.name.clone();
                rsx! {
                    div { class: "flex items-center gap-1.5",
                        button {
                            class: crate::components::ui::BTN_SECONDARY_SM,
                            onclick: move |_| selected.set(Some(pk)),
                            {t!("firmware-open")}
                        }
                        LabelsButton {
                            kind: pnex_core::resources::KIND_FIRMWARE_PROJECT.to_string(),
                            id: pk.to_string(),
                            can_write,
                            on_changed: move |_| reload.with_mut(|r| *r += 1),
                        }
                        if can_write {
                            button {
                                class: DANGER_BTN,
                                onclick: move |_| delete_target.set(Some((pk, name_delete.clone()))),
                                icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                                {t!("firmware-delete")}
                            }
                        }
                    }
                }
            },
        ).actions(),
    ];

    rsx! {
        ListLayout {
            title: t!("nav-firmware").to_string(),
            subtitle: Some(t!("firmware-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            header: selected().is_none(),
            can_write,
            add_label: Some(t!("firmware-new").to_string()),
            on_add: move |_| create_open.set(true),
            if org::current().is_none() {
                p { class: "text-gray-500 text-center py-12", {t!("orgs-empty")} }
            } else {
                match selected() {
                    Some(project_id) => rsx! {
                        FirmwareEditor {
                            key: "{project_id}",
                            project_id,
                            can_write,
                            on_back: move |_| {
                                selected.set(None);
                                reload.with_mut(|r| *r += 1);
                            },
                        }
                    },
                    None => rsx! {
                        FilterBar {
                            SearchInput {
                                placeholder: t!("firmware-search-placeholder").to_string(),
                                value: search,
                                on_submit: move |_| {},
                            }
                            LabelFilterInput {
                                placeholder: t!("resources-label-filter-placeholder").to_string(),
                                value: filter_label,
                                on_submit: move |_| {
                                    page.set(0);
                                    reload.with_mut(|r| *r += 1);
                                },
                            }
                        }
                        ListStates {
                            state: list_state,
                            is_empty,
                            empty_message: t!("firmware-empty").to_string(),
                            DataTable {
                                columns,
                                rows,
                                row_key: RowKey::new(|p: &FirmwareProjectSummary| p.id.to_string()),
                            }
                            ListPager { count, page }
                        }
                        if let Some((project_id, project_name)) = delete_target() {
                            ConfirmDialog {
                                title: t!("firmware-confirm-delete-title"),
                                message: t!(
                                    "common-quoted-message", name : project_name.clone(), message :
                                    t!("firmware-confirm-delete-message")
                                ),
                                confirm_label: t!("firmware-delete"),
                                on_confirm: move |_| {
                                    delete_target.set(None);
                                    spawn(async move {
                                        match api::firmware::delete(project_id).await {
                                            Ok(()) => toasts::success("toast-firmware-deleted"),
                                            Err(err) => toasts::error(err),
                                        }
                                        reload.with_mut(|r| *r += 1);
                                    });
                                },
                                on_cancel: move |_| delete_target.set(None),
                            }
                        }
                        if create_open() {
                            CreateProjectModal {
                                on_close: move |_| create_open.set(false),
                                on_created: move |id| {
                                    create_open.set(false);
                                    selected.set(Some(id));
                                    reload.with_mut(|r| *r += 1);
                                },
                            }
                        }
                    },
                }
            }
        }
    }
}

/// Creation: name + chip family (+ description). Revision 1 starts from the
/// starter sketch (served by the backend).
#[component]
fn CreateProjectModal(on_close: Callback<()>, on_created: Callback<i64>) -> Element {
    let mut name = use_signal(String::new);
    let mut description = use_signal(String::new);
    let mut chip = use_signal(|| "esp32".to_string());
    let mut name_error = use_signal(|| false);
    let mut creating = use_signal(|| false);

    let submit = move |_| {
        let trimmed = name().trim().to_string();
        if trimmed.is_empty() {
            name_error.set(true);
            return;
        }
        creating.set(true);
        let params = CreateFirmwareProject {
            name: trimmed,
            chip_family: chip(),
            description: Some(description().trim().to_string()).filter(|d| !d.is_empty()),
            main_cpp: None,
            lib_deps: Vec::new(),
        };
        spawn(async move {
            match api::firmware::create(params).await {
                Ok(detail) => {
                    toasts::success("toast-firmware-created");
                    on_created.call(detail.id);
                }
                Err(err) => {
                    creating.set(false);
                    toasts::error(err);
                }
            }
        });
    };

    rsx! {
        FormDialog {
            title: t!("firmware-create-title").to_string(),
            submit_label: t!("common-create").to_string(),
            on_close,
            on_submit: submit,
            busy: creating(),
            label { class: "block",
                span { class: FIELD_LABEL, {t!("firmware-field-name")} }
                input {
                    class: if name_error() { "w-full px-3 py-2 border border-red-400 bg-red-50 rounded-lg text-sm" } else { "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm" },
                    r#type: "text",
                    value: "{name}",
                    oninput: move |event| {
                        name.set(event.value());
                        name_error.set(false);
                    },
                }
                if name_error() {
                    span { class: "text-xs text-red-600 mt-1 block",
                        {t!("firmware-field-name-required")}
                    }
                }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("firmware-field-chip")} }
                select {
                    class: "{INPUT} w-full",
                    value: "{chip}",
                    onchange: move |event| chip.set(event.value()),
                    for soc in CHIP_FAMILIES {
                        option { value: soc.name(), selected: chip() == soc.name(),
                            {chip_label(soc.name())}
                        }
                    }
                }
                span { class: "text-xs text-gray-500 mt-1 block", {t!("firmware-field-chip-hint")} }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("firmware-field-description")} }
                input {
                    class: "{INPUT} w-full",
                    r#type: "text",
                    value: "{description}",
                    oninput: move |event| description.set(event.value()),
                }
            }
        }
    }
}
