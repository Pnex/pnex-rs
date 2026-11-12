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

/// The client of a request carrying credentials: it follows no redirect, so
/// a key or a password never reaches another host (R9). reqwest drops only
/// `Authorization` on a redirect, not a source's own header nor a body.
fn http_keyed() -> &'static reqwest::Client {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP.get_or_init(|| {
        pnex_core::egress::guarded(reqwest::Client::builder())
            .redirect(reqwest::redirect::Policy::none())
            .timeout(TIMEOUT)
            .user_agent(concat!("PNEX-Import/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client")
    })
}

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

/// A client for files of several hundred MB (MK22–MK24): no total
/// timeout, only one per connection and per read.
fn http_long() -> &'static reqwest::Client {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP.get_or_init(|| {
        pnex_core::egress::guarded(reqwest::Client::builder())
            .redirect(pnex_core::egress::redirect_policy(5))
            .connect_timeout(std::time::Duration::from_secs(30))
            .read_timeout(std::time::Duration::from_secs(120))
            .user_agent(concat!("PNEX-Import/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client")
    })
}

/// A large file read as it arrives: the response, `None` on a 404. The
/// caller reads its chunks; nothing is held whole.
pub async fn open_large(
    url: &reqwest::Url,
    by: &Declared,
) -> Result<Option<reqwest::Response>, &'static str> {
    let mut req = http_long().get(url.clone());
    if let Some(c) = &by.contact {
        req = req.header(
            reqwest::header::USER_AGENT,
            format!("PNEX-Import/{} {c}", env!("CARGO_PKG_VERSION")),
        );
    }
    let resp = req.send().await.map_err(|e| {
        tracing::info!(url = %url, error = %e.without_url(), "large file unreachable");
        UNREACHABLE
    })?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(UNREACHABLE);
    }
    Ok(Some(resp))
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
    read_refusal(e).0
}

/// The code of a read error and its arguments (`{rows, max}`…).
pub fn read_refusal(e: ReadError) -> (&'static str, Value) {
    match e {
        ReadError::TooLarge(l) => (TOO_LARGE, l.args()),
        ReadError::Malformed | ReadError::UnknownFormat => (UNREADABLE, Value::Null),
    }
}

async fn get(url: &reqwest::Url) -> Result<Vec<u8>, &'static str> {
    get_as(url, &Declared::default()).await
}

/// What a source wants to know of the caller.
#[derive(Default)]
pub struct Declared {
    /// Who answers for these requests, sent in the `User-Agent`.
    pub contact: Option<String>,
    /// A key, sent in `Authorization`: the client drops it on a redirect to
    /// another host, and it is never part of a URL or a log.
    pub authorization: Option<String>,
    /// Other headers of a source's own scheme (UCDP's token header).
    pub headers: Vec<(&'static str, String)>,
    /// A key a source only takes in its query (FRED): added to the request
    /// alone, so the logged and stored URL never carries it.
    pub query_key: Option<(&'static str, String)>,
}

pub async fn get_as(url: &reqwest::Url, by: &Declared) -> Result<Vec<u8>, &'static str> {
    get_found(url, by).await?.ok_or(UNREACHABLE)
}

/// Like [`get_as`], a 404 being `None`: a file the source does not have
/// (a price partition not published) is not a failure.
pub async fn get_found(url: &reqwest::Url, by: &Declared) -> Result<Option<Vec<u8>>, &'static str> {
    let client = if by.authorization.is_some() || !by.headers.is_empty() || by.query_key.is_some() {
        http_keyed()
    } else {
        http()
    };
    let mut target = url.clone();
    if let Some((param, key)) = &by.query_key {
        target.query_pairs_mut().append_pair(param, key);
    }
    let mut req = client.get(target);
    // An invalid header value fails the request, it never goes out raw.
    if let Some(c) = &by.contact {
        req = req.header(
            reqwest::header::USER_AGENT,
            format!("PNEX-Import/{} {c}", env!("CARGO_PKG_VERSION")),
        );
    }
    if let Some(a) = &by.authorization {
        req = req.header(reqwest::header::AUTHORIZATION, a);
    }
    for (name, value) in &by.headers {
        req = req.header(*name, value);
    }
    read_found(url, req).await
}

/// A form sent to `url` the way its page sends it (`fields` read from that
/// page, `referer` the page itself): `None` on a 404.
pub async fn post_form_found(
    url: &reqwest::Url,
    by: &Declared,
    referer: &reqwest::Url,
    fields: &[(String, String)],
) -> Result<Option<Vec<u8>>, &'static str> {
    let mut req = http()
        .post(url.clone())
        .header(reqwest::header::REFERER, referer.as_str())
        .form(fields);
    if let Some(c) = &by.contact {
        req = req.header(
            reqwest::header::USER_AGENT,
            format!("PNEX-Import/{} {c}", env!("CARGO_PKG_VERSION")),
        );
    }
    read_found(url, req).await
}

/// The body of a response, under [`UNZIPPED_MAX`]; `None` on a 404.
async fn read_found(
    url: &reqwest::Url,
    req: reqwest::RequestBuilder,
) -> Result<Option<Vec<u8>>, &'static str> {
    let mut resp = req.send().await.map_err(|e| {
        // The error would print the request URL, key included.
        let e = e.without_url();
        tracing::info!(url = %url, error = %e, "import source unreachable");
        UNREACHABLE
    })?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
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
    Ok(Some(body))
}

/// Records of a file at `url`.
pub async fn fetch_file(url: &str, m: &Mapping, by: &Declared) -> Result<Vec<Value>, &'static str> {
    let u = check_url(url).map_err(|_| UNREACHABLE)?;
    let bytes = get_as(&u, by).await?;
    let (name, m) = (u.path().to_string(), m.clone());
    tokio::task::spawn_blocking(move || read_records(&bytes, &name, &m).map_err(read_error))
        .await
        .unwrap_or(Err(UNREADABLE))
}

/// One slice of the press feed as text; `None` = absent (the manifest of
/// the feed announces files that are not there yet, or never come).
async fn press_file(base: &reqwest::Url, name: &str) -> Result<Option<String>, &'static str> {
    let url = base.join(name).map_err(|_| UNREACHABLE)?;
    let bytes = match get(&url).await {
        Ok(b) => b,
        Err(TOO_LARGE) => return Err(TOO_LARGE),
        Err(_) => return Ok(None),
    };
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
        let entry = archive.by_index(0).ok()?;
        let mut raw = Vec::new();
        entry.take(UNZIPPED_MAX).read_to_end(&mut raw).ok()?;
        Some(String::from_utf8_lossy(&raw).into_owned())
    })
    .await
    .map_err(|_| UNREADABLE)
}

/// The articles of the press feed naming `watched`, over its window: the
/// server reads the world's press once, whatever the number of companies.
pub async fn fetch_press(
    url: &str,
    feed: &pnex_core::ontology::import::PressFeed,
    watched: Vec<pnex_core::gkg::Watched>,
) -> Result<Vec<Value>, &'static str> {
    if watched.is_empty() {
        return Ok(Vec::new());
    }
    let raw = format!("{}/", url.trim_end_matches('/'));
    let base = check_url(&raw).map_err(|_| UNREACHABLE)?;
    let matcher = std::sync::Arc::new(pnex_core::gkg::Matcher::new(watched));
    let names = pnex_core::gkg::file_names(chrono::Utc::now(), feed.hours);
    let mut rows = Vec::new();
    let mut read = 0usize;
    for name in &names {
        if let Some(text) = press_file(&base, name).await? {
            read += 1;
            let m = matcher.clone();
            let found = tokio::task::spawn_blocking(move || m.records(&text))
                .await
                .map_err(|_| UNREADABLE)?;
            rows.extend(found);
        }
    }
    tracing::info!(
        files = names.len(),
        read,
        articles = rows.len(),
        "press feed read"
    );
    // Some files are always missing; all of them is the feed being down.
    if read == 0 {
        return Err(UNREACHABLE);
    }
    Ok(pnex_core::gkg::digest(rows))
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
