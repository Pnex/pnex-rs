//! Profil — porté du `Profile.tsx` React : identité (gérée par l'IdP Rauthy,
//! lecture seule côté app), préférences (`PATCH /api/v1/profile` : langue,
//! timezone, format de date, thème), changement de mot de passe (redirect
//! vers la page compte Rauthy `/auth/v1/account`), déconnexion.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::icons;
use crate::i18n;
use crate::state::{session, toasts};

#[component]
pub fn Profile() -> Element {
    let user = session::user();
    let Some(user) = user else {
        return rsx! {};
    };
    let profile = user.profile.clone().unwrap_or_default();

    // Valeurs initiales du formulaire = profil courant (init une seule fois).
    let mut language = use_signal(|| profile.language.clone());
    let mut timezone = use_signal(|| profile.timezone.clone());
    let mut date_format = use_signal(|| profile.date_format.clone().unwrap_or_default());
    let mut theme = use_signal(|| normalize_theme(&profile.theme));

    rsx! {
        div { class: "p-6",
            div { class: "mb-8",
                h1 { class: "text-3xl font-bold text-gray-900", {t!("nav-profile")} }
                p { class: "text-gray-600 mt-2", {t!("profile-subtitle")} }
            }

            div { class: "grid grid-cols-1 lg:grid-cols-3 gap-8",
                div { class: "lg:col-span-2 space-y-6",
                    // Identité — gérée par l'IdP Rauthy, lecture seule.
                    div { class: "bg-white rounded-lg shadow-sm",
                        div { class: "p-6 border-b border-gray-200",
                            h2 { class: "text-lg font-semibold text-gray-900", {t!("profile-identity")} }
                        }
                        div { class: "p-6",
                            div { class: "flex items-center space-x-6 mb-8",
                                div { class: "w-20 h-20 bg-blue-100 rounded-full flex items-center justify-center",
                                    icons::User { class: "h-8 w-8 text-blue-600" }
                                }
                                div {
                                    h3 { class: "text-xl font-semibold text-gray-900",
                                        {user.full_name.clone().unwrap_or_else(|| user.username.clone())}
                                    }
                                    p { class: "text-gray-600", {user.email.clone().unwrap_or_else(|| "—".into())} }
                                }
                            }
                            div { class: "grid grid-cols-1 md:grid-cols-2 gap-6",
                                label { class: "block",
                                    span { class: "block text-sm font-medium text-gray-700 mb-2", {t!("profile-username")} }
                                    input {
                                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg bg-gray-50 text-gray-900",
                                        r#type: "text",
                                        disabled: true,
                                        value: "{user.username}",
                                    }
                                }
                                label { class: "block",
                                    span { class: "block text-sm font-medium text-gray-700 mb-2", {t!("profile-email")} }
                                    input {
                                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg bg-gray-50 text-gray-900",
                                        r#type: "email",
                                        disabled: true,
                                        value: "{user.email.clone().unwrap_or_default()}",
                                    }
                                }
                                p { class: "md:col-span-2 text-xs text-gray-500", {t!("profile-idp-managed")} }
                            }
                        }
                    }

                    // Préférences — PATCH /api/v1/profile.
                    form {
                        class: "bg-white rounded-lg shadow-sm",
                        onsubmit: move |event| {
                            event.prevent_default();
                            let patch = pnex_core::ProfilePatch {
                                language: Some(language.cloned()),
                                timezone: Some(timezone.cloned()),
                                date_format: if date_format.cloned().trim().is_empty() {
                                    None
                                } else {
                                    Some(date_format.cloned().trim().to_string())
                                },
                                theme: Some(theme.cloned()),
                            };
                            spawn(async move {
                                match api::user::patch_profile(&patch).await {
                                    Ok(_) => {
                                        toasts::success("toast-saved");
                                        // resynchronise la session (profil à jour)
                                        if let Ok(fresh) = api::user::get_user_info().await {
                                            session::login(fresh);
                                        }
                                    }
                                    Err(err) => toasts::error(err.message),
                                }
                            });
                        },
                        div { class: "p-6 border-b border-gray-200",
                            h2 { class: "text-lg font-semibold text-gray-900", {t!("profile-preferences")} }
                        }
                        div { class: "p-6 grid grid-cols-1 md:grid-cols-2 gap-6",
                            label { class: "block",
                                span { class: "block text-sm font-medium text-gray-700 mb-2", {t!("profile-language")} }
                                select {
                                    class: "w-full px-3 py-2 border border-gray-300 rounded-lg bg-white text-gray-900",
                                    onchange: move |event| {
                                        let tag = event.value();
                                        language.set(tag.clone());
                                        // Applique immédiatement (persisté localement ;
                                        // le PATCH enregistre côté serveur).
                                        i18n::set_locale(&tag);
                                    },
                                    option { value: "en", selected: language() == "en", "English" }
                                    option { value: "fr", selected: language() == "fr", "Français" }
                                }
                            }
                            label { class: "block",
                                span { class: "block text-sm font-medium text-gray-700 mb-2", {t!("profile-timezone")} }
                                input {
                                    class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-gray-900",
                                    r#type: "text",
                                    value: "{timezone}",
                                    oninput: move |event| timezone.set(event.value()),
                                }
                            }
                            label { class: "block",
                                span { class: "block text-sm font-medium text-gray-700 mb-2", {t!("profile-date-format")} }
                                input {
                                    class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-gray-900",
                                    r#type: "text",
                                    placeholder: "YYYY-MM-DD HH:mm",
                                    value: "{date_format}",
                                    oninput: move |event| date_format.set(event.value()),
                                }
                            }
                            label { class: "block",
                                span { class: "block text-sm font-medium text-gray-700 mb-2", {t!("profile-theme")} }
                                select {
                                    class: "w-full px-3 py-2 border border-gray-300 rounded-lg bg-white text-gray-900",
                                    onchange: move |event| theme.set(event.value()),
                                    option { value: "light", selected: theme() == "light", {t!("profile-theme-light")} }
                                    option { value: "dark", selected: theme() == "dark", {t!("profile-theme-dark")} }
                                    option { value: "auto", selected: theme() == "auto", {t!("profile-theme-auto")} }
                                }
                            }
                        }
                        div { class: "px-6 pb-6",
                            button {
                                class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors",
                                r#type: "submit",
                                {t!("common-save")}
                            }
                        }
                    }
                }

                // Actions de compte.
                div { class: "space-y-6",
                    div { class: "bg-white rounded-lg shadow-sm",
                        div { class: "p-6 border-b border-gray-200",
                            h3 { class: "text-lg font-semibold text-gray-900", {t!("profile-account")} }
                        }
                        div { class: "p-6 space-y-3",
                            button {
                                class: "w-full flex items-center gap-3 px-4 py-3 bg-gray-50 hover:bg-gray-100 rounded-lg transition-colors text-left",
                                onclick: move |_| api::auth::start_pkce_login(Some("reset")),
                                icons::Key { class: "h-5 w-5 text-gray-600" }
                                span { class: "text-sm text-gray-900", {t!("profile-change-password")} }
                            }
                            button {
                                class: "w-full flex items-center gap-3 px-4 py-3 bg-red-50 hover:bg-red-100 rounded-lg transition-colors text-left",
                                onclick: move |_| session::logout(),
                                icons::LogOut { class: "h-5 w-5 text-red-600" }
                                span { class: "text-sm font-medium text-red-900", {t!("shell-logout")} }
                            }
                        }
                    }
                    p { class: "text-xs text-gray-400 px-2", {t!("profile-theme-note")} }
                }
            }
        }
    }
}

/// Le backend sérialise l'enum en Capitalized (« Light ») — le formulaire
/// travaille en minuscules.
fn normalize_theme(theme: &str) -> String {
    match theme.to_ascii_lowercase().as_str() {
        "light" | "dark" => theme.to_ascii_lowercase(),
        _ => "auto".into(),
    }
}
