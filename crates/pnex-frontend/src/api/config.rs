//! API URL base, platform-dependent.
//!
//! - **web**: the front is served by the backend → same-origin. reqwest requires
//!   **absolute** URLs (even in wasm, the `Url` parse rejects bare paths):
//!   so the base is the page origin, unless overridden at compile time
//!   (`PNEX_API_BASE_URL`, used by the hot dev loop —
//!   `task dev:hot` serves the front via dx on :5151, the Loco API is on :5150).
//! - **native (future desktop/ios/android target)**: the front is NOT served
//!   by the backend — "self-hosted" server URL, Bitwarden style, read at
//!   runtime (env, then the preference stored by the `ServerUrl` screen).
//!   This is the multi-platform seam of the port: nothing else knows
//!   the API origin.

// Native only (reads the stored server URL — desktop target).
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

/// Base to prefix to every API path (never a trailing slash).
pub fn api_base() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        // Compile-time first (hot dev), else the page origin (same-origin).
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

    /// Native without config: empty base (the desktop target will provide the URL).
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn base_native_sans_config_vide() {
        std::env::remove_var("PNEX_API_BASE_URL");
        assert_eq!(api_base(), "");
    }
}
