//! Shared HTTP client.
//!
//! - relative URLs (same-origin, see `config::api_base`);
//! - `Authorization: Bearer` + `X-Org-Id` (never on `/oauth2/*` nor
//!   `/api/v1/public/*`) read from
//!   storage on every request (storage is the source of truth);
//! - on 401: **single-flight** token refresh (one refresh request, waiting
//!   callers share the same future), then **one** retry of the request;
//! - refresh impossible/failed → session expired (purge + signal);
//! - error messages: server body kept + machine code; local errors carry
//!   an `err_codes` code + English fallback, i18n-resolved at render time;
//! - 204 / empty body → `None`.
//!
//! The client lives in a `thread_local`: on wasm the runtime is single-threaded and
//! reqwest futures are `!Send`; natively the UI stays on its thread.
//! (The future desktop target will have to make these futures `Send` — noted in
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
/// Public endpoints (tour sharing): **never** a Bearer/X-Org-Id —
/// the link token is the only key; and never a 401 refresh on them.
const PUBLIC_PREFIX: &str = "/api/v1/public";

/// Path exempt from authentication (oauth2 + public).
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

/// Authenticated JSON request; 204/empty allowed (→ None).
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

/// Authenticated JSON request expecting a body.
pub async fn request<T: DeserializeOwned>(
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<T, ApiError> {
    match request_opt(method, path, body).await? {
        Some(value) => Ok(value),
        None => from_empty_body(),
    }
}

/// `?key=value&…` of the non-empty list filters (`search`, `label`…),
/// values url-encoded; empty string when no filter is set.
pub fn filter_query(pairs: &[(&str, Option<&str>)]) -> String {
    let parts: Vec<String> = pairs
        .iter()
        .filter_map(|(k, v)| {
            v.map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| format!("{k}={}", crate::api::tours::urlencode(v)))
        })
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

/// Every row of a paginated list (D14 envelope), page after page at the
/// server maximum: for pickers and client-paged lists. A bare `limit=200`
/// is silently clamped to 100 by the server (O64).
pub async fn request_all<T: DeserializeOwned>(path: &str) -> Result<Vec<T>, ApiError> {
    let sep = if path.contains('?') { '&' } else { '?' };
    let mut rows = Vec::new();
    loop {
        let page: pnex_core::Paginated<T> = request(
            reqwest::Method::GET,
            &format!("{path}{sep}limit=100&offset={}", rows.len()),
            None,
        )
        .await?;
        let last = page.next.is_none() || page.results.is_empty();
        rows.extend(page.results);
        let last = last || rows.len() as i64 >= page.count;
        if last {
            return Ok(rows);
        }
    }
}

/// Value of an empty 2xx body: `()` (a 204 DELETE/PATCH) reads it as JSON
/// `null`; any type that needs a real body keeps the empty-body error.
fn from_empty_body<T: DeserializeOwned>() -> Result<T, ApiError> {
    serde_json::from_value(serde_json::Value::Null).map_err(|_| {
        ApiError::local(
            err_codes::CLIENT_EMPTY_BODY,
            "unexpected empty response",
            None,
        )
    })
}

/// Authenticated request expecting **bytes** (firmware
/// download) — same 401 refresh mechanism as `request_opt`.
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

/// Outcome of [`request_bytes_conditional`].
pub enum ConditionalBytes {
    /// 304: the copy named by `if_none_match` is still the current one.
    NotModified,
    /// Full body, with the server validator when it sent one.
    Body {
        bytes: Vec<u8>,
        etag: Option<String>,
    },
}

/// Streamed GET of a byte endpoint (large media): `on_progress(received,
/// total)` after every chunk (`total` = Content-Length when known);
/// `if_none_match` makes it a revalidation of a cached copy (304 →
/// [`ConditionalBytes::NotModified`]). Same auth/refresh as [`request_bytes`].
pub async fn request_bytes_conditional(
    path: &str,
    if_none_match: Option<&str>,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<ConditionalBytes, ApiError> {
    let get = || async {
        let mut req = authed(reqwest::Method::GET, path);
        if let Some(tag) = if_none_match {
            req = req.header(reqwest::header::IF_NONE_MATCH, tag);
        }
        req.send().await.map_err(|err| ApiError::network(&err))
    };
    let mut response = get().await?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED && !is_auth_exempt(path) {
        ensure_refresh().await?;
        response = get().await?;
    }
    let status = response.status().as_u16();
    if status == 304 {
        return Ok(ConditionalBytes::NotModified);
    }
    if !(200..300).contains(&status) {
        let text = response.text().await.unwrap_or_default();
        return Err(ApiError::http(status, &text));
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let total = response.content_length().filter(|n| *n > 0);
    // Capacity capped: a lying Content-Length must not reserve gigabytes.
    let mut bytes = Vec::with_capacity(total.unwrap_or(0).min(512 * 1024 * 1024) as usize);
    on_progress(0, total);
    #[cfg(target_arch = "wasm32")]
    {
        use futures::StreamExt;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| ApiError::network(&err))?;
            bytes.extend_from_slice(&chunk);
            on_progress(bytes.len() as u64, total);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| ApiError::network(&err))?
    {
        bytes.extend_from_slice(&chunk);
        on_progress(bytes.len() as u64, total);
    }
    Ok(ConditionalBytes::Body { bytes, etag })
}

/// Request builder with the API base URL and, outside the auth-exempt
/// routes, the bearer token and the current org.
fn authed(method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
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
    req
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
        // The ORG signal is the reactive mirror of the current tenant (written by the
        // org switcher, persisted through the same path).
        if let Some(org) = crate::state::org::current() {
            req = req.header("X-Org-Id", org.to_string());
        }
    }
    if let Some(body) = body {
        req = req.json(&body);
    }
    let _in_flight = crate::state::ui::InFlight::start();
    req.send().await.map_err(|err| ApiError::network(&err))
}

/// Variant of [`send`] with a **raw bytes** body (octet-stream) —
/// media upload (D21); no multipart feature required (works the same on
/// wasm + native).
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
    let _in_flight = crate::state::ui::InFlight::start();
    req.send().await.map_err(|err| ApiError::network(&err))
}

/// Authenticated request with a **bytes body** (media upload) expecting
/// a JSON response — same 401 refresh mechanism as `request_opt`; the
/// body is borrowed again for the retry after refresh.
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

/// Single-flight refresh: a single IdP call at a time, shared between
/// all concurrent 401 callers. Failure → session expired.
async fn ensure_refresh() -> Result<(), ApiError> {
    // No await between the borrow and the insertion → no race (single-threaded
    // event loop between two awaits).
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A 204 answers a `request::<()>` (deletes); a body-carrying type still
    /// reports the empty response.
    #[test]
    fn empty_body_is_unit_only() {
        assert!(from_empty_body::<()>().is_ok());
        #[derive(Debug, serde::Deserialize)]
        struct Row {
            #[allow(dead_code)]
            id: i64,
        }
        assert!(from_empty_body::<Row>().is_err());
        assert!(from_empty_body::<Vec<Row>>().is_err());
    }
}
