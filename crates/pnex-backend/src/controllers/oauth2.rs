//! OAuth2 proxy to Rauthy (full-Rust OIDC IdP, D10):
//!
//! - `POST /api/v1/oauth2/token`: grants `password` (dev/tests) and
//!   `authorization_code` + PKCE. Our own validation errors are `ErrorDetail`;
//!   identity provider errors are relayed verbatim in the RFC 6749 §5.2
//!   shape `{error, error_description}` (standard OAuth2 contract);
//! - `POST /api/v1/oauth2/refresh`: `grant_type=refresh_token`;
//! - `GET /api/v1/oauth2/sso`: 302 to the Rauthy authorize endpoint, PKCE
//!   S256 mandatory; `action=register` goes to the Rauthy sign-up page,
//!   `action=reset` to the Rauthy account page;
//! - `GET /api/v1/oauth2/logout`: auto-submitting HTML form to the Rauthy
//!   end-session endpoint (RP-initiated logout).
//!
//! The client stays public (no secret in the browser): PKCE is enough.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::auth::settings::RauthySettings;
use pnex_core::err_codes::{FIELD_INVALID, FIELD_REQUIRED};

fn http() -> &'static reqwest::Client {
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            // Rauthy rejects requests without a User-Agent (400).
            .user_agent("pnex-server")
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client")
    })
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GrantKind {
    Password,
    AuthorizationCode,
    RefreshToken,
}

/// Body of `/token` and `/refresh`; the fields depend on the grant.
#[derive(Deserialize, Default, utoipa::ToSchema)]
#[schema(
    description = "Token request; required fields depend on `grant_type`: password (username, password), authorization_code (code, code_verifier, redirect_uri), refresh_token or absent (refresh_token)."
)]
pub struct TokenParams {
    pub grant_type: Option<GrantKind>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub code: Option<String>,
    pub code_verifier: Option<String>,
    pub redirect_uri: Option<String>,
    pub refresh_token: Option<String>,
}

/// `field-invalid` 400 marking every missing grant field `required`.
fn missing_fields(fields: &[(&str, bool)]) -> Response {
    let map = fields
        .iter()
        .filter(|(_, missing)| *missing)
        .map(|(name, _)| ((*name).to_string(), FIELD_REQUIRED.into()))
        .collect();
    crate::controllers::fields_error(StatusCode::BAD_REQUEST, map)
}

/// Relays the Rauthy answer (status + JSON) verbatim.
async fn relay(response: reqwest::Response) -> Result<Response> {
    let status = StatusCode::from_u16(response.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body: serde_json::Value = response.json().await.unwrap_or(serde_json::json!({
        "error": "invalid_response",
        "error_description": "the identity provider returned an unreadable answer",
    }));
    let mut response = axum::Json(body).into_response();
    *response.status_mut() = status;
    Ok(response)
}

#[utoipa::path(
    post,
    path = "/api/v1/oauth2/token",
    tag = "oauth2",
    operation_id = "oauth2_token",
    summary = "Obtain tokens from the identity provider",
    description = "Proxy to the Rauthy token endpoint (public client, scope `openid profile email`): grants `password` (dev/tests), `authorization_code` with PKCE, or `refresh_token`. The provider answer (status and JSON) is relayed as is. Error shapes: a missing grant field is an `ErrorDetail` (`field-invalid`, `errors.fields.<field> = required`); an identity provider refusal (bad credentials, expired code...) keeps the standard OAuth2 error body of RFC 6749 section 5.2, `{error, error_description}`, relayed verbatim with the provider status (400 or 401), because OAuth2 clients expect that exact shape. No authentication.",
    request_body = TokenParams,
    responses(
        (status = 200, body = pnex_core::TokenResponse),
        (status = 400, description = "`field-invalid` (`ErrorDetail`) for a missing grant field, or the provider refusal in the RFC 6749 section 5.2 shape `{error, error_description}` (standard OAuth2 exception to `ErrorDetail`)", body = serde_json::Value),
        (status = 401, description = "Provider refusal (`invalid_client`...) in the RFC 6749 section 5.2 shape `{error, error_description}`", body = serde_json::Value),
        (status = 502, description = "Identity provider unreachable (`upstream`)", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn token(State(ctx): State<AppContext>, Json(params): Json<TokenParams>) -> Result<Response> {
    let settings = RauthySettings::from_config(&ctx.config)?;

    // Standard scope (like the SSO flow): guarantees the id_token is issued,
    // required for end-session (`id_token_hint`).
    let mut form: Vec<(&str, String)> = vec![
        ("client_id", settings.client_id.clone()),
        ("scope", "openid profile email".into()),
    ];
    match params.grant_type {
        Some(GrantKind::Password) => {
            let missing = [
                ("username", params.username.is_none()),
                ("password", params.password.is_none()),
            ];
            let (Some(username), Some(password)) = (params.username, params.password) else {
                return Ok(missing_fields(&missing));
            };
            form.push(("grant_type", "password".into()));
            form.push(("username", username));
            form.push(("password", password));
        }
        Some(GrantKind::AuthorizationCode) => {
            let missing = [
                ("code", params.code.is_none()),
                ("code_verifier", params.code_verifier.is_none()),
                ("redirect_uri", params.redirect_uri.is_none()),
            ];
            let (Some(code), Some(code_verifier), Some(redirect_uri)) =
                (params.code, params.code_verifier, params.redirect_uri)
            else {
                return Ok(missing_fields(&missing));
            };
            form.push(("grant_type", "authorization_code".into()));
            form.push(("code", code));
            form.push(("code_verifier", code_verifier));
            form.push(("redirect_uri", redirect_uri));
        }
        Some(GrantKind::RefreshToken) | None => {
            let Some(refresh_token) = params.refresh_token else {
                return Ok(crate::controllers::field_error(
                    "refresh_token",
                    FIELD_REQUIRED,
                ));
            };
            form.push(("grant_type", "refresh_token".into()));
            form.push(("refresh_token", refresh_token));
        }
    }

    let upstream = http()
        .post(settings.token_endpoint())
        .form(&form)
        .send()
        .await
        .map_err(|err| {
            tracing::error!(%err, "Rauthy unreachable (token)");
            Error::CustomError(
                StatusCode::BAD_GATEWAY,
                loco_rs::controller::ErrorDetail::new(
                    "upstream",
                    "identity provider unreachable".to_string(),
                ),
            )
        })?;
    relay(upstream).await
}

/// Body of `POST /api/v1/oauth2/refresh` (documentation; the handler reads
/// the raw JSON so a missing field answers 400).
#[derive(utoipa::ToSchema)]
#[allow(dead_code)]
struct RefreshParams {
    refresh_token: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/oauth2/refresh",
    tag = "oauth2",
    operation_id = "oauth2_refresh",
    summary = "Refresh an access token",
    description = "Exchanges a refresh token for new tokens at the identity provider (Rauthy); the provider answer (status and JSON) is relayed as is. A missing `refresh_token` is an `ErrorDetail` (`field-invalid`); a provider refusal (expired or revoked token) keeps the standard OAuth2 error body of RFC 6749 section 5.2, `{error, error_description}`, relayed verbatim. No authentication.",
    request_body = RefreshParams,
    responses(
        (status = 200, body = pnex_core::TokenResponse),
        (status = 400, description = "`field-invalid` (`ErrorDetail`) for a missing `refresh_token`, or the provider refusal in the RFC 6749 section 5.2 shape `{error, error_description}` (standard OAuth2 exception to `ErrorDetail`)", body = serde_json::Value),
        (status = 401, description = "Provider refusal in the RFC 6749 section 5.2 shape `{error, error_description}`", body = serde_json::Value),
        (status = 502, description = "Identity provider unreachable (`upstream`)", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn refresh(
    State(ctx): State<AppContext>,
    Json(body): Json<serde_json::Value>,
) -> Result<Response> {
    let Some(refresh_token) = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        return Ok(crate::controllers::field_error(
            "refresh_token",
            FIELD_REQUIRED,
        ));
    };
    let params = TokenParams {
        grant_type: Some(GrantKind::RefreshToken),
        refresh_token: Some(refresh_token),
        ..Default::default()
    };
    token(State(ctx), Json(params)).await
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LogoutParams {
    /// Where to land after logout (app origin, e.g. `http://localhost:5150/`).
    /// Validated by Rauthy against the client allow-list, never here.
    pub post_logout_redirect_uri: Option<String>,
    /// `id_token_hint`, optional: without it Rauthy falls back to its cookie
    /// session.
    pub id_token: Option<String>,
}

/// `GET /api/v1/oauth2/logout` — serves a self-submitting HTML form to the
/// Rauthy end-session. This POST is a TOP-LEVEL navigation: Rauthy's final
/// 302 to `post_logout_redirect_uri` becomes a real navigation and the
/// browser lands straight on the app (logged-out boot → login screen).
/// It is the only path that really redirects: Rauthy's SPA logout page
/// POSTs via `fetch` then always goes to its hardcoded landing
/// `/auth/v1/` — never the app (checked in the 0.36.2 source).
#[utoipa::path(
    get,
    path = "/api/v1/oauth2/logout",
    tag = "oauth2",
    operation_id = "oauth2_logout",
    summary = "Log out at the identity provider",
    description = "Browser navigation target: serves an auto-submitting HTML form that POSTs to the Rauthy end-session endpoint (RP-initiated logout), which then redirects to `post_logout_redirect_uri`. Ends the identity provider session. No authentication.",
    params(LogoutParams),
    responses(
        (status = 200, description = "Auto-submitting HTML form", content_type = "text/html", body = String)
    ),
    security(())
)]
async fn logout(
    State(ctx): State<AppContext>,
    Query(params): Query<LogoutParams>,
) -> Result<Response> {
    let settings = RauthySettings::from_config(&ctx.config)?;
    let post_logout_redirect_uri = params
        .post_logout_redirect_uri
        .unwrap_or_else(|| format!("{}/auth/v1/", settings.base_url));

    // Minimal HTML escaping (the id_token is base64url + dots, the URI
    // comes from the front — defense in depth against attribute injection).
    fn esc(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }
    let hint = params
        .id_token
        .map(|t| {
            format!(
                r#"<input type="hidden" name="id_token_hint" value="{}">"#,
                esc(&t)
            )
        })
        .unwrap_or_default();
    let html = format!(
        "<!DOCTYPE html><html><body>\
<form id=\"f\" method=\"POST\" action=\"{action}\">\
{hint}\
<input type=\"hidden\" name=\"post_logout_redirect_uri\" value=\"{uri}\">\
</form>\
<script>document.getElementById('f').submit()</script>\
</body></html>",
        action = esc(&settings.end_session_endpoint()),
        hint = hint,
        uri = esc(&post_logout_redirect_uri),
    );

    Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response())
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SsoParams {
    /// `register` | `reset` (absent = simple login).
    pub action: Option<String>,
    /// PKCE challenge, required.
    pub code_challenge: Option<String>,
    /// Must be `S256` (default).
    pub code_challenge_method: Option<String>,
    /// Default: `<scheme>://<host>/auth/callback` of the request.
    pub redirect_uri: Option<String>,
    /// Opaque state relayed to the identity provider, which returns it to
    /// `redirect_uri` (native login bridge correlation).
    pub state: Option<String>,
    /// `login` only: forces the sign-in form even with a live IdP session
    /// (after a logout that could not end it, SEC-41). Other values ignored.
    pub prompt: Option<String>,
}

/// Encodes a key/value pair as a query string (application/x-www-form-urlencoded).
fn form_urlencode(pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", k, urlencode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[utoipa::path(
    get,
    path = "/api/v1/oauth2/sso",
    tag = "oauth2",
    operation_id = "oauth2_sso",
    summary = "Start a browser sign-in (redirect)",
    description = "Browser navigation target: redirects (302) to the Rauthy authorize endpoint with PKCE S256 (authorization code flow), or to the Rauthy registration page (`action=register`) or account page (`action=reset`). `prompt=login` forces the sign-in form. No authentication.",
    params(SsoParams),
    responses(
        (status = 302, description = "Redirect to the identity provider (`Location` header)"),
        (status = 400, description = "`field-invalid`: `code_challenge` required, `code_challenge_method` not S256 or `redirect_uri` invalid", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn sso(
    State(ctx): State<AppContext>,
    Query(params): Query<SsoParams>,
    request: axum::extract::Request,
) -> Result<Response> {
    let settings = RauthySettings::from_config(&ctx.config)?;

    let Some(code_challenge) = params.code_challenge else {
        return Ok(crate::controllers::field_error(
            "code_challenge",
            FIELD_REQUIRED,
        ));
    };
    let method = params
        .code_challenge_method
        .unwrap_or_else(|| "S256".into());
    if method != "S256" {
        return Ok(crate::controllers::field_error(
            "code_challenge_method",
            FIELD_INVALID,
        ));
    }
    let redirect_uri = params.redirect_uri.unwrap_or_else(|| {
        let host = request
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("localhost:5150");
        // Behind the TLS edge (D70) nginx terminates https: keep the scheme
        // the browser actually used, or Rauthy rejects the redirect_uri.
        let scheme = request
            .headers()
            .get("x-forwarded-proto")
            .and_then(|h| h.to_str().ok())
            .filter(|p| *p == "https")
            .unwrap_or("http");
        format!("{scheme}://{host}/auth/callback")
    });
    // Dev bind on "all interfaces": a redirect_uri on 0.0.0.0/[::] is not
    // allowed by Rauthy — normalized to the loopback form.
    let redirect_uri = redirect_uri
        .replace("0.0.0.0", "localhost")
        .replace("[::]:", "[::1]:");

    let mut pairs: Vec<(&str, String)> = vec![
        ("client_id", settings.client_id.clone()),
        ("response_type", "code".into()),
        ("scope", "openid profile email".into()),
        ("redirect_uri", redirect_uri),
        ("code_challenge", code_challenge),
        ("code_challenge_method", "S256".into()),
    ];
    if let Some(state) = params.state {
        pairs.push(("state", state));
    }
    if params.prompt.as_deref() == Some("login") {
        pairs.push(("prompt", "login".into()));
    }
    // Rauthy UI pages (not OIDC endpoints): `action=register` →
    // sign-up form (activation by mail), `action=reset` →
    // account page where the password change lives. In both cases,
    // the user comes back to the pnex app and (re)starts the login.
    let location = match params.action.as_deref() {
        Some("register") => settings.register_page(),
        Some("reset") => settings.account_page(),
        _ => format!(
            "{}?{}",
            settings.authorize_endpoint(),
            form_urlencode(&pairs)
        ),
    };

    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::FOUND;
    let Ok(location) = HeaderValue::from_str(&location) else {
        return Ok(crate::controllers::field_error(
            "redirect_uri",
            FIELD_INVALID,
        ));
    };
    response.headers_mut().insert(header::LOCATION, location);
    Ok(response)
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/oauth2")
        .add("/token", post(token))
        .add("/refresh", post(refresh))
        .add("/sso", get(sso))
        .add("/logout", get(logout))
        // Pont de callback natif (login navigateur + polling app).
        .add("/native", get(native_callback))
        .add("/native/{state}", get(native_poll))
}

// ===== Native callback bridge =====
//
// dioxus-desktop opens every http(s) navigation in the system browser
// (`webbrowser::open` hardcoded in its navigation wrapper): an embedded
// webview is impossible without a fork. Native login therefore opens the browser
// on the SSO flow with redirect_uri = `/api/v1/oauth2/native` (this page);
// Rauthy brings `?code&state` back there, and the app — left in the background —
// fetches the code by polling `/native/{state}`, exchanges it for
// tokens (the PKCE verifier never leaves the app) and opens the session:
// the user finds the app already logged in on return.
//
// The random `state` (PKCE challenge, unique per flow) is the only access key
// to the code: it is the bridge's capability secret — it only transits
// through the user's browser.

//
// Cross-pod: the callback and the polls may hit different pods, so the
// `state → code` entry lives in Valkey (`SET EX 300`, consumed by `GETDEL`
// on the first successful poll). Without Valkey (dev single-node) a bounded
// in-memory map is used instead.

/// Local fallback store (used when Valkey is absent or failing).
static NATIVE_BRIDGE: LazyLock<Mutex<HashMap<String, (String, Instant)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
const NATIVE_BRIDGE_TTL: Duration = Duration::from_secs(300);
/// Bound of the local fallback map (unauthenticated writes).
const NATIVE_BRIDGE_MAX: usize = 10_000;
/// Input bounds: PKCE states are short random strings, Rauthy codes too.
const NATIVE_STATE_MAX_LEN: usize = 512;
const NATIVE_CODE_MAX_LEN: usize = 4096;
/// Bound of one Valkey round-trip on the request path.
const NATIVE_VALKEY_TIMEOUT: Duration = Duration::from_secs(1);

/// Valkey key of a bridge entry — the state is hashed (fixed length, no
/// attacker-chosen bytes in key names).
fn native_bridge_key(state: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(state.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("pnex:oauth2:native:{hex}")
}

fn native_local_put(state: String, code: String) {
    let mut bridge = NATIVE_BRIDGE.lock().unwrap_or_else(|e| e.into_inner());
    bridge.retain(|_, (_, at)| at.elapsed() < NATIVE_BRIDGE_TTL);
    if bridge.len() >= NATIVE_BRIDGE_MAX {
        // Flooded: drop the oldest entry rather than growing unbounded.
        if let Some(oldest) = bridge
            .iter()
            .min_by_key(|(_, (_, at))| *at)
            .map(|(k, _)| k.clone())
        {
            bridge.remove(&oldest);
        }
    }
    bridge.insert(state, (code, Instant::now()));
}

fn native_local_take(state: &str) -> Option<String> {
    let mut bridge = NATIVE_BRIDGE.lock().unwrap_or_else(|e| e.into_inner());
    let (code, at) = bridge.remove(state)?;
    (at.elapsed() < NATIVE_BRIDGE_TTL).then_some(code)
}

/// Stores `state → code` (Valkey when configured, local map otherwise or
/// on Valkey failure).
pub async fn native_bridge_put(config: &loco_rs::config::Config, state: &str, code: &str) {
    native_bridge_put_with(
        crate::services::shared_valkey::conn(config).await,
        state,
        code,
    )
    .await;
}

/// [`native_bridge_put`] on an explicit connection (tests: one per "pod").
pub async fn native_bridge_put_with(
    conn: Option<redis::aio::ConnectionManager>,
    state: &str,
    code: &str,
) {
    if let Some(mut conn) = conn {
        let cmd = redis::cmd("SET")
            .arg(native_bridge_key(state))
            .arg(code)
            .arg("EX")
            .arg(NATIVE_BRIDGE_TTL.as_secs())
            .clone();
        match tokio::time::timeout(NATIVE_VALKEY_TIMEOUT, cmd.query_async::<()>(&mut conn)).await {
            Ok(Ok(())) => return,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "native bridge: valkey SET failed, local fallback")
            }
            Err(_) => tracing::warn!("native bridge: valkey SET timed out, local fallback"),
        }
    }
    native_local_put(state.to_string(), code.to_string());
}

/// Consumes the code of `state` (single use), `None` while pending.
pub async fn native_bridge_take(config: &loco_rs::config::Config, state: &str) -> Option<String> {
    native_bridge_take_with(crate::services::shared_valkey::conn(config).await, state).await
}

/// [`native_bridge_take`] on an explicit connection (tests: one per "pod").
pub async fn native_bridge_take_with(
    conn: Option<redis::aio::ConnectionManager>,
    state: &str,
) -> Option<String> {
    if let Some(mut conn) = conn {
        let cmd = redis::cmd("GETDEL").arg(native_bridge_key(state)).clone();
        match tokio::time::timeout(
            NATIVE_VALKEY_TIMEOUT,
            cmd.query_async::<Option<String>>(&mut conn),
        )
        .await
        {
            Ok(Ok(Some(code))) => return Some(code),
            Ok(Ok(None)) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "native bridge: valkey GETDEL failed"),
            Err(_) => tracing::warn!("native bridge: valkey GETDEL timed out"),
        }
    }
    // Local entries exist without Valkey, or when a SET fell back locally.
    native_local_take(state)
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct NativeCallbackParams {
    code: String,
    state: String,
}

/// Rauthy callback (`redirect_uri` of the native login): stores
/// `state → code` (5 min TTL, cross-pod) and serves the "return to the app"
/// page.
#[utoipa::path(
    get,
    path = "/api/v1/oauth2/native",
    tag = "oauth2",
    operation_id = "oauth2_native_callback",
    summary = "Native login callback (browser)",
    description = "`redirect_uri` of the native app login: the identity provider sends the browser here with `code` and `state`; the code is stored for 5 minutes under the state (single use) and an HTML page sends the user back to the app. No authentication (the random state is the capability).",
    params(NativeCallbackParams),
    responses(
        (status = 200, description = "Return-to-app HTML page", content_type = "text/html", body = String),
        (status = 400, description = "`field-invalid`: empty or oversized `state` or `code`", body = crate::openapi::ErrorDetail)
    ),
    security(())
)]
async fn native_callback(
    State(ctx): State<AppContext>,
    Query(params): Query<NativeCallbackParams>,
    headers: axum::http::HeaderMap,
) -> Result<Response> {
    if params.state.is_empty()
        || params.state.len() > NATIVE_STATE_MAX_LEN
        || params.code.is_empty()
        || params.code.len() > NATIVE_CODE_MAX_LEN
    {
        let field = if params.state.is_empty() || params.state.len() > NATIVE_STATE_MAX_LEN {
            "state"
        } else {
            "code"
        };
        return Ok(crate::controllers::field_error(field, FIELD_INVALID));
    }
    native_bridge_put(&ctx.config, &params.state, &params.code).await;
    Ok(axum::response::Html(native_bridge_page(preferred_language(&headers))).into_response())
}

/// Language of the bridge page: first supported tag (fr, en) in
/// Accept-Language — the device browser reflects the phone's language.
/// Declaration order taken as preference (q-values ignored: two
/// supported languages, real browsers already sort by preference).
fn preferred_language(headers: &axum::http::HeaderMap) -> &'static str {
    let Some(raw) = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
    else {
        return "en";
    };
    for part in raw.split(',') {
        let tag = part
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if tag.starts_with("fr") {
            return "fr";
        }
        if tag.starts_with("en") {
            return "en";
        }
    }
    "en"
}

/// "Go back to the app" page, unified in ONE language (the initial HTML
/// mixed fr + en). Back to the app (finding 2026-09-09:
/// `window.close()` closes the tab but the browser stays in the foreground;
/// `intent://…action=MAIN` URIs are rejected by Chromium — "Élément
/// introuvable" under Brave (quoted UI text, english-guard: allow) — because a target without the BROWSABLE category
/// cannot be triggered from the web):
/// 1. navigation to `pnex://return` — deep link scheme declared by the APK
///    (VIEW/BROWSABLE intent-filter injected by
///    `crates/pnex-frontend/patch-android-manifest.py`): the browser
///    resolves the link, Android brings the app to the front; the code was already
///    handed to the app by polling, the page only serves that purpose;
/// 2. best-effort `window.close()` (the tab was opened by the app via
///    Intent — Chromium allows closing it);
/// 3. if the browser blocks automatic navigation (it sometimes requires
///    a gesture), the manual button redoes both with the user gesture.
fn native_bridge_page(lang: &'static str) -> String {
    let (title, subtitle, back) = match lang {
        "fr" => (
            "Connexion effectuée ✓",
            "La connexion est terminée — retour automatique à l'application…",
            "Retourner à l'application",
        ),
        _ => (
            "Signed in ✓",
            "Sign-in complete — returning to the PNeX app…",
            "Return to the PNeX app",
        ),
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="{lang}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>PNeX</title>
</head>
<body style="margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;background:#040d10;color:#e6f1f2;font-family:system-ui,sans-serif">
  <div style="text-align:center;padding:2rem">
    <p style="font-size:22px;margin:0 0 .5rem">{title}</p>
    <p style="opacity:.7;font-size:14px;margin:0">{subtitle}</p>
    <button id="back" onclick="toApp();window.close()" style="display:none;margin-top:1.5rem;padding:.6rem 1.2rem;border:0;border-radius:8px;background:#2563eb;color:#fff;font-size:14px;cursor:pointer">{back}</button>
  </div>
  <script>
    // Deep link of the app (`pnex` scheme registered by
    // patch-android-manifest.py, independent of the package name).
    var APP = 'pnex://return';
    function toApp() {{ window.location = APP; }}
    setTimeout(toApp, 1000);
    setTimeout(function () {{ window.close(); }}, 2000);
    setTimeout(function () {{ document.getElementById('back').style.display = 'inline-block'; }}, 3500);
  </script>
</body>
</html>"#
    )
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
struct NativePoll {
    /// Authorization code once the browser came back, `null` meanwhile.
    code: Option<String>,
}

/// App poll: the code once Rauthy came back, `null` meanwhile.
/// The code is handed out once (consumed): the app stops polling as soon as
/// it gets it.
#[utoipa::path(
    get,
    path = "/api/v1/oauth2/native/{state}",
    tag = "oauth2",
    operation_id = "oauth2_native_poll",
    summary = "Poll the native login code",
    description = "Polled by the native app: returns the authorization code stored for `state` once (consumed on read), `null` while pending; the app then exchanges it with its PKCE verifier at `/api/v1/oauth2/token`. No authentication (the random state is the capability).",
    params(("state" = String, Path, description = "PKCE flow state chosen by the app")),
    responses(
        (status = 200, body = NativePoll)
    ),
    security(())
)]
async fn native_poll(State(ctx): State<AppContext>, Path(state): Path<String>) -> Result<Response> {
    let code = if state.is_empty() || state.len() > NATIVE_STATE_MAX_LEN {
        None
    } else {
        native_bridge_take(&ctx.config, &state).await
    };
    Ok(axum::Json(NativePoll { code }).into_response())
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(token, refresh, sso, logout, native_callback, native_poll))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    fn hdr(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            header::ACCEPT_LANGUAGE,
            HeaderValue::from_str(value).unwrap(),
        );
        h
    }

    #[tokio::test]
    async fn native_bridge_local_fallback_is_single_use() {
        native_bridge_put_with(None, "state-local-1", "code-1").await;
        assert_eq!(
            native_bridge_take_with(None, "state-local-1")
                .await
                .as_deref(),
            Some("code-1")
        );
        assert_eq!(native_bridge_take_with(None, "state-local-1").await, None);
        assert_eq!(native_bridge_take_with(None, "never-set").await, None);
    }

    #[test]
    fn langue_de_la_page_pont() {
        assert_eq!(preferred_language(&hdr("fr-FR,fr;q=0.9,en;q=0.8")), "fr");
        assert_eq!(preferred_language(&hdr("en-US,en;q=0.9")), "en");
        assert_eq!(preferred_language(&hdr("de-DE,de;q=0.9,fr;q=0.2")), "fr");
        assert_eq!(preferred_language(&hdr("zh-CN,zh;q=0.9")), "en");
        assert_eq!(preferred_language(&HeaderMap::new()), "en");
    }
}
