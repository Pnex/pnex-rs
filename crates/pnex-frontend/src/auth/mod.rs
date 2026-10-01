//! Authentification côté front : PKCE (génération + redirection), session.

pub mod pkce;

// Login natif : navigateur système + pont backend (polling) — n'existe pas
// sur web, où le callback /auth/callback arrive dans le même navigateur.
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
