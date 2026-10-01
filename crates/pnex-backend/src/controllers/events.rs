//! Events API (camera-video.md D84) — org-scoped read of the JSON events
//! stored in OpenObserve logs streams (`ev_…`).
//!
//! - `GET /api/v1/events/streams` — event streams of the org
//! - `GET /api/v1/events?stream=&level=&q=&from=&to=&limit=&offset=` — D14
//!   envelope + `available` (false = O2 not configured / org never written)

use axum::extract::{Query, State};
use axum::http::StatusCode;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use serde::Deserialize;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::services::events::{self, EventQuery};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/events")
        .add("", get(list))
        .add("/streams", get(streams))
}

fn o2_unavailable(e: &str) -> Error {
    tracing::warn!(error = %e, "events: openobserve read failed");
    Error::CustomError(
        StatusCode::BAD_GATEWAY,
        loco_rs::controller::ErrorDetail::new(
            "events-unavailable",
            "Events storage (OpenObserve) is unavailable",
        ),
    )
}

async fn streams(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    let names = events::streams(&ctx, org.org.id)
        .await
        .map_err(|e| o2_unavailable(&e))?;
    format::json(names)
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    stream: Option<String>,
    level: Option<String>,
    q: Option<String>,
    /// RFC 3339 bounds.
    from: Option<String>,
    to: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

fn parse_us(raw: Option<&str>) -> Option<i64> {
    raw.and_then(|s| chrono::DateTime::parse_from_rfc3339(s.trim()).ok())
        .map(|d| d.timestamp_micros())
}

async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let Some(stream) = pnex_core::events::event_stream_name(q.stream.as_deref().unwrap_or(""))
    else {
        return Ok((
            StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({ "stream": "invalid" })),
        )
            .into_response());
    };
    let level = q
        .level
        .as_deref()
        .filter(|l| !l.is_empty())
        .map(|l| pnex_core::events::EventLevel::from_wire(l).map(|l| l.wire().to_string()));
    if matches!(level, Some(None)) {
        return Ok((
            StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({ "level": "invalid" })),
        )
            .into_response());
    }
    let query = EventQuery {
        stream: stream.clone(),
        level: level.flatten(),
        text: q.q.clone(),
        from_us: parse_us(q.from.as_deref()),
        to_us: parse_us(q.to.as_deref()),
        offset: page.offset,
        limit: page.limit,
    };
    let result = events::search(&ctx, org.org.id, &query)
        .await
        .map_err(|e| o2_unavailable(&e))?;
    let mut filters = vec![("stream".to_string(), stream)];
    for (k, v) in [
        ("level", &q.level),
        ("q", &q.q),
        ("from", &q.from),
        ("to", &q.to),
    ] {
        if let Some(v) = v.as_deref().filter(|v| !v.is_empty()) {
            filters.push((k.to_string(), v.to_string()));
        }
    }
    let (available, total, records) = match result {
        Some(r) => (true, r.total, r.records),
        None => (false, 0, Vec::new()),
    };
    let mut body = pagination::envelope("/api/v1/events", &filters, page, total, records);
    body["available"] = serde_json::Value::Bool(available);
    format::json(body)
}
