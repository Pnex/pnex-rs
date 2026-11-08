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
    tokio::task::spawn_blocking(move || match m.shape {
        Some(shape) => pnex_core::markets::records(shape, &bytes).ok_or(UNREADABLE),
        None => read_records(&bytes, &name, &m).map_err(read_error),
    })
    .await
    .unwrap_or(Err(UNREADABLE))
}

/// Placeholder of the day to read from: a few days before the last value the
/// object holds, the whole history the first time. Keeps a daily read small.
pub const SINCE: &str = "{since}";
const HISTORY_START: &str = "2000-01-01";

/// Placeholder of the per-object value in a source URL.
pub const VALUE: &str = "{value}";
/// Field of a per-object record holding the value it was queried with.
pub const QUERIED: &str = pnex_core::markets::QUERIED;

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
    by: &Declared,
    since: &std::collections::HashMap<String, String>,
    titles: &std::collections::HashMap<String, String>,
) -> Result<Vec<Value>, &'static str> {
    let base = check_url(url).map_err(|_| UNREACHABLE)?;
    let param = m.per_object.as_ref().map_or("", |p| p.param.as_str());
    let by_title = m.per_object.as_ref().map_or("", |p| p.by_title.as_str());
    let mut rows = Vec::new();
    let mut failures = 0usize;
    for v in values {
        let u = if param.is_empty() {
            // Inside a path or a query text: only plain identifiers go in
            // (unreserved characters, nothing to encode).
            if !plain(v) {
                failures += 1;
                continue;
            }
            let path_value = pnex_core::markets::url_value(m.shape, v);
            let from = since.get(v).map_or(HISTORY_START, String::as_str);
            check_url(&url.replace(VALUE, &path_value).replace(SINCE, from))
                .map_err(|_| UNREACHABLE)?
        } else {
            let asked = if by_title.is_empty() {
                v.as_str()
            } else if let Some(title) = titles.get(v) {
                title.as_str()
            } else {
                failures += 1;
                continue;
            };
            let mut u = base.clone();
            u.query_pairs_mut().append_pair(param, asked);
            u
        };
        match get_as(&u, by).await {
            Ok(bytes) => {
                let mut found = match m.shape {
                    Some(shape) => {
                        // Hostile input, and a large filer's facts weigh megabytes.
                        tokio::task::spawn_blocking(move || {
                            pnex_core::markets::records(shape, &bytes)
                        })
                        .await
                        .map_err(|_| UNREADABLE)?
                        .ok_or(UNREADABLE)?
                    }
                    None => {
                        let doc: Value = serde_json::from_slice(&bytes).map_err(|_| UNREADABLE)?;
                        json_rows(&doc, &m.rows).ok_or(UNREADABLE)?
                    }
                };
                if !by_title.is_empty() {
                    found.retain(|r| r.get(by_title).and_then(Value::as_str) == Some(v.as_str()));
                }
                // A record may not repeat what it was asked for (the parent
                // of a list of subsidiaries): the mapping reads it here.
                for r in &mut found {
                    if let Some(o) = r.as_object_mut() {
                        o.insert(QUERIED.to_string(), Value::String(v.clone()));
                        // An ALFRED answer does not name its series: it is the code asked.
                        if m.shape == Some(pnex_core::markets::ResponseShape::AlfredVintages) {
                            o.insert("metric".to_string(), Value::String(v.clone()));
                        }
                    }
                }
                rows.extend(found);
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

/// The leads of the GDELT Events feed over its window (D740): every slice
/// is read once, only the lines of the watched countries and codes are kept,
/// then aggregated per (country, code family, day).
pub async fn fetch_events(
    url: &str,
    hours: u32,
    filter: pnex_core::gdelt_events::EventsFilter,
) -> Result<Vec<Value>, &'static str> {
    if filter.countries.is_empty() || filter.codes.is_empty() {
        return Ok(Vec::new());
    }
    let raw = format!("{}/", url.trim_end_matches('/'));
    let base = check_url(&raw).map_err(|_| UNREACHABLE)?;
    use futures_util::StreamExt;
    let names = pnex_core::gdelt_events::file_names(chrono::Utc::now(), hours);
    let filter = std::sync::Arc::new(filter);
    let mut agg = pnex_core::gdelt_events::Aggregator::default();
    let mut read = 0usize;
    // A few files in flight: a history run reads thousands of them.
    let mut files = futures_util::stream::iter(names.clone())
        .map(|name| {
            let base = base.clone();
            async move { press_file(&base, &name).await }
        })
        .buffer_unordered(EVENTS_IN_FLIGHT);
    while let Some(file) = files.next().await {
        if let Some(text) = file? {
            read += 1;
            let f = filter.clone();
            agg = tokio::task::spawn_blocking(move || {
                agg.add(&f, &text);
                agg
            })
            .await
            .map_err(|_| UNREADABLE)?;
        }
    }
    tracing::info!(
        files = names.len(),
        read,
        lines = agg.lines,
        "events feed read"
    );
    // Some files are always missing; all of them is the feed being down.
    if read == 0 {
        return Err(UNREACHABLE);
    }
    Ok(agg.records())
}

/// GDELT files downloaded at once by an events feed.
const EVENTS_IN_FLIGHT: usize = 6;

/// Pages of a conflict source read per run, at most.
const CONFLICT_PAGES: u32 = 40;
/// Rows per ACLED page.
const ACLED_LIMIT: usize = 5000;

/// Verified conflict events of the watched countries over the last `days`
/// (D741). `secrets` are the values of `feed.source.secrets()`, in order;
/// the caller checked that `url` is the source's own host.
pub async fn fetch_conflicts(
    url: &str,
    feed: &pnex_core::ontology::import::ConflictFeed,
    secrets: Vec<String>,
    countries: Vec<String>,
) -> Result<Vec<Value>, &'static str> {
    use pnex_core::conflict_events::{self as ce, ConflictSource};
    if countries.is_empty() {
        return Ok(Vec::new());
    }
    let base = check_url(url).map_err(|_| UNREACHABLE)?;
    let now = chrono::Utc::now();
    let known_at = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let (since, today) = (
        (now.date_naive() - chrono::Duration::days(feed.days as i64)).to_string(),
        now.date_naive().to_string(),
    );
    let json = |b: Vec<u8>| serde_json::from_slice::<Value>(&b).map_err(|_| UNREADABLE);
    let mut agg = ce::Aggregator::new(&countries);
    // A live API stands for "known now" without report dates; a history
    // file does not (D531).
    let mut fallback = Some(known_at.as_str());
    match feed.source {
        ConflictSource::Ucdp => {
            let by = Declared {
                headers: vec![(
                    "x-ucdp-access-token",
                    secrets.first().cloned().unwrap_or_default(),
                )],
                ..Default::default()
            };
            // UCDP's pages count from 0 (its docs: page 1's previous is 0).
            for page in 0..CONFLICT_PAGES {
                let mut u = base.clone();
                u.query_pairs_mut()
                    .append_pair("pagesize", "1000")
                    .append_pair("page", &page.to_string())
                    .append_pair("StartDate", &since)
                    .append_pair("EndDate", &today);
                let doc = json(get_as(&u, &by).await?)?;
                let (rows, pages) = ce::ucdp_rows(&doc).ok_or(UNREADABLE)?;
                rows.iter().for_each(|r| agg.add(ce::ucdp_event(r)));
                if page as u64 + 1 >= pages {
                    break;
                }
                if page + 1 == CONFLICT_PAGES {
                    tracing::warn!(pages, "UCDP read stopped at the page cap");
                }
            }
        }
        ConflictSource::UcdpFile => {
            fallback = None;
            let bytes = get_as(&base, &Declared::default()).await?;
            agg = tokio::task::spawn_blocking(move || ucdp_csv(bytes, agg))
                .await
                .map_err(|_| UNREADABLE)??;
        }
        ConflictSource::Acled => {
            let token = acled_token(&base, &secrets).await?;
            let by = Declared {
                authorization: Some(format!("Bearer {token}")),
                ..Default::default()
            };
            for iso in countries.iter().filter_map(|c| ce::numeric_of_iso(c)) {
                for page in 1..=CONFLICT_PAGES {
                    let mut u = base.clone();
                    u.query_pairs_mut()
                        .append_pair("_format", "json")
                        .append_pair("event_date", &format!("{since}|{today}"))
                        .append_pair("event_date_where", "BETWEEN")
                        .append_pair("iso", &iso.to_string())
                        .append_pair("limit", &ACLED_LIMIT.to_string())
                        .append_pair("page", &page.to_string());
                    let doc = json(get_as(&u, &by).await?)?;
                    let rows = ce::acled_rows(&doc).ok_or(UNREADABLE)?;
                    let full = rows.len() >= ACLED_LIMIT;
                    rows.iter().for_each(|r| agg.add(ce::acled_event(r)));
                    if !full {
                        break;
                    }
                    if page == CONFLICT_PAGES {
                        tracing::warn!(iso, "ACLED read stopped at the page cap");
                    }
                }
            }
        }
    }
    tracing::info!(
        source = feed.source.as_str(),
        rows = agg.seen,
        kept = agg.kept,
        "conflict feed read"
    );
    Ok(agg.records(feed.source.as_str(), fallback))
}

/// A UCDP download (CSV, plain or zipped) read as a stream: GED is close to
/// 300 MB unzipped, only the watched countries' rows are kept.
fn ucdp_csv(
    bytes: Vec<u8>,
    agg: pnex_core::conflict_events::Aggregator,
) -> Result<pnex_core::conflict_events::Aggregator, &'static str> {
    use std::io::Read;
    let cursor = std::io::Cursor::new(bytes);
    if cursor.get_ref().starts_with(b"PK\x03\x04") {
        let mut archive = zip::ZipArchive::new(cursor).map_err(|_| UNREADABLE)?;
        let entry = archive.by_index(0).map_err(|_| UNREADABLE)?;
        let mut input = entry.take(CONFLICT_UNZIPPED_MAX);
        return ucdp_rows_of(&mut input, agg);
    }
    let mut input = cursor;
    ucdp_rows_of(&mut input, agg)
}

fn ucdp_rows_of(
    input: &mut dyn std::io::Read,
    mut agg: pnex_core::conflict_events::Aggregator,
) -> Result<pnex_core::conflict_events::Aggregator, &'static str> {
    let mut header: Option<Vec<String>> = None;
    let mut rows = 0usize;
    csv_records(input, |fields| match &header {
        None => header = Some(fields),
        Some(h) => {
            rows += 1;
            let row: serde_json::Map<String, Value> = h
                .iter()
                .cloned()
                .zip(fields.into_iter().map(Value::String))
                .collect();
            agg.add(pnex_core::conflict_events::ucdp_event(&Value::Object(row)));
        }
    })?;
    if header.is_none() || rows == 0 {
        return Err(UNREADABLE);
    }
    Ok(agg)
}

/// Largest UCDP download unzipped (GED 26.1 is 274 MB).
const CONFLICT_UNZIPPED_MAX: u64 = 600 * 1024 * 1024;

/// Every record of a CSV stream (RFC 4180, quoted fields over several
/// lines), one call per record, without holding the parsed table.
fn csv_records(
    input: &mut dyn std::io::Read,
    mut each: impl FnMut(Vec<String>),
) -> Result<(), &'static str> {
    use csv_core::{ReadRecordResult, Reader};
    let mut reader = Reader::new();
    let mut buf = vec![0u8; 64 * 1024];
    let (mut start, mut filled) = (0usize, 0usize);
    let mut out = vec![0u8; 1024 * 1024];
    let mut ends = vec![0usize; 256];
    let (mut out_len, mut ends_len) = (0usize, 0usize);
    let mut eof = false;
    loop {
        if start == filled && !eof {
            filled = input.read(&mut buf).map_err(|_| UNREADABLE)?;
            start = 0;
            eof = filled == 0;
        }
        let (res, nin, nout, nend) = reader.read_record(
            &buf[start..filled],
            &mut out[out_len..],
            &mut ends[ends_len..],
        );
        start += nin;
        out_len += nout;
        ends_len += nend;
        match res {
            ReadRecordResult::InputEmpty => continue,
            ReadRecordResult::OutputFull => {
                if out.len() >= 64 * 1024 * 1024 {
                    return Err(UNREADABLE);
                }
                out.resize(out.len() * 2, 0);
            }
            ReadRecordResult::OutputEndsFull => ends.resize(ends.len() * 2, 0),
            ReadRecordResult::Record => {
                let mut fields = Vec::with_capacity(ends_len);
                let mut from = 0;
                for &end in &ends[..ends_len] {
                    fields.push(String::from_utf8_lossy(&out[from..end]).into_owned());
                    from = end;
                }
                each(fields);
                out_len = 0;
                ends_len = 0;
            }
            ReadRecordResult::End => return Ok(()),
        }
    }
}

/// ACLED's OAuth token for the org's own account (password grant, on the
/// source's host only). The password is sent in the body, never logged.
async fn acled_token(base: &reqwest::Url, secrets: &[String]) -> Result<String, &'static str> {
    let (Some(user), Some(password)) = (secrets.first(), secrets.get(1)) else {
        return Err(UNREACHABLE);
    };
    let url = base.join("/oauth/token").map_err(|_| UNREACHABLE)?;
    let resp = http_keyed()
        .post(url)
        .form(&[
            ("username", user.as_str()),
            ("password", password.as_str()),
            ("grant_type", "password"),
            ("client_id", "acled"),
            ("scope", "authenticated"),
        ])
        .send()
        .await
        .map_err(|_| UNREACHABLE)?;
    if !resp.status().is_success() {
        return Err(UNREACHABLE);
    }
    let doc: Value = resp.json().await.map_err(|_| UNREADABLE)?;
    doc.get("access_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(UNREADABLE)
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
