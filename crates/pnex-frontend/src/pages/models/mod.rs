//! Models page (camera-video.md D81): the org's vision model registry —
//! D14 list, create (existing `model` media asset, or direct ONNX upload),
//! spec edit, delete, and "test with an image" (boxes drawn over the
//! picture). Writes are owner/admin only (the server enforces, the UI
//! hides).
//!
//! D100–D104: each row shows the model check (valid / invalid with the
//! runtime diagnostic / never checked), the inference time measured on the
//! server and the sustainable rate; "Check" re-runs it, "Live test" runs
//! the model on a camera.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::vision::{MlModel, ModelCheckStatus};

use crate::api;
use crate::components::badges::date_label;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{ACTIONS_TD_CLASS, ACTIONS_TH_CLASS};
use crate::components::icons;
use crate::state::{org, session, toasts};

mod form;
mod test_image;
mod test_live;

use form::ModelFormModal;
use test_image::TestImageModal;
use test_live::TestLiveModal;

/// User role in the current org (media/devices school).
fn current_role() -> Option<String> {
    let user = session::user()?;
    let org_id = org::current()?;
    user.orgs
        .iter()
        .find(|m| m.id == org_id)
        .map(|m| m.role.clone())
}

/// Which modal is open.
#[derive(Clone, PartialEq)]
enum Dialog {
    Create,
    Edit(MlModel),
    Test(MlModel),
    Live(MlModel),
    Delete(MlModel),
}

#[component]
pub fn Models() -> Element {
    let page = use_signal(|| 0i64);
    let mut reload = use_signal(|| 0u32);
    let mut dialog = use_signal(|| None::<Dialog>);
    let can_write = current_role().is_some_and(|role| crate::state::org::role_can_write(&role));

    let models = use_resource(move || {
        let offset = page() * PAGE_SIZE;
        async move {
            let _ = reload();
            let _ = org::ORG.read();
            api::ml_models::list(Some(PAGE_SIZE), Some(offset)).await
        }
    });

    let (list_state, is_empty, count, rows) = match &*models.value().read() {
        None => (None, false, 0, Vec::new()),
        Some(Ok(p)) => (
            Some(Ok(())),
            p.results.is_empty(),
            p.count,
            p.results.clone(),
        ),
        Some(Err(err)) => (Some(Err(err.clone())), false, 0, Vec::new()),
    };

    rsx! {
        ListLayout {
            title: t!("models-title").to_string(),
            subtitle: Some(t!("models-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write,
            add_label: Some(t!("models-add").to_string()),
            on_add: move |_| dialog.set(Some(Dialog::Create)),
            ListStates {
                state: list_state,
                is_empty,
                empty_message: t!("models-empty-title").to_string(),
                empty_icon: rsx! {
                    icons::Eye { class: "h-8 w-8 text-gray-400" }
                },
                empty_detail: rsx! {
                    p { class: "text-gray-600 mt-2 max-w-xl mx-auto", {t!("models-empty-message")} }
                },
                div { class: "space-y-4",
                    div { class: "overflow-x-auto bg-white rounded-lg shadow border border-gray-200",
                        table { class: "min-w-full divide-y divide-gray-200 text-sm",
                            thead { class: "bg-gray-50",
                                tr {
                                    th { class: "th", {t!("models-col-name")} }
                                    th { class: "th", {t!("models-col-family")} }
                                    th { class: "th hidden md:table-cell", {t!("models-col-input")} }
                                    th { class: "th hidden md:table-cell", {t!("models-col-labels")} }
                                    th { class: "th hidden md:table-cell",
                                        {t!("models-col-threshold")}
                                    }
                                    th { class: "th", {t!("models-col-check")} }
                                    th { class: "th hidden md:table-cell",
                                        {t!("models-col-updated")}
                                    }
                                    th { class: "th {ACTIONS_TH_CLASS}" }
                                }
                            }
                            tbody { class: "divide-y divide-gray-100",
                                for m in rows {
                                    ModelRow {
                                        key: "{m.id}",
                                        model: m.clone(),
                                        can_write,
                                        on_action: move |d: Dialog| dialog.set(Some(d)),
                                        on_checked: move |_| reload.with_mut(|r| *r += 1),
                                    }
                                }
                            }
                        }
                    }
                    ListPager { count, page }
                }
            }
        }
        match dialog() {
            Some(Dialog::Create) => rsx! {
                ModelFormModal {
                    existing: None,
                    on_close: move |_| dialog.set(None),
                    on_saved: move |_| {
                        dialog.set(None);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            },
            Some(Dialog::Edit(m)) => rsx! {
                ModelFormModal {
                    key: "edit-{m.id}",
                    existing: Some(m.clone()),
                    on_close: move |_| dialog.set(None),
                    on_saved: move |_| {
                        dialog.set(None);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            },
            Some(Dialog::Test(m)) => rsx! {
                TestImageModal {
                    key: "test-{m.id}",
                    model: m.clone(),
                    on_close: move |_| dialog.set(None),
                }
            },
            Some(Dialog::Live(m)) => rsx! {
                TestLiveModal {
                    key: "live-{m.id}",
                    model: m.clone(),
                    can_write,
                    on_close: move |_| dialog.set(None),
                    on_threshold_saved: move |_| reload.with_mut(|r| *r += 1),
                }
            },
            Some(Dialog::Delete(m)) => rsx! {
                ConfirmDialog {
                    title: t!("models-delete-title"),
                    message: t!("models-delete-message", name : m.name.clone()),
                    confirm_label: t!("models-delete-confirm"),
                    on_confirm: {
                        let id = m.id.clone();
                        move |_| {
                            let id = id.clone();
                            dialog.set(None);
                            let done = t!("models-deleted").to_string();
                            let failed = t!("models-delete-failed").to_string();
                            spawn(async move {
                                match api::ml_models::delete(&id).await {
                                    Ok(_) => {
                                        toasts::success(done);
                                        reload.with_mut(|r| *r += 1);
                                    }
                                    Err(err) => toasts::error(format!("{failed} : {}", err.message)),
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
fn ModelRow(
    model: MlModel,
    can_write: bool,
    on_action: Callback<Dialog>,
    on_checked: Callback<()>,
) -> Element {
    let m_test = model.clone();
    let m_live = model.clone();
    let check_id = model.id.clone();
    let mut checking = use_signal(|| false);
    let check = model.check.clone();
    let (badge_class, badge_label) = match check.status {
        ModelCheckStatus::Valid => ("bg-emerald-100 text-emerald-800", t!("models-check-valid")),
        ModelCheckStatus::Invalid => ("bg-red-100 text-red-800", t!("models-check-invalid")),
        ModelCheckStatus::Unchecked => ("bg-gray-100 text-gray-700", t!("models-check-unchecked")),
    };
    let speed = check
        .infer_ms
        .zip(check.max_fps())
        .map(|(ms, fps)| t!("models-check-speed", ms: ms, fps: format!("{fps:.1}")));
    let m_edit = model.clone();
    let m_delete = model.clone();
    rsx! {
        tr { class: "group hover:bg-gray-50",
            td { class: "td",
                p { class: "font-medium text-gray-900", "{model.name}" }
                if !model.description.is_empty() {
                    p { class: "text-xs text-gray-500", "{model.description}" }
                }
            }
            td { class: "td text-gray-600", {model.spec.family.wire().to_uppercase()} }
            td { class: "td hidden text-gray-600 md:table-cell",
                "{model.spec.input_width}×{model.spec.input_height}"
            }
            td { class: "td hidden text-gray-600 md:table-cell", "{model.spec.labels.len()}" }
            td { class: "td hidden text-gray-600 md:table-cell", "{model.spec.score_threshold:.2}" }
            td { class: "td min-w-[8rem] max-w-xs",
                span { class: "inline-flex px-2 py-0.5 rounded text-xs font-medium {badge_class}",
                    {badge_label}
                }
                if let Some(speed) = speed {
                    p { class: "text-xs text-gray-500 mt-0.5", {speed} }
                }
                if let Some(err) = check.error.clone() {
                    p {
                        class: "text-xs text-red-700 mt-0.5 break-words",
                        title: "{err}",
                        "{err}"
                    }
                }
            }
            td { class: "td hidden text-gray-500 md:table-cell", {date_label(&model.updated_at)} }
            td { class: "td {ACTIONS_TD_CLASS}",
                // Phones: icon-only buttons (label in title / aria-label),
                // otherwise five labelled buttons push the sticky column
                // past the screen edge.
                div { class: "flex justify-end gap-2",
                    button {
                        title: t!("models-test"),
                        aria_label: t!("models-test"),
                        class: "inline-flex items-center px-3 py-1 text-sm text-fuchsia-700 bg-fuchsia-50 border border-fuchsia-200 rounded-lg hover:bg-fuchsia-100 whitespace-nowrap",
                        r#type: "button",
                        onclick: move |_| on_action.call(Dialog::Test(m_test.clone())),
                        icons::Image { class: "h-4 w-4 sm:hidden" }
                        span { class: "hidden sm:inline", {t!("models-test")} }
                    }
                    button {
                        title: t!("models-test-live"),
                        aria_label: t!("models-test-live"),
                        class: "inline-flex items-center px-3 py-1 text-sm text-emerald-700 bg-emerald-50 border border-emerald-200 rounded-lg hover:bg-emerald-100 whitespace-nowrap",
                        r#type: "button",
                        onclick: move |_| on_action.call(Dialog::Live(m_live.clone())),
                        icons::Video { class: "h-4 w-4 sm:hidden" }
                        span { class: "hidden sm:inline", {t!("models-test-live")} }
                    }
                    if can_write {
                        button {
                            title: t!("models-check"),
                            aria_label: t!("models-check"),
                            class: "inline-flex items-center px-3 py-1 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50 whitespace-nowrap",
                            r#type: "button",
                            disabled: checking(),
                            onclick: move |_| {
                                let id = check_id.clone();
                                let failed = t!("models-check-failed").to_string();
                                spawn(async move {
                                    checking.set(true);
                                    if let Err(err) = api::ml_models::check(&id).await {
                                        toasts::error(format!("{failed} : {}", err.message));
                                    }
                                    checking.set(false);
                                    on_checked.call(());
                                });
                            },
                            icons::RefreshCw { class: "h-4 w-4 sm:hidden" }
                            span { class: "hidden sm:inline",
                                if checking() {
                                    {t!("models-checking")}
                                } else {
                                    {t!("models-check")}
                                }
                            }
                        }
                    }
                    if can_write {
                        button {
                            title: t!("models-edit"),
                            aria_label: t!("models-edit"),
                            class: "inline-flex items-center px-3 py-1 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50",
                            r#type: "button",
                            onclick: move |_| on_action.call(Dialog::Edit(m_edit.clone())),
                            icons::Pencil { class: "h-4 w-4 sm:hidden" }
                            span { class: "hidden sm:inline", {t!("models-edit")} }
                        }
                        button {
                            class: DANGER_BTN,
                            title: t!("models-delete"),
                            aria_label: t!("models-delete"),
                            r#type: "button",
                            onclick: move |_| on_action.call(Dialog::Delete(m_delete.clone())),
                            icons::Trash2 { class: "h-4 w-4 sm:hidden" }
                            span { class: "hidden sm:inline", {t!("models-delete")} }
                        }
                    }
                }
            }
        }
    }
}
