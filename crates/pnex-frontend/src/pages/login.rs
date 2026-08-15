//! Page de connexion — portée du `AuthWrapper.tsx` React : branding
//! « Welcome to PNeX », fond sombre (le canvas réseau animé de l'original est
//! approché par un dégradé + halos CSS, choix assumé cross-plateforme).
//!
//! Le login est un **redirect PKCE** vers Keycloak via le proxy backend (pas
//! de formulaire mot de passe dans l'UI) ; création de compte et réinitialisation
//! passent par `kc_action` côté Keycloak.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::icons;
use crate::i18n;

#[component]
pub fn Login() -> Element {
    let signin = move |_| api::auth::start_pkce_login(None);
    let register = move |_| api::auth::start_pkce_login(Some("register"));
    let reset = move |_| api::auth::start_pkce_login(Some("reset"));

    rsx! {
        div { class: "relative min-h-screen overflow-hidden bg-gray-900",
            // Fond : dégradé + halos bleus (approximation du réseau animé).
            div { class: "absolute inset-0",
                style: "background: linear-gradient(135deg, #111827 0%, #1e293b 100%)"
            }
            div { class: "absolute -top-24 -left-24 w-96 h-96 rounded-full bg-blue-500/10 blur-3xl" }
            div { class: "absolute bottom-0 right-0 w-[28rem] h-[28rem] rounded-full bg-blue-400/10 blur-3xl" }

            div { class: "relative z-10 min-h-screen flex items-center justify-center px-4",
                div { class: "max-w-md w-full",
                    div { class: "bg-white/95 backdrop-blur-sm rounded-2xl shadow-2xl p-8 space-y-8",
                        // Branding
                        div { class: "text-center",
                            div { class: "inline-flex items-center justify-center w-16 h-16 bg-gradient-to-br from-blue-500 to-blue-600 rounded-xl mb-4 shadow-lg",
                                icons::Zap { class: "h-8 w-8 text-white" }
                            }
                            h2 { class: "text-3xl font-bold text-gray-900 mb-2", {t!("login-welcome")} }
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
