//! Edge guard of the `/internal/*` routes (docs/architecture/security.md R7,
//! SEC-4). Their callers (flow runtime, notify relay, cluster peers) always
//! reach the backend directly (loopback or pod IP); a request carrying a
//! reverse-proxy forwarding header came from the public edge and is
//! answered 404, as if the route did not exist — even when the proxy itself
//! forgot to block the prefix (Helm ingress `/` covers everything).

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Path prefix of the service-to-service routes.
pub const INTERNAL_PREFIX: &str = "/internal/";

/// Headers a reverse proxy adds on the requests it forwards.
const FORWARDING_HEADERS: &[&str] = &["forwarded", "x-forwarded-for", "x-real-ip"];

/// Whether a request for `path` with these headers came through the edge
/// and targets an internal route.
pub fn proxied_internal(path: &str, headers: &axum::http::HeaderMap) -> bool {
    path.starts_with(INTERNAL_PREFIX) && FORWARDING_HEADERS.iter().any(|h| headers.contains_key(*h))
}

/// Router-wide middleware: proxied internal requests → 404.
pub async fn middleware(req: Request, next: Next) -> Response {
    if proxied_internal(req.uri().path(), req.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn only_forwarded_internal_requests_are_refused() {
        let mut fwd = HeaderMap::new();
        fwd.insert("x-forwarded-for", "203.0.113.7".parse().unwrap());
        assert!(proxied_internal("/internal/flow/device-write", &fwd));
        assert!(!proxied_internal("/api/v1/devices", &fwd));
        assert!(!proxied_internal(
            "/internal/flow/device-write",
            &HeaderMap::new()
        ));
    }
}
