//! Client HTTP partagé — porté du `ApiService` React (`api.ts`).
//!
//! - URLs relatives (same-origin, cf. `config::api_base`) ;
//! - `Authorization: Bearer` + `X-Org-Id` (jamais sur `/oauth2/*`) lus depuis
//!   le stockage à chaque requête (le stockage est la source de vérité) ;
//! - sur 401 : refresh du token **single-flight** (une seule requête de
//!   refresh, les appelants en attente partagent le même futur — parité
//!   `tokenRefreshPromise` React), puis **une** retry de la requête ;
//! - refresh impossible/échoué → session expirée (purge + signal) ;
//! - messages d'erreur extraits du corps et renvoyés tels quels ;
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

const OAUTH_PREFIX: &str = "/api/v1/oauth2";

type RefreshFuture = futures::future::Shared<Pin<Box<dyn Future<Output = Result<(), ApiError>> + 'static>>>;

thread_local! {
    static HTTP: reqwest::Client = reqwest::Client::new();
    static REFRESH_SLOT: RefCell<Option<RefreshFuture>> = const { RefCell::new(None) };
}

/// Requête JSON authentifiée ; 204/vide autorisé (→ None).
pub async fn request_opt<T: DeserializeOwned>(
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<Option<T>, ApiError> {
    let mut response = send(method.clone(), path, body.clone()).await?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED && !path.starts_with(OAUTH_PREFIX) {
        ensure_refresh().await?;
        response = send(method, path, body).await?;
    }
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    if (200..300).contains(&status) {
        if status == 204 || text.trim().is_empty() {
            return Ok(None);
        }
        serde_json::from_str(&text).map(Some).map_err(|err| ApiError {
            message: format!("réponse illisible : {err}"),
        })
    } else {
        Err(ApiError {
            message: crate::api::error::extract_message(status, &text),
        })
    }
}

/// Requête JSON authentifiée attendant un corps.
pub async fn request<T: DeserializeOwned>(
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<T, ApiError> {
    request_opt(method, path, body)
        .await?
        .ok_or_else(|| ApiError {
            message: "réponse vide inattendue".into(),
        })
}

async fn send(
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<reqwest::Response, ApiError> {
    let url = format!("{}{}", crate::api::config::api_base(), path);
    let mut req = HTTP.with(|c| c.clone()).request(method, &url);
    if let Some(token) = storage::local().get(KEY_ACCESS_TOKEN) {
        req = req.bearer_auth(token);
    }
    if !path.starts_with(OAUTH_PREFIX) {
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

/// Refresh single-flight : un seul appel Keycloak à la fois, partagé entre
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
                    return Err(ApiError {
                        message: "session expirée".into(),
                    });
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
