//! Serving object-store bytes over HTTP with byte ranges (RFC 9110 §14):
//! the one path for every route whose bytes a player seeks in or casts
//! (media, recordings, stream audio, shared tours, artifacts, dataset files).
//!
//! The body streams from the store part by part: a file is never held whole
//! in memory. HEAD is answered by axum from the same response (headers only,
//! the body stream is dropped unread).

use std::ops::Range;

use axum::{
    body::{Body, Bytes},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::Stream;

use crate::services::artifact_store::S3Store;

/// What a request asks of an object of a known size.
#[derive(Debug, PartialEq, Eq)]
pub enum Span {
    Whole,
    /// Half-open byte range, inside the object.
    Part(Range<u64>),
    /// Starts past the end: 416.
    Unsatisfiable,
}

/// The span a request asks for. A malformed `Range` is ignored (whole body),
/// as RFC 9110 allows; so is a range whose `If-Range` does not name `etag`.
pub fn span(headers: &HeaderMap, size: u64, etag: Option<&str>) -> Span {
    let Some(range) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) else {
        return Span::Whole;
    };
    // If-Range: the range only holds for the bytes the client already has.
    // Strong ETag comparison only; no Last-Modified is sent, so a date never matches.
    if let Some(cond) = headers.get(header::IF_RANGE) {
        let holds = etag
            .is_some_and(|e| !e.starts_with("W/") && cond.to_str().is_ok_and(|c| c.trim() == e));
        if !holds {
            return Span::Whole;
        }
    }
    let Some(spec) = range.trim().strip_prefix("bytes=") else {
        return Span::Whole;
    };
    // shortcut: a multi-range request gets the whole body (allowed by RFC 9110), add multipart/byteranges if a client needs it.
    if spec.contains(',') {
        return Span::Whole;
    }
    let Some((first, last)) = spec.split_once('-') else {
        return Span::Whole;
    };
    let (first, last) = (first.trim(), last.trim());
    if first.is_empty() {
        // Suffix `-n`: the last n bytes.
        return match last.parse::<u64>() {
            Err(_) => Span::Whole,
            Ok(0) => Span::Unsatisfiable,
            Ok(_) if size == 0 => Span::Unsatisfiable,
            Ok(n) => Span::Part(size.saturating_sub(n)..size),
        };
    }
    let Ok(start) = first.parse::<u64>() else {
        return Span::Whole;
    };
    let end = if last.is_empty() {
        size
    } else {
        match last.parse::<u64>() {
            Ok(e) if e >= start => e.saturating_add(1).min(size),
            _ => return Span::Whole,
        }
    };
    if start >= size {
        return Span::Unsatisfiable;
    }
    Span::Part(start..end)
}

/// Bytes `range` of `key`, read from the store 8 MiB at a time.
pub fn read_stream(
    store: S3Store,
    key: String,
    range: Range<u64>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    const PART: u64 = 8 * 1024 * 1024;
    let end = range.end;
    futures_util::stream::unfold(range.start, move |at| {
        let (store, key) = (store.clone(), key.clone());
        async move {
            if at >= end {
                return None;
            }
            let to = (at + PART).min(end);
            Some(match store.get_range(&key, at..to).await {
                Ok(bytes) => (Ok(bytes), to),
                Err(e) => (Err(std::io::Error::other(e)), end),
            })
        }
    })
}

/// Response for the object `key`: 200 whole, 206 for a range, 416 past its
/// end; always `Accept-Ranges: bytes`, and `ETag` when given. `size` known
/// from the database skips a stat; `None` stats it, and a missing object is
/// `Ok(None)`. The caller adds its own headers (type, disposition, cache).
pub async fn serve(
    store: S3Store,
    key: String,
    size: Option<u64>,
    request: &HeaderMap,
    etag: Option<&str>,
) -> Result<Option<Response>, String> {
    let size = match size {
        Some(size) => size,
        None => match store.size(&key).await? {
            Some(size) => size,
            None => return Ok(None),
        },
    };
    let (status, range) = match span(request, size, etag) {
        Span::Whole => (StatusCode::OK, 0..size),
        Span::Part(range) => (StatusCode::PARTIAL_CONTENT, range),
        Span::Unsatisfiable => {
            let mut res = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            let h = res.headers_mut();
            h.insert(
                header::CONTENT_RANGE,
                header_value(format!("bytes */{size}")),
            );
            h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            return Ok(Some(res));
        }
    };
    let length = range.end - range.start;
    let content_range = format!(
        "bytes {}-{}/{size}",
        range.start,
        range.end.saturating_sub(1)
    );
    let mut res = (status, Body::from_stream(read_stream(store, key, range))).into_response();
    let h = res.headers_mut();
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    if status == StatusCode::PARTIAL_CONTENT {
        h.insert(header::CONTENT_RANGE, header_value(content_range));
    }
    if let Some(etag) = etag.and_then(|e| HeaderValue::from_str(e).ok()) {
        h.insert(header::ETAG, etag);
    }
    Ok(Some(res))
}

fn header_value(s: String) -> HeaderValue {
    // Digits, spaces, `-`, `/` and `*` only: always a valid header value.
    HeaderValue::from_str(&s).expect("ascii header value")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(range: &str, if_range: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::RANGE, HeaderValue::from_str(range).unwrap());
        if let Some(c) = if_range {
            h.insert(header::IF_RANGE, HeaderValue::from_str(c).unwrap());
        }
        h
    }

    #[test]
    fn spans() {
        let n = 301_296;
        let s = |r: &str| span(&req(r, None), n, None);
        assert_eq!(span(&HeaderMap::new(), n, None), Span::Whole);
        assert_eq!(s("bytes=0-99"), Span::Part(0..100));
        assert_eq!(s("bytes=100-"), Span::Part(100..n));
        assert_eq!(s("bytes=-500"), Span::Part(n - 500..n));
        assert_eq!(s("bytes=-999999"), Span::Part(0..n));
        assert_eq!(s("bytes=0-999999"), Span::Part(0..n));
        assert_eq!(s("bytes=301296-"), Span::Unsatisfiable);
        assert_eq!(s("bytes=-0"), Span::Unsatisfiable);
        // Malformed or unsupported: ignored.
        for bad in [
            "bytes=5-3",
            "items=0-1",
            "bytes=a-b",
            "bytes=0-1,5-6",
            "bytes=-",
        ] {
            assert_eq!(s(bad), Span::Whole, "{bad}");
        }
        assert_eq!(span(&req("bytes=0-", None), 0, None), Span::Unsatisfiable);
    }

    #[test]
    fn if_range_needs_the_same_strong_etag() {
        let e = "\"v1\"";
        assert_eq!(
            span(&req("bytes=0-9", Some(e)), 100, Some(e)),
            Span::Part(0..10)
        );
        assert_eq!(
            span(&req("bytes=0-9", Some("\"v0\"")), 100, Some(e)),
            Span::Whole
        );
        assert_eq!(span(&req("bytes=0-9", Some(e)), 100, None), Span::Whole);
        let date = "Wed, 21 Oct 2015 07:28:00 GMT";
        assert_eq!(
            span(&req("bytes=0-9", Some(date)), 100, Some(e)),
            Span::Whole
        );
    }
}
