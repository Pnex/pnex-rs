//! Modale de flash navigateur — Web Serial + esptool-js (cf. flash.rs et
//! js/flasher.js). Les octets firmware sont téléchargés à l'ouverture, PAS au
//! clic : `requestPort()` exige un geste utilisateur sans attente réseau
//! intermédiaire.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::flash::{self, FlashEvent};

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
    Failed(String),
}

/// Libellé i18n d'une étape du glue JS.
fn stage_label(stage: &str) -> String {
    match stage {
        "connect" => t!("flash-stage-connect"),
        "reset" => t!("flash-stage-reset"),
        _ => t!("flash-stage-write"),
    }
}

#[component]
pub fn FlashModal(
    device_pk: i64,
    device_id: String,
    needs_config: bool,
    on_close: Callback<()>,
) -> Element {
    // Octets du firmware (image mergée @0x0) — un téléchargement à l'ouverture.
    let fetch_id = device_id.clone();
    let firmware = use_resource(move || {
        let id = fetch_id.clone();
        async move { api::builds::download(&id).await }
    });
    let mut state = use_signal(|| None::<FlashState>);
    // Chip détecté au sync (affiché tel quel, ex. « ESP32-D0WD-V3 »).
    let mut chip = use_signal(String::new);
    // Brick 0 — formulaire secteur PNEXCFG (devices génériques seulement) :
    // host prérempli = hôte de la page (le device rejoint le même serveur) ;
    // ws_ssl par défaut = schéma de la page (https → wss).
    let mut cfg_ssid = use_signal(String::new);
    let mut cfg_password = use_signal(String::new);
    let mut cfg_host = use_signal(crate::api::config::device_connect_host);
    let cfg_ws_ssl = use_signal(move || {
        crate::api::config::api_base().starts_with("https")
    });
    // Octets du secteur préparés (POST config-sector) — le bouton Flasher
    // n'est activé qu'avec le secteur en mémoire.
    let mut cfg_bytes = use_signal(|| None::<Vec<u8>>);
    let mut cfg_error = use_signal(|| None::<String>);

    // Le clic déclenche tout le flow d'un trait : requestPort() (sélecteur
    // natif) → sync → écriture → redémarrage.
    let start = move |_| {
        let bytes = match &*firmware.read() {
            Some(Ok(bytes)) => bytes.clone(),
            _ => return,
        };
        // Secteur PNEXCFG obligatoire pour un device générique.
        let sector = if needs_config {
            match cfg_bytes() {
                Some(sector) => Some(sector),
                None => return,
            }
        } else {
            None
        };
        state.set(Some(FlashState::Flashing {
            stage: "connect".to_string(),
            percent: 0,
        }));
        chip.set(String::new());
        spawn(async move {
            let mut entries: Vec<(u32, Vec<u8>)> = vec![(0x0, bytes)];
            if let Some(sector) = sector {
                entries.push((0x200_000, sector));
            }
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
                FlashEvent::Error { message } => state.set(Some(FlashState::Failed(message))),
            })
            .await;
            // Rejet de la promise JS : le glue émet déjà un événement error,
            // ce Err réécrit le même état (ex. flasher.js absent du build).
            if let Err(message) = outcome {
                state.set(Some(FlashState::Failed(message)));
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
                                // Brick 0 — config secteur requise avant le flash.
                                if needs_config && cfg_bytes().is_none() {
                                    div { class: "space-y-3 rounded-lg border border-gray-200 p-3",
                                        div { class: "space-y-1",
                                            label { class: "text-xs font-medium text-gray-500", {t!("flashcfg-ssid")} }
                                            input {
                                                class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                                value: "{cfg_ssid}",
                                                oninput: move |e| cfg_ssid.set(e.value()),
                                            }
                                        }
                                        div { class: "space-y-1",
                                            label { class: "text-xs font-medium text-gray-500", {t!("flashcfg-password")} }
                                            input {
                                                r#type: "password",
                                                class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                                value: "{cfg_password}",
                                                oninput: move |e| cfg_password.set(e.value()),
                                            }
                                        }
                                        div { class: "space-y-1",
                                            label { class: "text-xs font-medium text-gray-500", {t!("flashcfg-host")} }
                                            input {
                                                class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                                                value: "{cfg_host}",
                                                oninput: move |e| cfg_host.set(e.value()),
                                            }
                                            if let Some(err) = cfg_error() {
                                                p { class: "text-xs text-red-600", {err} }
                                            }
                                        }
                                        button {
                                            class: "w-full px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-semibold",
                                            r#type: "button",
                                            onclick: move |_| {
                                                let req = api::pins::ConfigSectorRequest {
                                                    wifi_ssid: cfg_ssid.cloned(),
                                                    wifi_password: cfg_password.cloned(),
                                                    host: cfg_host.cloned(),
                                                    ws_ssl: cfg_ws_ssl(),
                                                };
                                                let pk = device_pk;
                                                spawn(async move {
                                                    match api::pins::config_sector(pk, &req).await {
                                                        Ok(bytes) => {
                                                            if bytes.len() == 4096 {
                                                                cfg_bytes.set(Some(bytes));
                                                                cfg_error.set(None);
                                                            } else {
                                                                cfg_error.set(Some(
                                                                    format!("secteur inattendu : {} octets", bytes.len()),
                                                                ));
                                                            }
                                                        }
                                                        Err(err) => cfg_error.set(Some(err.message)),
                                                    }
                                                });
                                            },
                                            icons::Zap { class: "h-4 w-4 inline mr-1" }
                                            {t!("flashcfg-prepare")}
                                        }
                                    }
                                }
                                button {
                                    class: if needs_config && cfg_bytes().is_none() {
                                        "w-full px-4 py-2 bg-gray-300 text-gray-500 rounded-lg text-sm font-semibold cursor-not-allowed"
                                    } else {
                                        "w-full px-4 py-2 bg-green-600 text-white rounded-lg hover:bg-green-700 transition-colors text-sm font-semibold"
                                    },
                                    disabled: needs_config && cfg_bytes().is_none(),
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
                            Some(FlashState::Failed(message)) => rsx! {
                                div { class: "space-y-3",
                                    div { class: "bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                                        p { {t!("flash-error")} }
                                        p { class: "mt-1 font-medium break-words", {message} }
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
