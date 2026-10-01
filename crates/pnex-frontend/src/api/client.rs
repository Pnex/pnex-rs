//! Client HTTP partagé — porté du `ApiService` React (`api.ts`).
//!
//! - URLs relatives (same-origin, cf. `config::api_base`) ;
//! - `Authorization: Bearer` + `X-Org-Id` (jamais sur `/oauth2/*` ni
//!   `/api/v1/public/*`) lus depuis
//!   le stockage à chaque requête (le stockage est la source de vérité) ;
//! - sur 401 : refresh du token **single-flight** (une seule requête de
//!   refresh, les appelants en attente partagent le même futur — parité
//!   `tokenRefreshPromise` React), puis **une** retry de la requête ;
//! - refresh impossible/échoué → session expirée (purge + signal) ;
//! - error messages: server body kept + machine code; local errors carry
//!   an `err_codes` code + English fallback, i18n-resolved at render time;
//! - 204 / corps vide → `None`.
//!
//! Le client vit en `thread_local` : sur wasm le runtime est mono-thread et
//! les futurs reqwest sont `!Send` ; sur natif l'UI reste sur son thread.
//! (La future cible desktop devra rendre ces futurs `Send` — noté dans
//! docs/architecture/features.md.)

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;

use futures::FutureExt;
use serde::de::DeserializeOwned;

use crate::api::error::ApiError;
use crate::storage::{self, KeyValueStorage, KEY_ACCESS_TOKEN, KEY_REFRESH_TOKEN};
use pnex_core::err_codes;

const OAUTH_PREFIX: &str = "/api/v1/oauth2";
/// Endpoints publics (partage de tour) : **jamais** de Bearer/X-Org-Id —
/// le token du lien est la seule clé ; et jamais de refresh 401 dessus.
const PUBLIC_PREFIX: &str = "/api/v1/public";

/// Chemin exempté d'authentification (oauth2 + public).
fn is_auth_exempt(path: &str) -> bool {
    path.starts_with(OAUTH_PREFIX) || path.starts_with(PUBLIC_PREFIX)
}

type RefreshFuture =
    futures::future::Shared<Pin<Box<dyn Future<Output = Result<(), ApiError>> + 'static>>>;

thread_local! {
    /// Shared client, keyed by the pinned CA it was built with (`api::tls`):
    /// pinning a new CA rebuilds it on the next request.
    static HTTP: RefCell<Option<(Option<String>, reqwest::Client)>> = const { RefCell::new(None) };
    static REFRESH_SLOT: RefCell<Option<RefreshFuture>> = const { RefCell::new(None) };
}

/// Shared client trusting the currently pinned CA (native) — reqwest's
/// client is an internal `Arc`, cloning is cheap.
fn http() -> reqwest::Client {
    let ca = crate::api::tls::pinned_ca();
    HTTP.with(|slot| {
        let mut slot = slot.borrow_mut();
        match slot.as_ref() {
            Some((key, client)) if *key == ca => client.clone(),
            _ => {
                let client = crate::api::tls::client_with_ca(ca.as_deref(), None);
                *slot = Some((ca, client.clone()));
                client
            }
        }
    })
}

/// Requête JSON authentifiée ; 204/vide autorisé (→ None).
pub async fn request_opt<T: DeserializeOwned>(
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<Option<T>, ApiError> {
    let mut response = send(method.clone(), path, body.clone()).await?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED && !is_auth_exempt(path) {
        ensure_refresh().await?;
        response = send(method, path, body).await?;
    }
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    if (200..300).contains(&status) {
        if status == 204 || text.trim().is_empty() {
            return Ok(None);
        }
        serde_json::from_str(&text).map(Some).map_err(|err| {
            ApiError::local(
                err_codes::CLIENT_UNREADABLE_BODY,
                format!("unreadable response: {err}"),
                Some(serde_json::json!({ "msg": err.to_string() })),
            )
        })
    } else {
        Err(ApiError::http(status, &text))
    }
}

/// Requête JSON authentifiée attendant un corps.
pub async fn request<T: DeserializeOwned>(
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<T, ApiError> {
    request_opt(method, path, body).await?.ok_or_else(|| {
        ApiError::local(
            err_codes::CLIENT_EMPTY_BODY,
            "unexpected empty response",
            None,
        )
    })
}

/// Requête authentifiée attendant des **octets** (téléchargement de
/// firmware) — même mécanique de refresh 401 que `request_opt`.
pub async fn request_bytes(method: reqwest::Method, path: &str) -> Result<Vec<u8>, ApiError> {
    let mut response = send(method.clone(), path, None).await?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED && !is_auth_exempt(path) {
        ensure_refresh().await?;
        response = send(method, path, None).await?;
    }
    let status = response.status().as_u16();
    let bytes = response.bytes().await.unwrap_or_default();
    if (200..300).contains(&status) {
        Ok(bytes.to_vec())
    } else {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        Err(ApiError::http(status, &text))
    }
}

async fn send(
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<reqwest::Response, ApiError> {
    let url = format!("{}{}", crate::api::config::api_base(), path);
    let mut req = http().request(method, &url);
    if !is_auth_exempt(path) {
        if let Some(token) = storage::local().get(KEY_ACCESS_TOKEN) {
            req = req.bearer_auth(token);
        }
        // Le signal ORG est le miroir réactif du tenant courant (écrit par le
        // sélecteur d'org, persisté par le même chemin).
        if let Some(org) = crate::state::org::current() {
            req = req.header("X-Org-Id", org.to_string());
        }
    }
    if let Some(body) = body {
        req = req.json(&body);
    }
    req.send().await.map_err(|err| ApiError::network(&err))
}

/// Variante de [`send`] avec un corps d'**octets bruts** (octet-stream) —
/// upload média (D21) ; pas de feature multipart requise (marche identique
/// wasm + natif).
async fn send_bytes(
    method: reqwest::Method,
    path: &str,
    body: &[u8],
) -> Result<reqwest::Response, ApiError> {
    let url = format!("{}{}", crate::api::config::api_base(), path);
    let mut req = http().request(method, &url);
    if !is_auth_exempt(path) {
        if let Some(token) = storage::local().get(KEY_ACCESS_TOKEN) {
            req = req.bearer_auth(token);
        }
        if let Some(org) = crate::state::org::current() {
            req = req.header("X-Org-Id", org.to_string());
        }
    }
    req = req.header("Content-Type", "application/octet-stream");
    req = req.body(body.to_vec());
    req.send().await.map_err(|err| ApiError::network(&err))
}

/// Requête authentifiée avec un **corps d'octets** (upload média) attendant
/// une réponse JSON — même mécanique de refresh 401 que `request_opt` ; le
/// corps est ré-emprunté pour la retry après refresh.
pub async fn request_upload<T: DeserializeOwned>(
    method: reqwest::Method,
    path: &str,
    bytes: Vec<u8>,
) -> Result<T, ApiError> {
    let mut response = send_bytes(method.clone(), path, &bytes).await?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED && !is_auth_exempt(path) {
        ensure_refresh().await?;
        response = send_bytes(method, path, &bytes).await?;
    }
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    if (200..300).contains(&status) {
        serde_json::from_str(&text).map_err(|err| {
            ApiError::local(
                err_codes::CLIENT_UNREADABLE_BODY,
                format!("unreadable response: {err}"),
                Some(serde_json::json!({ "msg": err.to_string() })),
            )
        })
    } else {
        Err(ApiError::http(status, &text))
    }
}

/// Refresh single-flight : un seul appel IdP à la fois, partagé entre
/// tous les appelants 401 concurrents. Échec → session expirée.
async fn ensure_refresh() -> Result<(), ApiError> {
    // Aucun await entre l'emprunt et l'insertion → pas de course (mono-thread
    // d'événements entre deux awaits).
    let running = REFRESH_SLOT.with(|slot| slot.borrow().clone());
    let future = match running {
        Some(running) => running,
        None => {
            let future: RefreshFuture = async {
                let Some(refresh_token) = storage::local().get(KEY_REFRESH_TOKEN) else {
                    crate::state::session::expire();
                    return Err(ApiError::local(
                        err_codes::CLIENT_SESSION_EXPIRED,
                        "session expired",
                        None,
                    ));
                };
                match crate::api::auth::refresh_tokens(&refresh_token).await {
                    Ok(tokens) => {
                        crate::api::auth::store_tokens(&tokens);
                        Ok(())
                    }
                    Err(err) => {
                        crate::state::session::expire();
                        Err(err)
                    }
                }
            }
            .boxed_local()
            .shared();
            REFRESH_SLOT.with(|slot| *slot.borrow_mut() = Some(future.clone()));
            future
        }
    };
    let outcome = future.await;
    REFRESH_SLOT.with(|slot| *slot.borrow_mut() = None);
    outcome
}
