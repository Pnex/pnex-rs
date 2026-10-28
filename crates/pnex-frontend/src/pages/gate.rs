//! Blocking screen of the compatibility gate — rendered when the server does
//! not speak the same API contract, is unreachable, or is not PNEX
//! (`GATE` state ≠ `Ok`). Clean refusal rather than a boot full of opaque API
//! errors: versions and contracts of both sides shown, retry possible,
//! back to the server configuration on native (web: same-origin, a single
//! server possible).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::meta::MetaError;
use crate::components::gerbe::Gerbe;
use crate::state::compat::{GateState, GATE};

#[component]
pub fn GateScreen() -> Element {
    match GATE.cloned() {
        GateState::Checking => rsx! {
            div { class: "min-h-screen flex flex-col items-center justify-center bg-gray-50 space-y-4",
                Gerbe { size: 44 }
                p { class: "text-sm text-gray-500", {t!("gate-checking")} }
                // Server being checked — makes the wait readable.
                if !crate::api::config::api_base().is_empty() {
                    p { class: "text-xs text-gray-500", "{crate::api::config::api_base()}" }
                }
                // Escape hatch: never leave the user trapped on the spinner.
                if !cfg!(target_arch = "wasm32") && crate::api::config::locked_server().is_none() {
                    button {
                        class: "py-2 px-4 rounded-lg text-sm font-medium text-gray-700 \
                                bg-gray-100 hover:bg-gray-200 transition-colors",
                        onclick: move |_| {
                            crate::pages::server_url::SERVER_READY.with_mut(|v| *v = false);
                            crate::state::compat::reset();
                        },
                        {t!("gate-change-server")}
                    }
                }
            }
        },
        GateState::Ok => rsx! {},
        GateState::Incompatible {
            server,
            app_contract,
        } => rsx! {
            gate_layout {
                accent_class: "text-red-600",
                title: t!("gate-incompatible-title"),
                message: t!(
                    "gate-incompatible-message", app_version : crate ::version::APP_VERSION,
                    app_contract : app_contract.to_string(), server_version : server.version.clone(),
                    server_contract : server.contract.to_string()
                ),
                hint: None,
            }
        },
        GateState::Unreachable { error } => rsx! {
            gate_layout {
                accent_class: "text-gray-900",
                title: t!("gate-unreachable-title"),
                message: unreachable_message(&error),
                hint: Some(t!("gate-unreachable-hint")),
            }
        },
    }
}

/// Message of the unreachable state: technical detail relayed as is (repo
/// rule — error messages are never translated), the "this is not a PNEX
/// server" identity being the only specifically translated case.
fn unreachable_message(error: &MetaError) -> String {
    let msg = match error {
        MetaError::Unreachable(detail) => detail.clone(),
        MetaError::NotPnex => t!("gate-error-not-pnex"),
    };
    t!(
        "gate-unreachable-message",
        base: crate::api::config::api_base(),
        msg: msg
    )
}

/// Shared template of both blocking screens — same visual pattern as
/// `ServerUrl` (gradient background + white card).
#[component]
fn gate_layout(
    accent_class: String,
    title: String,
    message: String,
    hint: Option<String>,
) -> Element {
    rsx! {
        div { class: "relative min-h-screen overflow-hidden bg-gray-900",
            div {
                class: "absolute inset-0",
                style: "background: linear-gradient(135deg, #040d10 0%, #0b2830 100%)",
            }
            div { class: "relative z-10 min-h-screen flex items-center justify-center px-4",
                div { class: "max-w-md w-full",
                    div { class: "bg-white/95 backdrop-blur-sm rounded-2xl shadow-2xl p-8 space-y-6 text-center",
                        img {
                            src: asset!("/assets/logo.png"),
                            alt: "PNeX",
                            class: "mx-auto h-14 w-auto",
                        }
                        h1 { class: "text-lg font-semibold {accent_class}", {title} }
                        p { class: "text-sm text-gray-700", {message} }
                        if let Some(hint) = hint {
                            p { class: "text-xs text-gray-500", {hint} }
                        }
                        div { class: "space-y-2 pt-2",
                            button {
                                class: "w-full py-2.5 px-4 rounded-lg text-sm font-semibold text-white \
                                        bg-signal-600 hover:bg-signal-700 transition-colors disabled:opacity-50",
                                onclick: move |_| {
                                    // Detached on purpose: `check()` flips GATE to Checking,
                                    // which unmounts this layout — a scoped `spawn` would be
                                    // cancelled mid-probe and leave the gate stuck on Checking.
                                    dioxus::dioxus_core::spawn_forever(async {
                                        crate::state::compat::check().await;
                                    });
                                },
                                {t!("gate-retry")}
                            }
                            // Web: same-origin, a single server possible —
                            // changing server makes no sense.
                            if !cfg!(target_arch = "wasm32") && crate::api::config::locked_server().is_none() {
                                button {
                                    class: "w-full py-2.5 px-4 rounded-lg text-sm font-medium text-gray-700 \
                                            bg-gray-100 hover:bg-gray-200 transition-colors",
                                    onclick: move |_| {
                                        crate::pages::server_url::SERVER_READY
                                            .with_mut(|v| *v = false);
                                        crate::state::compat::reset();
                                    },
                                    {t!("gate-change-server")}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
