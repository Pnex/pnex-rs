//! Fetching an import source (D301): `http(s)` only, behind the egress
//! guard of `http_fetch` (R8: no loopback, link-local or cloud metadata,
//! every redirect re-checked), size and time capped.

use std::time::Duration;

use pnex_core::import_read::{read_records, ReadError, UNZIPPED_MAX};
use pnex_core::ontology::import::{get_path, json_rows, Mapping, ROWS_MAX};
use serde_json::Value;

/// Machine codes of a source that cannot be read (`err_codes`).
pub use pnex_core::err_codes::{
    IMPORT_SOURCE_TOO_LARGE as TOO_LARGE, IMPORT_SOURCE_UNREACHABLE as UNREACHABLE,
    IMPORT_SOURCE_UNREADABLE as UNREADABLE,
};

const TIMEOUT: Duration = Duration::from_secs(120);

fn http() -> &'static reqwest::Client {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP.get_or_init(|| {
        pnex_core::egress::guarded(reqwest::Client::builder())
            .redirect(pnex_core::egress::redirect_policy(5))
            .timeout(TIMEOUT)
            .user_agent(concat!("PNEX-Import/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client")
    })
}

/// A URL an import may fetch: `http(s)`, host allowed by the policy.
pub fn check_url(raw: &str) -> Result<reqwest::Url, &'static str> {
    let u = reqwest::Url::parse(raw.trim()).map_err(|_| "invalid")?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
        return Err("invalid");
    }
    match pnex_core::egress::check_url(&u) {
        Some(code) => Err(code),
        None => Ok(u),
    }
}

pub fn read_error(e: ReadError) -> &'static str {
    match e {
        ReadError::TooLarge => TOO_LARGE,
        ReadError::Malformed | ReadError::UnknownFormat => UNREADABLE,
    }
}

async fn get(url: &reqwest::Url) -> Result<Vec<u8>, &'static str> {
    let mut resp = http().get(url.clone()).send().await.map_err(|e| {
        tracing::info!(url = %url, error = %e, "import source unreachable");
        UNREACHABLE
    })?;
    if !resp.status().is_success() {
        return Err(UNREACHABLE);
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|_| UNREACHABLE)? {
        if (body.len() + chunk.len()) as u64 > UNZIPPED_MAX {
            return Err(TOO_LARGE);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Records of a file at `url`.
pub async fn fetch_file(url: &str, m: &Mapping) -> Result<Vec<Value>, &'static str> {
    let u = check_url(url).map_err(|_| UNREACHABLE)?;
    let bytes = get(&u).await?;
    let (name, m) = (u.path().to_string(), m.clone());
    tokio::task::spawn_blocking(move || read_records(&bytes, &name, &m).map_err(read_error))
        .await
        .unwrap_or(Err(UNREADABLE))
}

/// Placeholder of the per-object value in a source URL.
pub const VALUE: &str = "{value}";

/// An identifier safe to splice into a URL path or a query text (SIREN,
/// LEI, Wikidata id): letters, digits, `-`, `_`, `.`.
fn plain(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Records of a JSON API queried once per value of `per_object` (a
/// request that fails counts as no record: one unknown identifier must
/// not stop the run).
pub async fn fetch_per_object(
    url: &str,
    m: &Mapping,
    values: &[String],
) -> Result<Vec<Value>, &'static str> {
    let base = check_url(url).map_err(|_| UNREACHABLE)?;
    let Some(p) = &m.per_object else {
        return Ok(Vec::new());
    };
    let mut rows = Vec::new();
    let mut failures = 0usize;
    for v in values {
        let u = if p.param.is_empty() {
            // Inside a path or a query text: only plain identifiers go in
            // (unreserved characters, nothing to encode).
            if !plain(v) {
                failures += 1;
                continue;
            }
            check_url(&url.replace(VALUE, v)).map_err(|_| UNREACHABLE)?
        } else {
            let mut u = base.clone();
            u.query_pairs_mut().append_pair(&p.param, v);
            u
        };
        match get(&u).await {
            Ok(bytes) => {
                let doc: Value = serde_json::from_slice(&bytes).map_err(|_| UNREADABLE)?;
                rows.extend(json_rows(&doc, &m.rows).ok_or(UNREADABLE)?);
            }
            Err(TOO_LARGE) => return Err(TOO_LARGE),
            Err(_) => failures += 1,
        }
        if rows.len() > ROWS_MAX {
            return Err(TOO_LARGE);
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    // Every request failed: the API is down, not the identifiers unknown.
    if !values.is_empty() && failures == values.len() {
        return Err(UNREACHABLE);
    }
    Ok(rows)
}

/// Records of a paginated JSON API: `page_param` (1, 2…) until a page is
/// empty, or `cursor_path` → `cursor_param` until the cursor is gone;
/// a single page without either.
pub async fn fetch_api(url: &str, m: &Mapping, max_pages: u32) -> Result<Vec<Value>, &'static str> {
    let base = check_url(url).map_err(|_| UNREACHABLE)?;
    let mut rows = Vec::new();
    let mut cursor: Option<String> = None;
    for page in 1..=max_pages.max(1) {
        let mut u = base.clone();
        match (&m.page_param, &m.cursor_param, &cursor) {
            (Some(p), _, _) => {
                u.query_pairs_mut().append_pair(p, &page.to_string());
            }
            (None, Some(p), Some(c)) => {
                u.query_pairs_mut().append_pair(p, c);
            }
            _ => {}
        }
        let bytes = get(&u).await?;
        let doc: Value = serde_json::from_slice(&bytes).map_err(|_| UNREADABLE)?;
        let page_rows = json_rows(&doc, &m.rows).ok_or(UNREADABLE)?;
        let empty = page_rows.is_empty();
        rows.extend(page_rows);
        if rows.len() > ROWS_MAX {
            return Err(TOO_LARGE);
        }
        cursor = m
            .cursor_path
            .as_deref()
            .and_then(|p| get_path(&doc, p))
            .and_then(|v| match v {
                Value::String(s) if !s.is_empty() => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            });
        let more = match (&m.page_param, &m.cursor_path) {
            (Some(_), _) => !empty,
            (None, Some(_)) => cursor.is_some(),
            _ => false,
        };
        if !more {
            break;
        }
        // Gentle on public APIs (≈ 7 requests/s at most).
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    Ok(rows)
}
