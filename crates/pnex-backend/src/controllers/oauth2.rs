//! OAuth2 proxy to Rauthy (full-Rust OIDC IdP, D10):
//!
//! - `POST /api/v1/oauth2/token` : grants `password` (dev/tests) et
//!   `authorization_code` + PKCE ; les erreurs Rauthy sont relayées telles
//!   quelles (400 `{"error": ...}`) ;
//! - `POST /api/v1/oauth2/refresh` : `grant_type=refresh_token` ;
//! - `GET /api/v1/oauth2/sso` : 302 vers l'authorize endpoint Rauthy, PKCE
//!   S256 obligatoire ; `action=register` → page UI d'inscription Rauthy
//!   (activation par mail), `action=reset` → page compte Rauthy ;
//! - `GET /api/v1/oauth2/logout` : 302 vers `/auth/v1/oidc/logout`
//!   (RP-initiated logout, `id_token_hint` + `post_logout_redirect_uri`).
//!
//! Le client reste public (pas de secret côté navigateur) : PKCE suffit.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::auth::settings::RauthySettings;

fn http() -> &'static reqwest::Client {
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            // Rauthy refuse les requêtes sans User-Agent (400).
            .user_agent("pnex-server")
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client")
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantKind {
    Password,
    AuthorizationCode,
    RefreshToken,
}

/// Body accepté pour `/token` et `/refresh` — les champs dépendent du grant.
#[derive(Deserialize, Default)]
pub struct TokenParams {
    pub grant_type: Option<GrantKind>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub code: Option<String>,
    pub code_verifier: Option<String>,
    pub redirect_uri: Option<String>,
    pub refresh_token: Option<String>,
}

/// Relaye la réponse Rauthy (statut + JSON) telle quelle.
async fn relay(response: reqwest::Response) -> Result<Response> {
    let status = StatusCode::from_u16(response.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body: serde_json::Value = response.json().await.unwrap_or(serde_json::json!({
        "error": "invalid_response",
        "error_description": "Rauthy a renvoyé une réponse illisible",
    }));
    let mut response = axum::Json(body).into_response();
    *response.status_mut() = status;
    Ok(response)
}

async fn token(State(ctx): State<AppContext>, Json(params): Json<TokenParams>) -> Result<Response> {
    let settings = RauthySettings::from_config(&ctx.config)?;

    // Scope standard (comme le flow SSO) : garantit l'émission de l'id_token,
    // requis pour l'end-session (`id_token_hint`).
    let mut form: Vec<(&str, String)> = vec![
        ("client_id", settings.client_id.clone()),
        ("scope", "openid profile email".into()),
    ];
    match params.grant_type {
        Some(GrantKind::Password) => {
            let (Some(username), Some(password)) = (params.username, params.password) else {
                return Err(Error::BadRequest(
                    "grant password : username et password requis".into(),
                ));
            };
            form.push(("grant_type", "password".into()));
            form.push(("username", username));
            form.push(("password", password));
        }
        Some(GrantKind::AuthorizationCode) => {
            let (Some(code), Some(code_verifier), Some(redirect_uri)) =
                (params.code, params.code_verifier, params.redirect_uri)
            else {
                return Err(Error::BadRequest(
                    "grant authorization_code : code, code_verifier et redirect_uri requis".into(),
                ));
            };
            form.push(("grant_type", "authorization_code".into()));
            form.push(("code", code));
            form.push(("code_verifier", code_verifier));
            form.push(("redirect_uri", redirect_uri));
        }
        Some(GrantKind::RefreshToken) | None => {
            let Some(refresh_token) = params.refresh_token else {
                return Err(Error::BadRequest("refresh_token requis".into()));
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
            tracing::error!(%err, "Rauthy injoignable (token)");
            Error::CustomError(
                StatusCode::BAD_GATEWAY,
                loco_rs::controller::ErrorDetail::new("upstream", "Rauthy injoignable".to_string()),
            )
        })?;
    relay(upstream).await
}

async fn refresh(
    State(ctx): State<AppContext>,
    Json(body): Json<serde_json::Value>,
) -> Result<Response> {
    let Some(refresh_token) = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        return Err(Error::BadRequest("refresh_token requis".into()));
    };
    let params = TokenParams {
        grant_type: Some(GrantKind::RefreshToken),
        refresh_token: Some(refresh_token),
        ..Default::default()
    };
    token(State(ctx), Json(params)).await
}

#[derive(Deserialize)]
pub struct LogoutParams {
    /// Retour après déconnexion (origine de l'app, ex. `http://localhost:5150/`).
    /// Validé par Rauthy contre la liste autorisée du client — jamais ici.
    pub post_logout_redirect_uri: Option<String>,
    /// `id_token_hint` — optionnel : sans lui, Rauthy retombe sur la session
    /// cookie (fallback interne `find_session_with_user_fallback`).
    pub id_token: Option<String>,
}

/// `GET /api/v1/oauth2/logout` — sert un formulaire HTML auto-soumis vers
/// l'end-session Rauthy. Ce POST est une navigation TOP-LEVEL : le 302 final
/// de Rauthy vers `post_logout_redirect_uri` devient une vraie navigation et
/// le navigateur atterrit directement sur l'app (boot déconnecté → écran de
/// login). C'est le seul chemin qui redirige réellement : la page logout SPA
/// de Rauthy POSTe en `fetch` puis rejoint toujours sa landing hardcodée
/// `/auth/v1/` — jamais l'app (vérifié dans le source 0.36.2).
async fn logout(
    State(ctx): State<AppContext>,
    Query(params): Query<LogoutParams>,
) -> Result<Response> {
    let settings = RauthySettings::from_config(&ctx.config)?;
    let post_logout_redirect_uri = params
        .post_logout_redirect_uri
        .unwrap_or_else(|| format!("{}/auth/v1/", settings.base_url));

    // Échappement HTML minimal (l'id_token est base64url + points, l'URI
    // vient du front — défense en profondeur contre une injection d'attribut).
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

#[derive(Deserialize)]
pub struct SsoParams {
    /// `register` | `reset` (absent = simple login).
    pub action: Option<String>,
    /// PKCE obligatoire.
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub redirect_uri: Option<String>,
    /// Corrélation du callback natif (pont backend, cf. `native_callback`) —
    /// relayé à Rauthy qui le renvoie tels quels au redirect_uri.
    pub state: Option<String>,
}

/// Encode une paire clé/valeur en query string (application/x-www-form-urlencoded).
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

async fn sso(
    State(ctx): State<AppContext>,
    Query(params): Query<SsoParams>,
    request: axum::extract::Request,
) -> Result<Response> {
    let settings = RauthySettings::from_config(&ctx.config)?;

    let Some(code_challenge) = params.code_challenge else {
        return Err(Error::BadRequest(
            "code_challenge requis (PKCE obligatoire)".into(),
        ));
    };
    let method = params
        .code_challenge_method
        .unwrap_or_else(|| "S256".into());
    if method != "S256" {
        return Err(Error::BadRequest(
            "code_challenge_method doit être S256".into(),
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
    // Bind dev « toutes interfaces » : une redirect_uri en 0.0.0.0/[::] n'est
    // pas autorisée côté Rauthy — normalisation vers la forme loopback.
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
    // Pages UI Rauthy (pas des endpoints OIDC) : `action=register` →
    // formulaire d'inscription (activation par mail), `action=reset` →
    // page compte où vit le changement de mot de passe. Dans les deux cas,
    // l'utilisateur revient à l'app pnex et (re)lance le login.
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
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&location)
            .map_err(|_| Error::BadRequest("redirect_uri ou action invalide".into()))?,
    );
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

// ===== Pont de callback natif =====
//
// dioxus-desktop ouvre toute navigation http(s) dans le navigateur système
// (`webbrowser::open` codé en dur dans son wrapper de navigation) : la webview
// embarquée est impossible sans fork. Le login natif ouvre donc le navigateur
// sur le flow SSO avec redirect_uri = `/api/v1/oauth2/native` (cette page) ;
// Rauthy y rapatrie `?code&state`, et l'app — restée en arrière-plan —
// récupère le code par polling de `/native/{state}`, l'échange contre des
// tokens (le verifier PKCE ne quitte jamais l'app) et ouvre la session :
// l'utilisateur retrouve l'app déjà connectée au retour.
//
// Le `state` aléatoire (challenge PKCE, unique par flow) est la seule clé
// d'accès au code : c'est le secret de capacité du pont — il ne transite que
// via le navigateur de l'utilisateur.

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

#[derive(Debug, Deserialize)]
struct NativeCallbackParams {
    code: String,
    state: String,
}

/// Rauthy callback (`redirect_uri` of the native login): stores
/// `state → code` (5 min TTL, cross-pod) and serves the "return to the app"
/// page.
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
        return Err(Error::BadRequest("invalid state or code".into()));
    }
    native_bridge_put(&ctx.config, &params.state, &params.code).await;
    Ok(axum::response::Html(native_bridge_page(preferred_language(&headers))).into_response())
}

/// Langue de la page pont : premier tag supporté (fr, en) dans
/// Accept-Language — le navigateur du device reflète la langue du téléphone.
/// Ordre de déclaration pris pour préférence (q-values ignorées : deux
/// langues supportées, les navigateurs réels trient déjà par préférence).
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

/// Page « retournez dans l'application », unifiée dans UNE langue (le HTML
/// initial mélangeait fr + en). Retour à l'app (constat 2026-09-09 :
/// `window.close()` ferme l'onglet mais le navigateur reste au premier plan ;
/// les URI `intent://…action=MAIN` sont refusées par Chromium — « Élément
/// introuvable » sous Brave — car une cible sans catégorie BROWSABLE n'est
/// pas déclenchable depuis le web) :
/// 1. navigation vers `pnex://return` — schéma deep link déclaré par l'APK
///    (intent-filter VIEW/BROWSABLE injecté par
///    `crates/pnex-frontend/patch-android-manifest.py`) : le navigateur
///    résout le lien, Android met l'app devant ; le code a déjà été passé à
///    l'app par le polling, la page ne sert plus qu'à ça ;
/// 2. `window.close()` en best-effort (l'onglet a été ouvert par l'app via
///    Intent — Chromium autorise sa fermeture) ;
/// 3. si le navigateur bloque la navigation automatique (elle exige parfois
///    un geste), le bouton manuel refait les deux avec le geste utilisateur.
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

#[derive(Debug, Serialize)]
struct NativePoll {
    code: Option<String>,
}

/// App poll: the code once Rauthy came back, `null` meanwhile.
/// The code is handed out once (consumed): the app stops polling as soon as
/// it gets it.
async fn native_poll(State(ctx): State<AppContext>, Path(state): Path<String>) -> Result<Response> {
    let code = if state.is_empty() || state.len() > NATIVE_STATE_MAX_LEN {
        None
    } else {
        native_bridge_take(&ctx.config, &state).await
    };
    Ok(axum::Json(NativePoll { code }).into_response())
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
