//! Écran « URL du serveur » — façon Bitwarden, pour les futures cibles
//! desktop/ios/android où le front n'est PAS servi par le backend (donc pas
//! de same-origin). Le web n'utilise jamais cet écran : URLs relatives.
//!
//! Non routé pour l'instant (phase desktop ultérieure) mais compilé et
//! maintenu pour ne pas pourrir — la cible desktop le branchera sur son
//! premier lancement lorsque `api::config::api_base()` n'est pas résolue.

use dioxus::prelude::*;
use dioxus_i18n::t;

/// URL serveur candidate, validée côté saisie (schéma http/https requis).
#[component]
#[allow(dead_code)]
pub fn ServerUrl() -> Element {
    rsx! { p { {t!("server-url-title")} } }
}
