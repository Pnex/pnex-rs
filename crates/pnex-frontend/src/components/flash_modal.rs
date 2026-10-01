//! Modale de flash navigateur — Web Serial + esptool-js (cf. flash.rs et
//! js/flasher.js). Les octets firmware sont téléchargés à l'ouverture, PAS au
//! clic : `requestPort()` exige un geste utilisateur sans attente réseau
//! intermédiaire.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::flash::{self, FlashError, FlashEvent};
use pnex_core::err_codes;

use super::{icons, modal::Modal};

/// Étape du flow après le clic (« None » = prêt, en attente du clic).
#[derive(Clone, PartialEq)]
enum FlashState {
    /// Stage brute du glue JS (connect/write/reset) + progression 0-100.
    Flashing {
        stage: String,
        percent: u8,
    },
    Done,
    /// Machine code (`err_codes::FLASH_*`) + verbatim JS detail, resolved at
    /// display time via `error_i18n`.
    Failed {
        error: FlashError,
    },
}

/// Libellé i18n d'une étape du glue JS.
fn stage_label(stage: &str) -> String {
    match stage {
        "connect" => t!("flash-stage-connect"),
        "reset" => t!("flash-stage-reset"),
        _ => t!("flash-stage-write"),
    }
}

/// Normalizes a flash error into the next state. Cancelling the native port
/// picker surfaces as "No port selected" (flasher.js requestPort rejection):
/// it is not a failure — back to idle instead of the replug screen. The
/// match is case-insensitive and partial so Chrome's exact wording can drift.
fn error_state(error: FlashError) -> Option<FlashState> {
    let cancelled = error
        .detail
        .as_deref()
        .is_some_and(|detail| detail.to_ascii_lowercase().contains("no port selected"));
    if cancelled {
        None
    } else {
        Some(FlashState::Failed { error })
    }
}

#[component]
pub fn FlashModal(device_id: String, on_close: Callback<()>) -> Element {
    // Octets du firmware (image mergée @0x0) — un téléchargement à l'ouverture.
    let fetch_id = device_id.clone();
    let firmware = use_resource(move || {
        let id = fetch_id.clone();
        async move { api::builds::download(&id).await }
    });
    let mut state = use_signal(|| None::<FlashState>);
    // Chip détecté au sync (affiché tel quel, ex. « ESP32-D0WD-V3 »).
    let mut chip = use_signal(String::new);

    // Le clic déclenche tout le flow d'un trait : requestPort() (sélecteur
    // natif) → sync → écriture → redémarrage.
    let start = move |_| {
        let bytes = match &*firmware.read() {
            Some(Ok(bytes)) => bytes.clone(),
            _ => return,
        };
        state.set(Some(FlashState::Flashing {
            stage: "connect".to_string(),
            percent: 0,
        }));
        chip.set(String::new());
        spawn(async move {
            // Image unique @0x0 — le device est compilé par device (décision
            // du 2026-09-02 : plus de secteur PNEXCFG à 0x200000).
            let entries: Vec<(u32, Vec<u8>)> = vec![(0x0, bytes)];
            let outcome = flash::flash(entries, |event| match event {
                FlashEvent::Stage { stage } => state.with_mut(|current| {
                    if let Some(FlashState::Flashing { stage: current, .. }) = current {
                        *current = stage;
                    }
                }),
                FlashEvent::Chip { chip: name } => chip.set(name),
                FlashEvent::Progress { percent } => state.with_mut(|current| {
                    if let Some(FlashState::Flashing {
                        percent: current, ..
                    }) = current
                    {
                        *current = percent;
                    }
                }),
                FlashEvent::Done => state.set(Some(FlashState::Done)),
                FlashEvent::Error { message } => state.set(error_state(FlashError {
                    code: err_codes::FLASH_UNAVAILABLE,
                    detail: Some(message),
                })),
            })
            .await;
            // Rejet de la promise JS : le glue émet déjà un événement error,
            // ce Err réécrit le même état normalisé (ex. flasher.js absent
            // du build).
            if let Err(error) = outcome {
                state.set(error_state(error));
            }
        });
    };

    rsx! {
        Modal {
            title: t!("flash-title"),
            max_width: "max-w-md".to_string(),
            on_close,
            div { class: "space-y-4",
                if !flash::supported() {
                    div { class: "bg-amber-50 border border-amber-200 rounded-lg p-3 text-sm text-amber-700",
                        {t!("flash-unsupported")}
                    }
                } else {
                    match &*firmware.read() {
                        None => rsx! {
                            div { class: "flex items-center gap-3 text-sm text-gray-600",
                                span { class: "animate-spin inline-block rounded-full h-5 w-5 border-b-2 border-blue-600" }
                                {t!("flash-fetching")}
                            }
                        },
                        Some(Err(err)) => rsx! {
                            div { class: "bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                                p { {t!("flash-fetch-error")} }
                                p { class: "mt-1 font-medium", {err.message.clone()} }
                            }
                        },
                        Some(Ok(_)) => match state() {
                            None => rsx! {
                                p { class: "text-sm text-gray-600", {t!("flash-instructions")} }
                                button {
                                    class: "w-full px-4 py-2 bg-green-600 text-white rounded-lg hover:bg-green-700 transition-colors text-sm font-semibold",
                                    r#type: "button",
                                    onclick: start,
                                    icons::Zap { class: "h-4 w-4 inline mr-1" }
                                    {t!("flash-start")}
                                }
                            },
                            Some(FlashState::Flashing { stage, percent }) => rsx! {
                                div { class: "space-y-2",
                                    div { class: "flex items-center justify-between text-sm",
                                        span { class: "text-gray-600", {stage_label(&stage)} }
                                        if !chip().is_empty() {
                                            span { class: "text-xs text-gray-400", {chip()} }
                                        }
                                    }
                                    div { class: "w-full bg-gray-200 rounded-full h-2.5",
                                        div {
                                            class: "bg-blue-600 h-2.5 rounded-full transition-all duration-300",
                                            style: "width: {percent}%",
                                        }
                                    }
                                    p { class: "text-xs text-gray-400 text-right", "{percent} %" }
                                }
                            },
                            Some(FlashState::Done) => rsx! {
                                div { class: "text-center space-y-4",
                                    icons::CheckCircle { class: "h-10 w-10 text-green-500 mx-auto" }
                                    p { class: "text-sm text-gray-700", {t!("flash-done")} }
                                    button {
                                        class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-medium",
                                        r#type: "button",
                                        onclick: move |_| on_close.call(()),
                                        {t!("common-close")}
                                    }
                                }
                            },
                            Some(FlashState::Failed { error }) => rsx! {
                                div { class: "space-y-3 text-center",
                                    p { class: "text-sm font-medium text-red-700", {t!("flash-error")} }
                                    // After a failed flash the board's serial port
                                    // usually stays busy: only a physical
                                    // unplug/replug lets esptool reopen it.
                                    ReplugAnimation {}
                                    p { class: "text-sm text-gray-700", {t!("flash-replug-prompt")} }
                                    p { class: "text-xs text-gray-500", {t!("flash-replug-hint")} }
                                    p { class: "text-xs text-gray-400 break-words",
                                        {crate::api::error_i18n::resolve(
                                            &err_codes::fluent_key(error.code),
                                            None,
                                        )}
                                    }
                                    if let Some(detail) = &error.detail {
                                        p { class: "text-xs text-gray-400 break-words", {detail.clone()} }
                                    }
                                    div { class: "flex gap-2",
                                        button {
                                            class: "flex-1 px-4 py-2 bg-green-600 text-white rounded-lg hover:bg-green-700 transition-colors text-sm font-semibold",
                                            r#type: "button",
                                            onclick: move |_| state.set(None),
                                            {t!("flash-retry")}
                                        }
                                        button {
                                            class: "flex-1 px-4 py-2 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors",
                                            r#type: "button",
                                            onclick: move |_| on_close.call(()),
                                            {t!("common-close")}
                                        }
                                    }
                                }
                            },
                        },
                    }
                }
            }
        }
    }
}

/// Unplug/replug loop for the flash-failure screen: board (static, with a
/// status LED) + USB cable that slides out then back in, direction arrows,
/// and two step chips ("1. Unplug" / "2. Plug back in") lit in sync — all
/// driven by the `pnex-replug-*` keyframes in style/tailwind.css.
/// Bespoke inline SVG: the project's lucide-style icons cannot animate a
/// sub-group.
#[component]
fn ReplugAnimation() -> Element {
    rsx! {
        div { class: "space-y-2",
            svg {
                xmlns: "http://www.w3.org/2000/svg",
                view_box: "0 0 96 36",
                class: "h-14 w-36 mx-auto text-gray-500",
                fill: "none",
                stroke: "currentColor",
                "stroke-width": "2",
                "stroke-linecap": "round",
                "stroke-linejoin": "round",
                // Board (static, left): outline, chip, status LED.
                rect { x: "4", y: "8", width: "20", height: "24", rx: "2" }
                rect { x: "9", y: "15", width: "8", height: "8", rx: "1", "stroke-width": "1.5" }
                circle { class: "pnex-replug-led", cx: "9", cy: "28", r: "1.6", "stroke-width": "1" }
                // USB port mouth.
                path { d: "M24 15h3v10h-3" }
                // Direction arrows above the cable (out → / in ←).
                g { class: "pnex-replug-arrow-out", "stroke-width": "1.5",
                    path { d: "M40 4h14M50 1l4 3-4 3" }
                }
                g { class: "pnex-replug-arrow-in", "stroke-width": "1.5",
                    path { d: "M54 4H40M44 1l-4 3 4 3" }
                }
                // Cable + plug (animated group).
                g { class: "pnex-replug-cable",
                    path { d: "M28 17.5h3M28 22.5h3" }
                    rect { x: "31", y: "14", width: "9", height: "12", rx: "1.5" }
                    path { d: "M40 20c8 0 8 9 16 9h40" }
                }
            }
            div { class: "flex justify-center items-center gap-3 text-xs text-gray-700",
                span { class: "pnex-replug-step-1", {t!("flash-replug-step-unplug")} }
                span { class: "text-gray-400", "→" }
                span { class: "pnex-replug-step-2", {t!("flash-replug-step-replug")} }
            }
        }
    }
}
