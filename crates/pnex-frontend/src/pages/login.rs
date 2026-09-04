//! Page de connexion — portée du `AuthWrapper.tsx` React : wordmark logo
//! officiel, fond animé « gerbe de faisceaux » WebGL2 (assets/tron-gerbe.js
//! via le pont `crate::tron`), dégradé sombre en secours (WebGL2 absent).
//!
//! Le login est un **redirect PKCE** vers Rauthy via le proxy backend (pas
//! de formulaire mot de passe dans l'UI) ; création de compte et réinitialisation
//! passent par les pages UI Rauthy (register/account).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::i18n;
use crate::tron;

#[component]
pub fn Login() -> Element {
    let signin = move |_| api::auth::start_pkce_login(None);
    let register = move |_| api::auth::start_pkce_login(Some("register"));
    let reset = move |_| api::auth::start_pkce_login(Some("reset"));

    // Libère canvas + contexte GL à la navigation hors login.
    use_drop(tron::unmount);

    rsx! {
        div { class: "relative min-h-screen overflow-hidden bg-gray-900",
            // Fond : gerbe WebGL montée dans ce div au onmounted (id passé
            // au pont JS) ; le dégradé teal sombre n'est visible que si
            // WebGL2 manque — même teinte que le fond du shader.
            div {
                id: "tron-gerbe-bg",
                class: "absolute inset-0",
                style: "background: linear-gradient(135deg, #040d10 0%, #0b2830 100%)",
                onmounted: move |_| tron::mount("tron-gerbe-bg"),
            }

            div { class: "relative z-10 min-h-screen flex items-center justify-center px-4",
                div { class: "max-w-md w-full",
                    div { class: "bg-white/95 backdrop-blur-sm rounded-2xl shadow-2xl p-8 space-y-8",
                        // Branding : wordmark officiel — le logo contient déjà
                        // le nom, pas de titre texte redondant.
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
                                        bg-gradient-to-r from-blue-600 to-blue-700 hover:from-blue-700 hover:to-blue-800 \
                                        shadow-md hover:shadow-lg transform hover:-translate-y-0.5 transition-all",
                                onclick: signin,
                                {t!("login-signin")}
                            }
                            div { class: "flex items-center justify-between text-sm",
                                button {
                                    class: "text-blue-600 hover:text-blue-700 font-medium",
                                    onclick: register,
                                    {t!("login-register")}
                                }
                                button {
                                    class: "text-gray-500 hover:text-gray-700",
                                    onclick: reset,
                                    {t!("login-reset")}
                                }
                            }
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
                    }
                }
            }
        }
    }
}
