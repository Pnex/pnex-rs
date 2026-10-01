//! Base des URLs API, dépendante de la plateforme.
//!
//! - **web** : le front est servi par le backend → same-origin. reqwest exige
//!   des URLs **absolues** (même en wasm, le parse `Url` refuse les chemins
//!   seuls) : la base est donc l'origine de la page, sauf override à la
//!   compilation (`PNEX_API_BASE_URL`, utilisé par la boucle de dev hot —
//!   `task dev:hot` sert le front via dx sur :5151, l'API Loco est sur :5150).
//! - **natif (future cible desktop/ios/android)** : le front n'est PAS servi
//!   par le backend — URL serveur « auto-hébergée » façon Bitwarden, lue à
//!   l'exécution (env, puis préférence stockée par l'écran `ServerUrl`).
//!   C'est le seam multi-plateforme du portage : rien d'autre ne connaît
//!   l'origine de l'API.

// Uniquement côté natif (lecture de l'URL serveur stockée — cible desktop).
#[cfg(not(target_arch = "wasm32"))]
use crate::storage::KeyValueStorage;

/// Production server baked into native builds (`PNEX_PROD_HOST` at compile
/// time, bare host or full URL): the server screens disappear and users can
/// never enter or change the server. Always `None` on web (same-origin).
pub fn locked_server() -> Option<String> {
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    normalize_locked(option_env!("PNEX_PROD_HOST"))
}

fn normalize_locked(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim().trim_end_matches('/');
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with("https://") || raw.starts_with("http://") {
        Some(raw.to_string())
    } else {
        Some(format!("https://{raw}"))
    }
}

/// Base à préfixer à tous les chemins d'API (jamais de slash final).
pub fn api_base() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        // Compile-time d'abord (dev hot), sinon origine de la page (same-origin).
        match option_env!("PNEX_API_BASE_URL") {
            Some(base) => base.trim_end_matches('/').to_string(),
            None => web_sys::window()
                .map(|w| w.location().origin().ok().unwrap_or_default())
                .unwrap_or_default()
                .trim_end_matches('/')
                .to_string(),
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(locked) = locked_server() {
            return locked;
        }
        std::env::var("PNEX_API_BASE_URL")
            .ok()
            .or_else(|| crate::storage::local().get(crate::storage::KEY_API_BASE))
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_server_accepts_bare_host_or_url() {
        assert_eq!(normalize_locked(None), None);
        assert_eq!(normalize_locked(Some(" ")), None);
        assert_eq!(
            normalize_locked(Some("iot.example.com")).as_deref(),
            Some("https://iot.example.com")
        );
        assert_eq!(
            normalize_locked(Some("http://10.0.0.2:5150/")).as_deref(),
            Some("http://10.0.0.2:5150")
        );
    }

    /// En natif sans config : base vide (la cible desktop fournira l'URL).
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn base_native_sans_config_vide() {
        std::env::remove_var("PNEX_API_BASE_URL");
        assert_eq!(api_base(), "");
    }
}
