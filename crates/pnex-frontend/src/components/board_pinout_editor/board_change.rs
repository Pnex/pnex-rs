//! Board change of a registered device (O39): the variants of the same chip,
//! the soldered screen follows the board, then rebuild and update.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::gerbe::Gerbe;
use crate::components::modal::{Modal, MODAL_FOOTER};
use crate::components::ui::{BTN_PRIMARY, BTN_SECONDARY};
use crate::state::toasts;

#[component]
pub(super) fn BoardChangeModal(
    device_pk: i64,
    current_id: i64,
    soc: String,
    on_close: Callback<()>,
    on_changed: Callback<()>,
) -> Element {
    let picked = use_signal(|| current_id);
    let mut busy = use_signal(|| false);
    let soc_filter = soc.clone();
    let boards = use_resource(move || {
        let soc = soc_filter.clone();
        async move { api::boards::list(Some(&soc)).await }
    });
    let save = move |_| {
        let board_id = picked();
        if board_id == current_id {
            on_close.call(());
            return;
        }
        busy.set(true);
        spawn(async move {
            match api::devices::set_board(device_pk, board_id).await {
                Ok(_) => {
                    toasts::info(t!("board-change-done").to_string());
                    on_changed.call(());
                    on_close.call(());
                }
                Err(err) => toasts::error(err),
            }
            busy.set(false);
        });
    };
    let list = match &*boards.value().read() {
        Some(Ok(list)) => Some(list.clone()),
        _ => None,
    };
    rsx! {
        Modal {
            title: t!("board-change-title"),
            max_width: "max-w-lg",
            on_close,
            div { class: "space-y-4",
                p { class: "text-sm text-gray-600", {t!("board-change-help")} }
                match list {
                    None => rsx! {
                        div { class: "text-center py-4",
                            Gerbe { size: 28 }
                        }
                    },
                    Some(list) => rsx! {
                        fieldset { class: "space-y-2",
                            legend { class: "sr-only", {t!("board-change-title")} }
                            for board in list {
                                BoardOption {
                                    key: "{board.id}",
                                    board,
                                    picked,
                                    current_id,
                                }
                            }
                        }
                    },
                }
                div { class: MODAL_FOOTER,
                    button {
                        class: BTN_SECONDARY,
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: BTN_PRIMARY,
                        r#type: "button",
                        disabled: busy() || picked() == current_id,
                        onclick: save,
                        {t!("board-change-confirm")}
                    }
                }
            }
        }
    }
}

#[component]
fn BoardOption(board: api::boards::Board, mut picked: Signal<i64>, current_id: i64) -> Element {
    let id = board.id;
    let input_id = format!("board-option-{id}");
    let builtin_screen = board
        .screens()
        .iter()
        .any(|s| s.get("builtin").and_then(|b| b.as_bool()) == Some(true));
    let name = board
        .pretty_name
        .clone()
        .unwrap_or_else(|| board.name.clone());
    rsx! {
        label {
            r#for: "{input_id}",
            class: if picked() == id { "flex items-center gap-3 rounded-lg border border-signal-500 ring-1 ring-signal-500 px-3 py-2 cursor-pointer" } else { "flex items-center gap-3 rounded-lg border border-gray-300 px-3 py-2 cursor-pointer hover:border-signal-400" },
            input {
                id: "{input_id}",
                r#type: "radio",
                name: "board-change",
                checked: picked() == id,
                onchange: move |_| picked.set(id),
            }
            span { class: "flex-1 text-sm font-medium text-gray-900", {name} }
            if builtin_screen {
                span { class: "text-xs text-teal-700 bg-teal-50 rounded-full px-1.5 py-0.5",
                    {t!("board-peripheral-screen")}
                }
            }
            if id == current_id {
                span { class: "text-xs text-gray-500", {t!("board-change-current")} }
            }
        }
    }
}
