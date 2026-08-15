//! Base des URLs API, dépendante de la plateforme.
//!
//! - **web** : le front est servi par le backend → same-origin, URLs
//!   relatives (base vide). Exception : `PNEX_API_BASE_URL` au moment de la
//!   compilation, pour la boucle de dev hot (`task dev:hot` sert le front via
//!   dx sur :5151, l'API Loco est sur :5150).
//! - **natif (future cible desktop/ios/android)** : le front n'est PAS servi
//!   par le backend — URL serveur « auto-hébergée » façon Bitwarden, lue à
//!   l'exécution (env, puis préférence stockée par l'écran `ServerUrl`).
//!   C'est le seam multi-plateforme du portage : rien d'autre ne connaît
//!   l'origine de l'API.

use crate::storage::KeyValueStorage;

/// Base à préfixer à tous les chemins d'API ("" = same-origin).
pub fn api_base() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        // Compile-time : le wasm n'a pas d'env d'exécution exploitable.
        option_env!("PNEX_API_BASE_URL")
            .unwrap_or("")
            .trim_end_matches('/')
            .to_string()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("PNEX_API_BASE_URL")
            .ok()
            .or_else(|| crate::storage::local().get(crate::storage::KEY_API_BASE))
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string()
    }
}
