//! Page de connexion — portée du `AuthWrapper.tsx` React (branding
//! « Welcome to PNeX », fond réseau animé approché en CSS/SVG). Le login est
//! un redirect PKCE vers Keycloak via le proxy backend.

use dioxus::prelude::*;
use dioxus_i18n::t;

#[component]
pub fn Login() -> Element {
    rsx! { h1 { class: "text-3xl font-bold text-gray-900", {t!("login-title")} } }
}
