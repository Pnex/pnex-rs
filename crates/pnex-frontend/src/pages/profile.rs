//! Profile — ported from the React `Profile.tsx`: identity (managed by the
//! Rauthy IdP, read-only in the app), preferences (`PATCH /api/v1/profile`:
//! language, timezone, date format, theme), password change (redirect to
//! the Rauthy account page `/auth/v1/account`), logout.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::icons;
use crate::components::ui::BTN_PRIMARY;
use crate::components::ui::{FIELD_LABEL, INPUT};
use crate::i18n;
use crate::state::{session, toasts};

#[component]
pub fn Profile() -> Element {
    let user = session::user();
    let Some(user) = user else {
        return rsx! {};
    };
    let profile = user.profile.clone().unwrap_or_default();

    // Current server (version + contract) — public meta endpoint, same call
    // as the boot compatibility gate.
    let server = use_resource(|| async move {
        crate::api::meta::server_info(std::time::Duration::from_secs(4))
            .await
            .ok()
    });
    // t!() outside the rsx: the value is computed before rendering (the
    // i18n dictionary lives in the component's scope).
    let server_value = match server.cloned() {
        Some(Some(info)) => t!(
            "about-server-value",
            version: info.version,
            contract: info.contract.to_string()
        ),
        Some(None) => t!("about-server-unknown"),
        None => t!("about-server-loading"),
    };

    // Initial form values = current profile (init only once).
    let mut language = use_signal(|| profile.language.clone());
    let mut timezone = use_signal(|| profile.timezone.clone());
    let mut date_format = use_signal(|| profile.date_format.clone().unwrap_or_default());
    let mut theme = use_signal(|| normalize_theme(profile.theme.as_str()));

    rsx! {
        crate::components::crud::layout::ListLayout {
            title: t!("nav-profile").to_string(),
            subtitle: t!("profile-subtitle").to_string(),
            can_write: false,

            div { class: "grid grid-cols-1 lg:grid-cols-3 gap-8",
                div { class: "lg:col-span-2 space-y-6",
                    // Identity — managed by the Rauthy IdP, read-only.
                    div { class: "bg-white rounded-lg border border-line",
                        div { class: "p-6 border-b border-gray-200",
                            h2 { class: "text-lg font-semibold text-gray-900",
                                {t!("profile-identity")}
                            }
                        }
                        div { class: "p-6",
                            div { class: "flex items-center space-x-6 mb-8",
                                div { class: "w-20 h-20 bg-signal-100 rounded-full flex items-center justify-center",
                                    icons::User { class: "h-8 w-8 text-signal-600" }
                                }
                                div {
                                    h3 { class: "text-xl font-semibold text-gray-900",
                                        {user.full_name.clone().unwrap_or_else(|| user.username.clone())}
                                    }
                                    p { class: "text-gray-600",
                                        {user.email.clone().unwrap_or_else(|| "—".into())}
                                    }
                                }
                            }
                            div { class: "grid grid-cols-1 md:grid-cols-2 gap-6",
                                label { class: "block",
                                    span { class: FIELD_LABEL, {t!("profile-username")} }
                                    input {
                                        class: "{INPUT} w-full",
                                        r#type: "text",
                                        disabled: true,
                                        value: "{user.username}",
                                    }
                                }
                                label { class: "block",
                                    span { class: FIELD_LABEL, {t!("profile-email")} }
                                    input {
                                        class: "{INPUT} w-full",
                                        r#type: "email",
                                        disabled: true,
                                        value: "{user.email.clone().unwrap_or_default()}",
                                    }
                                }
                                p { class: "md:col-span-2 text-xs text-gray-500",
                                    {t!("profile-idp-managed")}
                                }
                            }
                        }
                    }

                    // Preferences — PATCH /api/v1/profile.
                    form {
                        class: "bg-white rounded-lg border border-line",
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
                                        // resyncs the session (up-to-date profile)
                                        if let Ok(fresh) = api::user::get_user_info().await {
                                            session::login(fresh);
                                        }
                                    }
                                    Err(err) => toasts::error(err),
                                }
                            });
                        },
                        div { class: "p-6 border-b border-gray-200",
                            h2 { class: "text-lg font-semibold text-gray-900",
                                {t!("profile-preferences")}
                            }
                        }
                        div { class: "p-6 grid grid-cols-1 md:grid-cols-2 gap-6",
                            label { class: "block",
                                span { class: FIELD_LABEL, {t!("profile-language")} }
                                select {
                                    class: "{INPUT} w-full",
                                    onchange: move |event| {
                                        let tag = event.value();
                                        language.set(tag.clone());
                                        // Applies immediately (persisted locally;
                                        // the PATCH saves on the server side).
                                        i18n::set_locale(&tag);
                                    },
                                    option {
                                        value: "en",
                                        selected: language() == "en",
                                        "English"
                                    }
                                    option {
                                        value: "fr",
                                        selected: language() == "fr",
                                        "Français"
                                    }
                                }
                            }
                            label { class: "block",
                                span { class: FIELD_LABEL, {t!("profile-timezone")} }
                                input {
                                    class: "{INPUT} w-full",
                                    r#type: "text",
                                    value: "{timezone}",
                                    oninput: move |event| timezone.set(event.value()),
                                }
                            }
                            label { class: "block",
                                span { class: FIELD_LABEL, {t!("profile-date-format")} }
                                input {
                                    class: "{INPUT} w-full",
                                    r#type: "text",
                                    placeholder: "YYYY-MM-DD HH:mm",
                                    value: "{date_format}",
                                    oninput: move |event| date_format.set(event.value()),
                                }
                            }
                            label { class: "block",
                                span { class: FIELD_LABEL, {t!("profile-theme")} }
                                select {
                                    class: "{INPUT} w-full",
                                    onchange: move |event| {
                                        // Preview at once, like the language; saving persists it.
                                        crate::state::theme::apply(&event.value());
                                        theme.set(event.value());
                                    },
                                    option {
                                        value: "light",
                                        selected: theme() == "light",
                                        {t!("profile-theme-light")}
                                    }
                                    option {
                                        value: "dark",
                                        selected: theme() == "dark",
                                        {t!("profile-theme-dark")}
                                    }
                                    option {
                                        value: "auto",
                                        selected: theme() == "auto",
                                        {t!("profile-theme-auto")}
                                    }
                                }
                            }
                        }
                        div { class: "px-6 pb-6",
                            button { class: BTN_PRIMARY, r#type: "submit", {t!("common-save")} }
                        }
                    }
                }

                // Account actions.
                div { class: "space-y-6",
                    div { class: "bg-white rounded-lg border border-line",
                        div { class: "p-6 border-b border-gray-200",
                            h3 { class: "text-lg font-semibold text-gray-900",
                                {t!("profile-account")}
                            }
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
                    // About — app + server versions and API contract:
                    // clear identification of both sides (clean refusal by
                    // the compatibility gate, support).
                    div { class: "bg-white rounded-lg border border-line",
                        div { class: "p-6 border-b border-gray-200",
                            h3 { class: "text-lg font-semibold text-gray-900", {t!("about-title")} }
                        }
                        div { class: "p-6 space-y-3 text-sm",
                            div { class: "flex items-center justify-between",
                                span { class: "text-gray-500", {t!("about-app-version")} }
                                span { class: "text-gray-900 font-medium", {crate::version::full()} }
                            }
                            div { class: "flex items-center justify-between",
                                span { class: "text-gray-500", {t!("about-contract")} }
                                span { class: "text-gray-900 font-medium",
                                    {pnex_api_contract::CONTRACT.to_string()}
                                }
                            }
                            div { class: "flex items-center justify-between",
                                span { class: "text-gray-500", {t!("about-server")} }
                                span { class: "text-gray-900 font-medium text-right",
                                    {server_value}
                                }
                            }
                        }
                    }
                    crate::components::server_ca_card::ServerCaCard {}
                    p { class: "text-xs text-gray-500 px-2", {t!("profile-theme-note")} }
                }
            }
        }
    }
}

/// The backend serializes the enum Capitalized ("Light") — the form
/// works in lowercase.
fn normalize_theme(theme: &str) -> String {
    match theme.to_ascii_lowercase().as_str() {
        "light" | "dark" => theme.to_ascii_lowercase(),
        _ => "auto".into(),
    }
}
