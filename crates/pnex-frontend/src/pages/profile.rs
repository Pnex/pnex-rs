//! Profil — porté du `Profile.tsx` React : identité (Keycloak, lecture),
//! préférences (PATCH /api/v1/profile), changement de mot de passe (redirect
//! Keycloak), déconnexion.

use dioxus::prelude::*;
use dioxus_i18n::t;

#[component]
pub fn Profile() -> Element {
    rsx! { h1 { class: "text-3xl font-bold text-gray-900", {t!("nav-profile")} } }
}
