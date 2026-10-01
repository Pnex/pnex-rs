//! Trust-on-first-use dialog for a self-hosted server's root CA (D70).
//!
//! Shown as an overlay by `App` whenever `apply_server` parks a CA in
//! `PENDING_CA` (HTTPS server signed by its own local CA). The user compares
//! the SHA-256 fingerprint with the one displayed by the server (web UI,
//! Profile → About) before pinning it. Never rendered on the web: the
//! browser owns trust there and `PENDING_CA` stays empty.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::pages::server_url::PENDING_CA;

#[component]
pub(crate) fn TrustCaDialog() -> Element {
    let Some(pending) = PENDING_CA.cloned() else {
        return rsx! {};
    };
    let accept_target = pending.clone();
    let download_url = format!("{}{}", pending.base, pnex_api_contract::META_CA_PATH);

    rsx! {
        div { class: "fixed inset-0 z-50 flex items-center justify-center bg-black/60 px-4",
            div { class: "max-w-md w-full bg-white rounded-2xl shadow-2xl p-6 space-y-4",
                h2 { class: "text-lg font-semibold text-gray-900", {t!("trust-ca-title")} }
                p { class: "text-sm text-gray-700",
                    {t!("trust-ca-message", base: pending.base.clone())}
                }
                div { class: "space-y-1",
                    p { class: "text-xs font-medium text-gray-500", {t!("trust-ca-fingerprint")} }
                    p { class: "text-xs font-mono text-gray-900 break-all bg-gray-50 rounded-lg p-2",
                        "{pending.fingerprint}"
                    }
                }
                p { class: "text-xs text-gray-500", {t!("trust-ca-device-hint")} }
                button {
                    class: "w-full py-2 px-4 rounded-lg text-xs font-medium text-blue-700 \
                            bg-blue-50 hover:bg-blue-100 transition-colors",
                    onclick: move |_| open_in_browser(&download_url),
                    {t!("trust-ca-download")}
                }
                div { class: "flex gap-2 pt-2",
                    button {
                        class: "flex-1 py-2.5 px-4 rounded-lg text-sm font-medium text-gray-700 \
                                bg-gray-100 hover:bg-gray-200 transition-colors",
                        onclick: move |_| PENDING_CA.with_mut(|slot| *slot = None),
                        {t!("trust-ca-cancel")}
                    }
                    button {
                        class: "flex-1 py-2.5 px-4 rounded-lg text-sm font-semibold text-white \
                                bg-blue-600 hover:bg-blue-700 transition-colors",
                        onclick: move |_| accept(accept_target.clone()),
                        {t!("trust-ca-accept")}
                    }
                }
            }
        }
    }
}

/// Pins the CA then replays the connection. Detached (`spawn_forever`): the
/// dialog and the screen underneath unmount as soon as the server is ready.
fn accept(pending: crate::pages::server_url::PendingCa) {
    PENDING_CA.with_mut(|slot| *slot = None);
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::api::tls::pin_ca(&pending.pem);
        dioxus::dioxus_core::spawn_forever(async move {
            let _ = crate::pages::server_url::apply_server(&pending.base).await;
        });
    }
    #[cfg(target_arch = "wasm32")]
    let _ = pending;
}

/// Opens the CA download in the system browser, which hands `.crt` files
/// to the OS certificate installer.
fn open_in_browser(url: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Err(err) = webbrowser::open(url) {
        eprintln!("pnex-trust-ca: cannot open browser: {err}");
    }
    #[cfg(target_arch = "wasm32")]
    let _ = url;
}
