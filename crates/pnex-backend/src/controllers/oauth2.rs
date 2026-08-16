//! Proxy OAuth2 vers Keycloak — parité fonctionnelle du contrat Django
//! (`authent/oauth2_views.py`) :
//!
//! - `POST /api/v1/oauth2/token` : grants `password` (dev/tests) et
//!   `authorization_code` + PKCE ; les erreurs Keycloak sont relayées telles
//!   quelles (400 `{"error": ...}`) ;
//! - `POST /api/v1/oauth2/refresh` : `grant_type=refresh_token` ;
//! - `GET /api/v1/oauth2/sso` : 302 vers l'authorize endpoint Keycloak,
//!   PKCE S256 obligatoire ; `action=register` utilise l'endpoint
//!   registrations dédié, `action=reset` pose `kc_action=UPDATE_PASSWORD` ;
//! - `GET /api/v1/oauth2/logout` : 302 vers l'end-session Keycloak
//!   (RP-initiated logout, `id_token_hint` + `post_logout_redirect_uri`).
//!
//! Le client reste public (pas de secret côté navigateur) : PKCE suffit.

use std::sync::OnceLock;

use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use loco_rs::prelude::*;
use serde::Deserialize;

use crate::auth::settings::KeycloakSettings;

fn http() -> &'static reqwest::Client {
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
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

/// Relaye la réponse Keycloak (statut + JSON) telle quelle.
async fn relay(response: reqwest::Response) -> Result<Response> {
    let status = StatusCode::from_u16(response.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body: serde_json::Value = response.json().await.unwrap_or(serde_json::json!({
        "error": "invalid_response",
        "error_description": "Keycloak a renvoyé une réponse illisible",
    }));
    let mut response = axum::Json(body).into_response();
    *response.status_mut() = status;
    Ok(response)
}

async fn token(State(ctx): State<AppContext>, Json(params): Json<TokenParams>) -> Result<Response> {
    let settings = KeycloakSettings::from_config(&ctx.config)?;

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

    let kc = http()
        .post(settings.token_endpoint())
        .form(&form)
        .send()
        .await
        .map_err(|err| {
            tracing::error!(%err, "Keycloak injoignable (token)");
            Error::CustomError(
                StatusCode::BAD_GATEWAY,
                loco_rs::controller::ErrorDetail::new("upstream", "Keycloak injoignable".to_string()),
            )
        })?;
    relay(kc).await
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
    /// Retour après déconnexion (défaut : origine du requérant).
    pub post_logout_redirect_uri: Option<String>,
    /// `id_token_hint` — identifie la session à détruire sans interaction.
    pub id_token: Option<String>,
}

/// `GET /api/v1/oauth2/logout` — 302 vers l'end-session Keycloak
/// (RP-initiated logout). Le front purge ses tokens AVANT de rediriger :
/// au retour sur `/`, l'app boote déconnectée et le cookie SSO est mort.
async fn logout(
    State(ctx): State<AppContext>,
    Query(params): Query<LogoutParams>,
    request: axum::extract::Request,
) -> Result<Response> {
    let settings = KeycloakSettings::from_config(&ctx.config)?;
    let post_logout_redirect_uri = params.post_logout_redirect_uri.unwrap_or_else(|| {
        let host = request
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("localhost:5150");
        format!("http://{host}/")
    });

    let mut pairs: Vec<(&str, String)> = vec![
        ("client_id", settings.client_id.clone()),
        ("post_logout_redirect_uri", post_logout_redirect_uri),
    ];
    if let Some(id_token) = params.id_token {
        pairs.push(("id_token_hint", id_token));
    }
    let location = format!(
        "{}?{}",
        settings.end_session_endpoint(),
        form_urlencode(&pairs)
    );

    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::FOUND;
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&location)
            .map_err(|_| Error::BadRequest("post_logout_redirect_uri invalide".into()))?,
    );
    Ok(response)
}

#[derive(Deserialize)]
pub struct SsoParams {
    /// `register` | `reset` (absent = simple login).
    pub action: Option<String>,
    /// PKCE obligatoire.
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub redirect_uri: Option<String>,
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
    let settings = KeycloakSettings::from_config(&ctx.config)?;

    let Some(code_challenge) = params.code_challenge else {
        return Err(Error::BadRequest(
            "code_challenge requis (PKCE obligatoire)".into(),
        ));
    };
    let method = params.code_challenge_method.unwrap_or_else(|| "S256".into());
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
        format!("http://{host}/auth/callback")
    });

    let mut pairs: Vec<(&str, String)> = vec![
        ("client_id", settings.client_id.clone()),
        ("response_type", "code".into()),
        ("scope", "openid profile email".into()),
        ("redirect_uri", redirect_uri),
        ("code_challenge", code_challenge),
        ("code_challenge_method", "S256".into()),
    ];
    // `action=register` : endpoint registrations dédié (kc_action=register
    // est ignoré quand une session SSO existe — re-login silencieux au lieu
    // du formulaire d'inscription). `action=reset` : required action
    // UPDATE_PASSWORD après authentification.
    let endpoint = match params.action.as_deref() {
        Some("register") => settings.registration_endpoint(),
        _ => {
            if params.action.as_deref() == Some("reset") {
                pairs.push(("kc_action", "UPDATE_PASSWORD".into()));
            }
            settings.authorize_endpoint()
        }
    };
    let location = format!("{}?{}", endpoint, form_urlencode(&pairs));

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
}
