//! A local server standing for the artifact routes of PneX (D744), shared by the tests of
//! the transfer scripts and of the micro-VM executor.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, put};
use sha2::{Digest, Sha256};

pub const TOKEN: &str = "pnart_test-token-value";

#[derive(Default)]
pub struct Server {
    pub inputs: HashMap<String, Vec<u8>>,
    /// name → (form header, body)
    pub outputs: HashMap<String, (String, Vec<u8>)>,
    /// (name, key) → archive
    pub caches: HashMap<(String, String), Vec<u8>>,
}

pub type Shared = Arc<Mutex<Server>>;

fn authorized(headers: &HeaderMap) -> bool {
    headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {TOKEN}"))
}

pub async fn serve(server: Shared) -> String {
    let app =
        axum::Router::new()
            .route(
                "/inputs/{name}",
                get(
                    |State(s): State<Shared>,
                     headers: HeaderMap,
                     UrlPath(name): UrlPath<String>| async move {
                        if !authorized(&headers) {
                            return Err(StatusCode::UNAUTHORIZED);
                        }
                        s.lock()
                            .unwrap()
                            .inputs
                            .get(&name)
                            .cloned()
                            .ok_or(StatusCode::NOT_FOUND)
                    },
                ),
            )
            .route(
                "/outputs/{name}",
                put(
                    |State(s): State<Shared>,
                     headers: HeaderMap,
                     UrlPath(name): UrlPath<String>,
                     body: Bytes| async move {
                        if !authorized(&headers) {
                            return StatusCode::UNAUTHORIZED;
                        }
                        let form = headers
                            .get("x-pnex-artifact-form")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or_default()
                            .to_string();
                        s.lock()
                            .unwrap()
                            .outputs
                            .insert(name, (form, body.to_vec()));
                        StatusCode::NO_CONTENT
                    },
                ),
            )
            .route(
            "/caches/{name}",
            get(
                |State(s): State<Shared>,
                 headers: HeaderMap,
                 UrlPath(name): UrlPath<String>,
                 axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>| async move {
                    if !authorized(&headers) {
                        return Err(StatusCode::UNAUTHORIZED);
                    }
                    let key = q.get("key").cloned().unwrap_or_default();
                    let s = s.lock().unwrap();
                    match s.caches.get(&(name.clone(), key)) {
                        Some(a) => Ok(([("x-pnex-cache-exact", "true")], a.clone())),
                        None => s
                            .caches
                            .iter()
                            .find(|((n, _), _)| n == &name)
                            .map(|(_, a)| ([("x-pnex-cache-exact", "false")], a.clone()))
                            .ok_or(StatusCode::NOT_FOUND),
                    }
                },
            )
            .put(
                |State(s): State<Shared>,
                 headers: HeaderMap,
                 UrlPath(name): UrlPath<String>,
                 axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
                 body: Bytes| async move {
                    if !authorized(&headers) {
                        return StatusCode::UNAUTHORIZED;
                    }
                    let key = q.get("key").cloned().unwrap_or_default();
                    s.lock().unwrap().caches.insert((name, key), body.to_vec());
                    StatusCode::NO_CONTENT
                },
            ),
        )
        .with_state(server);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

pub fn hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
