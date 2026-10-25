//! HTTP client of a PneX service account: `Authorization: Bearer
//! pnex_sk_…`, optional `X-Org-Id`, a few retries on network errors and
//! server errors, never on a refusal.

use std::time::Duration;

use reqwest::{header, Method, StatusCode};
use serde::de::DeserializeOwned;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The server refused (4xx) or kept failing (5xx), with its body.
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error(transparent)]
    Network(#[from] reqwest::Error),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// A downloaded file whose SHA-256 is not the one PneX announced.
    #[error("media {asset_id}: SHA-256 mismatch")]
    Checksum { asset_id: String },
}

impl Error {
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Http { status, .. } => Some(*status),
            Self::Network(e) => e.status().map(|s| s.as_u16()),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

const TRIES: u32 = 4;

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    key: String,
    org: Option<i64>,
}

/// A body answer: the bytes and its `ETag`, or nothing new (304).
pub enum Fetched {
    Body {
        bytes: Vec<u8>,
        etag: Option<String>,
    },
    NotModified,
}

impl Client {
    /// `base` = the server root (`https://pnex.example`), `key` = the
    /// service account's access key.
    pub fn new(base: &str, key: &str, org: Option<i64>) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("pnex-mirror/", env!("CARGO_PKG_VERSION")))
                .timeout(Duration::from_secs(120))
                .build()?,
            base: base.trim_end_matches('/').to_string(),
            key: key.to_string(),
            org,
        })
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let mut r = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.key);
        if let Some(o) = self.org {
            r = r.header("X-Org-Id", o.to_string());
        }
        r
    }

    /// `GET path` (path and query, from `/api/v1/…`) as JSON.
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let bytes = match self.get(path, None).await? {
            Fetched::Body { bytes, .. } => bytes,
            Fetched::NotModified => Vec::new(),
        };
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// `GET path`, `If-None-Match: etag` when given.
    pub async fn get(&self, path: &str, etag: Option<&str>) -> Result<Fetched> {
        let mut wait = Duration::from_millis(300);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut r = self.request(Method::GET, path);
            if let Some(e) = etag {
                r = r.header(header::IF_NONE_MATCH, e);
            }
            let e = match r.send().await {
                Ok(res) if res.status() == StatusCode::NOT_MODIFIED => {
                    return Ok(Fetched::NotModified)
                }
                Ok(res) if res.status().is_success() => {
                    let etag = res
                        .headers()
                        .get(header::ETAG)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    return Ok(Fetched::Body {
                        bytes: res.bytes().await?.to_vec(),
                        etag,
                    });
                }
                Ok(res) => {
                    let status = res.status().as_u16();
                    let body = res.text().await.unwrap_or_default();
                    Error::Http { status, body }
                }
                Err(e) => Error::Network(e),
            };
            let retry = match &e {
                Error::Http { status, .. } => *status >= 500,
                _ => true,
            };
            if !retry || attempt >= TRIES {
                return Err(e);
            }
            tokio::time::sleep(wait).await;
            wait *= 2;
        }
    }
}
