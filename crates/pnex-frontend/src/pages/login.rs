//! Login page — port of the React `AuthWrapper.tsx`: official logo
//! wordmark, animated WebGL2 "beam sheaf" background (assets/tron-gerbe.js
//! via the `crate::tron` bridge), dark gradient as fallback (no WebGL2).
//!
//! Login is a **PKCE redirect** to Rauthy via the backend proxy (no
//! password form in the UI); account creation and reset go through
//! the Rauthy UI pages (register/account).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::i18n;
use crate::tron;

#[component]
pub fn Login() -> Element {
    // The three actions go through the same PKCE flow on every target:
    // web = browser redirect, native = system browser + backend bridge
    // (auth/native.rs).
    let signin = move |_| api::auth::start_pkce_login(None);
    let register = move |_| api::auth::start_pkce_login(Some("register"));
    let reset = move |_| api::auth::start_pkce_login(Some("reset"));

    // Frees canvas + GL context when navigating away from login.
    use_drop(tron::unmount);

    rsx! {
        div { class: "relative min-h-screen overflow-hidden bg-gray-900",
            // Background: WebGL sheaf mounted in this div on onmounted (id passed
            // to the JS bridge); the dark teal gradient is only visible if
            // WebGL2 is missing — same tint as the shader background.
            div {
                id: "tron-gerbe-bg",
                class: "absolute inset-0",
                style: "background: linear-gradient(135deg, #040d10 0%, #0b2830 100%)",
                onmounted: move |_| tron::mount("tron-gerbe-bg"),
            }

            div { class: "relative z-10 min-h-screen flex items-center justify-center px-4",
                div { class: "max-w-md w-full",
                    div { class: "bg-white/95 backdrop-blur-sm rounded-2xl shadow-2xl p-8 space-y-8",
                        // Branding: official wordmark — the logo already holds
                        // the name, no redundant text title.
                        div { class: "text-center",
                            img {
                                src: asset!("/assets/logo.png"),
                                alt: "PNeX",
                                class: "mx-auto h-16 w-auto mb-5",
                            }
                            p { class: "text-sm text-gray-600 mb-1", {t!("login-tagline")} }
                            p { class: "text-xs text-gray-500", {t!("login-description")} }
                        }

                        // Actions
                        div { class: "space-y-4",
                            button {
                                class: "w-full flex justify-center items-center py-3 px-4 rounded-lg text-sm font-semibold text-white \
                                        bg-gradient-to-r from-signal-600 to-signal-700 hover:from-signal-700 hover:to-signal-800 \
                                        shadow-md hover:shadow-lg transform hover:-translate-y-0.5 transition-[translate,box-shadow] duration-150 ease-out-ui",
                                onclick: signin,
                                {t!("login-signin")}
                            }
                            div { class: "flex items-center justify-between text-sm",
                                button {
                                    class: "text-signal-600 hover:text-signal-700 font-medium",
                                    onclick: register,
                                    {t!("login-register")}
                                }
                                button {
                                    class: "text-gray-500 hover:text-gray-700",
                                    onclick: reset,
                                    {t!("login-reset")}
                                }
                            }
                            // Self-hosted server selection built into the
                            // page (Bitwarden-style) — web: renders nothing.
                            crate::pages::server_url::SelfHostedSection {}
                        }

                        div { class: "pt-6 border-t border-gray-200 flex items-center justify-between",
                            p { class: "text-xs text-gray-500", {t!("login-footer")} }
                            select {
                                class: "text-xs border border-gray-300 rounded-lg px-2 py-1 bg-white text-gray-700",
                                value: "{i18n::current_tag()}",
                                onchange: move |event| i18n::set_locale(&event.value()),
                                option { value: "fr-FR", "Français" }
                                option { value: "en-US", "English" }
                            }
                        }
                        // App version — visible before any login:
                        // the clean refusal (compatibility gate) relies
                        // on a clear identification of both sides.
                        p { class: "pt-3 text-center text-xs text-gray-500",
                            {t!("login-app-version", version : crate ::version::APP_VERSION)}
                        }
                    }
                }
            }
        }
    }
}
