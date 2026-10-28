//! Front-side authentication: PKCE (generation + redirect), session.

pub mod pkce;

// Native login: system browser + backend bridge (polling) — does not exist
// on the web, where the /auth/callback callback lands in the same browser.
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
