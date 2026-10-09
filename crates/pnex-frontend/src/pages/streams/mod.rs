//! Streams page (media-ingest.md P2.13, §7): the org's captured audio
//! streams — D14 list with capture state, create / edit (URL, vault
//! secret, transcription profile, retention, TDM check), enable, delete.
//! The server enforces roles; the UI hides writes from viewers.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::media_ingest::{CaptureState, MediaStream, MediaStreamInput};

use crate::api;
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{ACTIONS_TD_CLASS, ACTIONS_TH_CLASS};
use crate::components::icons;
use crate::state::{org, session, toasts};

mod form;

use form::StreamFormModal;

/// User role in the current org.
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
    Edit(MediaStream),
    Delete(MediaStream),
}

#[component]
pub fn Streams() -> Element {
    let page = use_signal(|| 0i64);
    let mut reload = use_signal(|| 0u32);
    let mut dialog = use_signal(|| None::<Dialog>);
    let can_write = current_role().is_some_and(|role| org::role_can_write(&role));

    let streams = use_resource(move || {
        let offset = page() * PAGE_SIZE;
        async move {
            let _ = reload();
            let _ = org::ORG.read();
            api::media_streams::list(Some(PAGE_SIZE), Some(offset)).await
        }
    });

    let (list_state, is_empty, count, rows) = match &*streams.value().read() {
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
            title: t!("streams-title").to_string(),
            subtitle: Some(t!("streams-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write,
            add_label: Some(t!("streams-add").to_string()),
            on_add: move |_| dialog.set(Some(Dialog::Create)),
            ListStates {
                state: list_state,
                is_empty,
                empty_message: t!("streams-empty-title").to_string(),
                empty_icon: rsx! {
                    icons::Radio { class: "h-8 w-8 text-gray-400" }
                },
                empty_detail: rsx! {
                    p { class: "text-gray-600 mt-2 max-w-xl mx-auto", {t!("streams-empty-message")} }
                },
                div { class: "space-y-4",
                    div { class: "overflow-x-auto bg-white rounded-lg shadow border border-gray-200",
                        table { class: "min-w-full divide-y divide-gray-200 text-sm",
                            thead { class: "bg-gray-50",
                                tr {
                                    th { class: "th", {t!("streams-col-name")} }
                                    th { class: "th hidden md:table-cell", {t!("streams-col-kind")} }
                                    th { class: "th", {t!("streams-col-capture")} }
                                    th { class: "th hidden md:table-cell",
                                        {t!("streams-col-retention")}
                                    }
                                    th { class: "th {ACTIONS_TH_CLASS}" }
                                }
                            }
                            tbody { class: "divide-y divide-gray-100",
                                for s in rows {
                                    StreamRow {
                                        key: "{s.id}",
                                        stream: s.clone(),
                                        can_write,
                                        on_action: move |d: Dialog| dialog.set(Some(d)),
                                        on_changed: move |_| reload.with_mut(|r| *r += 1),
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
                StreamFormModal {
                    existing: None,
                    on_close: move |_| dialog.set(None),
                    on_saved: move |_| {
                        dialog.set(None);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            },
            Some(Dialog::Edit(s)) => rsx! {
                StreamFormModal {
                    key: "edit-{s.id}",
                    existing: Some(s.clone()),
                    on_close: move |_| dialog.set(None),
                    on_saved: move |_| {
                        dialog.set(None);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            },
            Some(Dialog::Delete(s)) => rsx! {
                ConfirmDialog {
                    title: t!("streams-delete-title"),
                    message: t!("streams-delete-message", name : s.name.clone()),
                    confirm_label: t!("streams-delete-confirm"),
                    on_confirm: {
                        let id = s.id.clone();
                        move |_| {
                            let id = id.clone();
                            dialog.set(None);
                            let done = t!("streams-deleted").to_string();
                            spawn(async move {
                                match api::media_streams::delete(&id).await {
                                    Ok(_) => {
                                        toasts::success(done);
                                        reload.with_mut(|r| *r += 1);
                                    }
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

/// Capture failure code (`media_streams.capture_error`) → sentence;
/// unknown codes stay verbatim.
fn capture_error_label(code: &str) -> String {
    match code {
        "unreachable" => t!("streams-error-unreachable").to_string(),
        "format-unsupported" => t!("streams-error-format-unsupported").to_string(),
        "encrypted" => t!("streams-error-encrypted").to_string(),
        "too-large" => t!("streams-error-too-large").to_string(),
        "stalled" => t!("streams-error-stalled").to_string(),
        "decoder-failed" => t!("streams-error-decoder-failed").to_string(),
        "decoder-missing" => t!("streams-error-decoder-missing").to_string(),
        "secret-unreadable" => t!("streams-error-secret-unreadable").to_string(),
        "store-failed" => t!("streams-error-store-failed").to_string(),
        other => other.to_string(),
    }
}

/// Badge of the capture state (disabled streams read « off »).
fn capture_badge(s: &MediaStream) -> (&'static str, String) {
    if !s.enabled {
        return (
            "bg-gray-100 text-gray-700",
            t!("streams-state-off").to_string(),
        );
    }
    match s.capture_state {
        CaptureState::Running => (
            "bg-emerald-100 text-emerald-800",
            t!("streams-state-running").to_string(),
        ),
        CaptureState::Starting => (
            "bg-blue-100 text-blue-800",
            t!("streams-state-starting").to_string(),
        ),
        CaptureState::Backoff => (
            "bg-amber-100 text-amber-800",
            t!("streams-state-backoff").to_string(),
        ),
        CaptureState::Stopped => (
            "bg-gray-100 text-gray-700",
            t!("streams-state-stopped").to_string(),
        ),
    }
}

#[component]
fn StreamRow(
    stream: MediaStream,
    can_write: bool,
    on_action: Callback<Dialog>,
    on_changed: Callback<()>,
) -> Element {
    let (badge_class, badge_label) = capture_badge(&stream);
    let mut busy = use_signal(|| false);
    let toggle = {
        let id = stream.id.clone();
        let enabled = stream.enabled;
        move |_| {
            let id = id.clone();
            busy.set(true);
            spawn(async move {
                let input = MediaStreamInput {
                    enabled: Some(!enabled),
                    ..Default::default()
                };
                let result = api::media_streams::update(&id, &input).await;
                busy.set(false);
                match result {
                    Ok(_) => on_changed.call(()),
                    Err(err) => toasts::error(err),
                }
            });
        }
    };
    let s_edit = stream.clone();
    let s_delete = stream.clone();
    rsx! {
        tr { class: "group hover:bg-gray-50",
            td { class: "td",
                p { class: "font-medium text-gray-900", "{stream.name}" }
                p { class: "text-xs text-gray-500 font-mono", "{stream.slug}" }
                if stream.tdm_checked_at.is_none() {
                    p { class: "text-xs text-amber-700 mt-0.5", {t!("streams-tdm-unchecked")} }
                }
            }
            td { class: "td hidden text-gray-600 md:table-cell", {stream.kind.wire()} }
            td { class: "td",
                span { class: "inline-flex px-2 py-0.5 rounded text-xs font-medium {badge_class}",
                    {badge_label}
                }
                if let Some(err) = stream.capture_error.as_deref() {
                    p { class: "text-xs text-red-700 mt-0.5 break-words",
                        {capture_error_label(err)}
                    }
                }
            }
            td { class: "td hidden text-gray-600 md:table-cell", "{stream.audio_retention}" }
            td { class: "td {ACTIONS_TD_CLASS}",
                if can_write {
                    div { class: "flex justify-end gap-2",
                        button {
                            class: "px-2 py-1 text-xs border border-gray-300 rounded-lg hover:bg-gray-50 disabled:opacity-40",
                            r#type: "button",
                            disabled: busy(),
                            onclick: toggle,
                            if stream.enabled {
                                {t!("streams-disable")}
                            } else {
                                {t!("streams-enable")}
                            }
                        }
                        button {
                            class: "px-2 py-1 text-xs border border-gray-300 rounded-lg hover:bg-gray-50",
                            r#type: "button",
                            onclick: move |_| on_action.call(Dialog::Edit(s_edit.clone())),
                            {t!("streams-edit")}
                        }
                        button {
                            class: DANGER_BTN,
                            r#type: "button",
                            onclick: move |_| on_action.call(Dialog::Delete(s_delete.clone())),
                            {t!("streams-delete")}
                        }
                    }
                }
            }
        }
    }
}
