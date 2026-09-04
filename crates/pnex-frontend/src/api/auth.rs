//! Appels OAuth2 (proxy backend vers l'IdP Rauthy) + démarrage du flow PKCE.

use pnex_core::TokenResponse;

use crate::api::client;
use crate::api::error::ApiError;
use crate::auth::pkce;
use crate::storage::{
    self, KeyValueStorage, KEY_ACCESS_TOKEN, KEY_ID_TOKEN, KEY_PKCE_VERIFIER, KEY_REFRESH_TOKEN,
};

/// `POST /api/v1/oauth2/token` (grant `authorization_code` + PKCE).
pub async fn exchange_code(
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<TokenResponse, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/oauth2/token",
        Some(serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "code_verifier": code_verifier,
            "redirect_uri": redirect_uri,
        })),
    )
    .await
}

/// `POST /api/v1/oauth2/refresh` — refresh du grant token.
pub async fn refresh_tokens(refresh_token: &str) -> Result<TokenResponse, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/oauth2/refresh",
        Some(serde_json::json!({ "refresh_token": refresh_token })),
    )
    .await
}

/// Stocke les tokens (connecté) — l'id_token sert à l'end-session Rauthy.
pub fn store_tokens(tokens: &TokenResponse) {
    let local = storage::local();
    local.set(KEY_ACCESS_TOKEN, &tokens.access_token);
    local.set(KEY_REFRESH_TOKEN, &tokens.refresh_token);
    match &tokens.id_token {
        Some(id_token) => local.set(KEY_ID_TOKEN, id_token),
        None => local.remove(KEY_ID_TOKEN),
    }
}

/// Purge les tokens (déconnexion / expiration).
pub fn clear_tokens() {
    let local = storage::local();
    local.remove(KEY_ACCESS_TOKEN);
    local.remove(KEY_REFRESH_TOKEN);
    local.remove(KEY_ID_TOKEN);
}

/// Lit l'id_token stocké — `session::logout()` le capture AVANT la purge
/// locale pour alimenter le `id_token_hint` du flux end-session.
pub fn stored_id_token() -> Option<String> {
    storage::local().get(KEY_ID_TOKEN)
}

/// Déconnexion Rauthy : navigation pleine page vers le proxy backend, qui
/// sert un formulaire auto-soumis POSTant l'end-session en TOP-LEVEL — le
/// 302 final de Rauthy vers `post_logout_redirect_uri` devient une vraie
/// navigation : le navigateur atterrit sur `{origine}/` (boot déconnecté →
/// écran de login). À appeler APRÈS la purge locale (l'id_token doit avoir
/// été capturé avant).
#[cfg(target_arch = "wasm32")]
pub fn end_session(id_token: Option<String>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let mut url = format!(
        "/api/v1/oauth2/logout?post_logout_redirect_uri={}",
        pkce::urlencode(&format!(
            "{}/",
            window.location().origin().ok().unwrap_or_default()
        ))
    );
    if let Some(token) = id_token {
        url.push_str(&format!("&id_token={}", pkce::urlencode(&token)));
    }
    let _ = window.location().set_href(&url);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn end_session(_id_token: Option<String>) {}

/// URI de callback du flow PKCE — même calcul au départ et au retour (l'URI
/// doit être identique à la signature du code côté Rauthy).
pub fn redirect_uri() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let origin = web_sys::window()
            .map(|w| w.location().origin().ok().unwrap_or_default())
            .unwrap_or_default();
        format!("{origin}/auth/callback")
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Cible desktop : schéma/loopback dédiés — phase ultérieure.
        String::from("http://localhost/auth/callback")
    }
}

/// Démarre le login PKCE : génère la paire verifier/challenge S256, stocke le
/// verifier en sessionStorage (survit à la redirection, pas à l'onglet) puis
/// redirige le navigateur vers le proxy SSO backend (302 → Rauthy).
///
/// `action` : `Some("register")` (création de compte) ou `Some("reset")`
/// (changement de mot de passe) — mappés `kc_action` côté backend.
pub fn start_pkce_login(action: Option<&str>) {
    let pkce = pkce::generate();
    storage::session().set(KEY_PKCE_VERIFIER, &pkce.verifier);
    let url = sso_url(&pkce.challenge, action);
    navigate(&url);
}

/// URL du proxy SSO backend (302 → Rauthy), PKCE S256 obligatoire.
fn sso_url(challenge: &str, action: Option<&str>) -> String {
    let mut url = format!(
        "/api/v1/oauth2/sso?code_challenge={challenge}&code_challenge_method=S256&redirect_uri={}",
        pkce::urlencode(&redirect_uri())
    );
    if let Some(action) = action {
        url.push_str(&format!("&action={}", pkce::urlencode(action)));
    }
    url
}

/// Verifier PKCE stocké au moment du départ (consommé au callback).
pub fn take_pkce_verifier() -> Option<String> {
    let session = storage::session();
    let verifier = session.get(KEY_PKCE_VERIFIER);
    session.remove(KEY_PKCE_VERIFIER);
    verifier
}

/// Redirection plein page (décharge le SPA pour la page de login Rauthy).
#[cfg(target_arch = "wasm32")]
fn navigate(url: &str) {
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_href(url);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn navigate(_url: &str) {
    // Cible desktop : le flow ouvrira une webview dédiée (phase desktop) —
    // sans navigateur, la redirection n'a pas d'objet ici.
}
