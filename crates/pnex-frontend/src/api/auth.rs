//! OAuth2 calls (backend proxy to the Rauthy IdP) + PKCE flow start.

use pnex_core::TokenResponse;

use crate::api::client;
use crate::api::error::ApiError;
use crate::auth::pkce;
use crate::storage::{
    self, KeyValueStorage, KEY_ACCESS_TOKEN, KEY_FORCE_LOGIN, KEY_ID_TOKEN, KEY_PKCE_VERIFIER,
    KEY_REFRESH_TOKEN,
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

/// `POST /api/v1/oauth2/refresh` — refresh of the grant token.
pub async fn refresh_tokens(refresh_token: &str) -> Result<TokenResponse, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/oauth2/refresh",
        Some(serde_json::json!({ "refresh_token": refresh_token })),
    )
    .await
}

/// Stores the tokens (signed in) — the id_token is used for the Rauthy end-session.
pub fn store_tokens(tokens: &TokenResponse) {
    let local = storage::local();
    local.set(KEY_ACCESS_TOKEN, &tokens.access_token);
    local.set(KEY_REFRESH_TOKEN, &tokens.refresh_token);
    // A refresh response may omit the id_token: keep the previous one, the
    // end-session hint only needs to identify the user.
    if let Some(id_token) = &tokens.id_token {
        local.set(KEY_ID_TOKEN, id_token);
    }
    local.remove(KEY_FORCE_LOGIN);
}

/// The IdP session could not be ended (no id_token hint): the next login
/// forces the sign-in form, so a shared computer does not log the previous
/// user back in silently (SEC-41).
pub fn force_next_login() {
    storage::local().set(KEY_FORCE_LOGIN, "1");
}

/// Purges the tokens (logout / expiry).
pub fn clear_tokens() {
    let local = storage::local();
    local.remove(KEY_ACCESS_TOKEN);
    local.remove(KEY_REFRESH_TOKEN);
    local.remove(KEY_ID_TOKEN);
}

/// Reads the stored id_token — `session::logout()` captures it BEFORE the
/// local purge to feed the `id_token_hint` of the end-session flow.
pub fn stored_id_token() -> Option<String> {
    storage::local().get(KEY_ID_TOKEN)
}

/// Rauthy logout: full-page navigation to the backend proxy, which serves
/// an auto-submitted form POSTing the end-session at TOP-LEVEL — Rauthy's
/// final 302 to `post_logout_redirect_uri` becomes a real navigation: the
/// browser lands on `{origin}/` (signed-out boot → login screen). Call
/// AFTER the local purge (the id_token must have been captured before).
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

/// Callback URI of the PKCE flow — same computation at start and on return
/// (the URI must match the code signature on the Rauthy side).
pub fn redirect_uri() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let origin = web_sys::window()
            .map(|w| w.location().origin().ok().unwrap_or_default())
            .unwrap_or_default();
        // Dev bind "all interfaces": opened via 0.0.0.0 (or [::]), the
        // browser produces an origin not allowed by Rauthy — we fall back
        // to the loopback form (localhost), which serves the same UI.
        let origin = origin
            .replace("0.0.0.0", "localhost")
            .replace("[::]", "[::1]");
        format!("{origin}/auth/callback")
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Native: the backend bridge (auth/native.rs + controller oauth2.rs) —
        // same value at authorize and at exchange (compared exactly by
        // Rauthy). {base} = the server entered on the ServerUrl screen.
        format!("{}/api/v1/oauth2/native", crate::api::config::api_base())
    }
}

/// Starts the PKCE login: generates the S256 verifier/challenge pair, stores
/// the verifier in sessionStorage (survives the redirect, not the tab) then
/// redirects the browser to the backend SSO proxy (302 → Rauthy).
///
/// `action`: `Some("register")` (account creation) or `Some("reset")`
/// (password change) — mapped to `kc_action` on the backend side.
pub fn start_pkce_login(action: Option<&str>) {
    let pkce = pkce::generate();
    storage::session().set(KEY_PKCE_VERIFIER, &pkce.verifier);
    #[cfg(target_arch = "wasm32")]
    navigate(&sso_url(&pkce.challenge, action));
    // Native: system browser on the SSO flow with a correlation `state`
    // (= challenge, unique per flow — relayed by the SSO proxy, sent back to
    // the bridge, and reused as polling key) — see auth/native.rs.
    #[cfg(not(target_arch = "wasm32"))]
    crate::auth::native::open_login(
        format!(
            "{}{}&state={}",
            crate::api::config::api_base(),
            sso_url(&pkce.challenge, action),
            pkce::urlencode(&pkce.challenge)
        ),
        pkce.challenge,
    );
}

/// URL of the backend SSO proxy (302 → Rauthy), PKCE S256 mandatory.
fn sso_url(challenge: &str, action: Option<&str>) -> String {
    let mut url = format!(
        "/api/v1/oauth2/sso?code_challenge={challenge}&code_challenge_method=S256&redirect_uri={}",
        pkce::urlencode(&redirect_uri())
    );
    if let Some(action) = action {
        url.push_str(&format!("&action={}", pkce::urlencode(action)));
    } else if storage::local().get(KEY_FORCE_LOGIN).is_some() {
        url.push_str("&prompt=login");
    }
    url
}

/// PKCE verifier stored at start (consumed at the callback).
pub fn take_pkce_verifier() -> Option<String> {
    let session = storage::session();
    let verifier = session.get(KEY_PKCE_VERIFIER);
    session.remove(KEY_PKCE_VERIFIER);
    verifier
}

/// Full-page redirect (unloads the SPA for the Rauthy login page) — web
/// only: natively, `start_pkce_login` goes through the system browser +
/// backend bridge (auth/native.rs), without `navigate`.
#[cfg(target_arch = "wasm32")]
fn navigate(url: &str) {
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_href(url);
    }
}
