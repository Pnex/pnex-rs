//! Events API (camera-video.md D84) — org-scoped read of the JSON events
//! stored in OpenObserve logs streams (`ev_…`).
//!
//! - `GET /api/v1/events/streams` — event streams of the org (D14 page)
//! - `GET /api/v1/events?stream=&level=&q=&from=&to=&limit=&offset=` — D14
//!   page; OpenObserve not configured (or org never provisioned) = 503
//!   `events-unavailable`, read failure = 502 `events-unavailable`.

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

/// 503 when OpenObserve is not configured or the org has no event storage.
fn o2_not_configured() -> Error {
    Error::CustomError(
        StatusCode::SERVICE_UNAVAILABLE,
        loco_rs::controller::ErrorDetail::new(
            "events-unavailable",
            "Events storage (OpenObserve) is not configured for this organization",
        ),
    )
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct StreamsQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/events/streams",
    tag = "events",
    operation_id = "events_streams",
    summary = "List the event streams of the organisation",
    description = "Names of the OpenObserve event streams (`ev_…`) written by the org flows, sorted, paginated (`limit`/`offset`). Read-only, any member.",
    params(StreamsQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<String>),
        (status = 502, description = "`events-unavailable`: OpenObserve read failed", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`events-unavailable`: OpenObserve not configured for the org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn streams(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<StreamsQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let names = events::streams(&ctx, org.org.id)
        .await
        .map_err(|e| o2_unavailable(&e))?
        .ok_or_else(o2_not_configured)?;
    let (skip, take) = page.slice(names.len());
    let count = names.len() as i64;
    let rows: Vec<String> = names.into_iter().skip(skip).take(take).collect();
    format::json(pagination::envelope(
        "/api/v1/events/streams",
        &[],
        page,
        count,
        rows,
    ))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// Event stream name (required in practice: empty or invalid = 400).
    stream: Option<String>,
    /// Level filter (`debug`, `info`, `warn`, `error`…); empty = all.
    level: Option<String>,
    /// Full-text search on the event.
    q: Option<String>,
    /// RFC 3339 bounds.
    from: Option<String>,
    to: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

fn parse_us(raw: Option<&str>) -> Option<i64> {
    raw.and_then(|s| chrono::DateTime::parse_from_rfc3339(s.trim()).ok())
        .map(|d| d.timestamp_micros())
}

#[utoipa::path(
    get,
    path = "/api/v1/events",
    tag = "events",
    operation_id = "events_list",
    summary = "Search the events of a stream",
    description = "Paginated (`limit`/`offset`), newest first, default window = last 24 h when `from`/`to` are absent. Read-only, any member.",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::events::EventRecord>),
        (status = 400, description = "`field-invalid`: `stream` or `level` is `invalid`", body = crate::openapi::ErrorDetail),
        (status = 502, description = "`events-unavailable`: OpenObserve read failed", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`events-unavailable`: OpenObserve not configured for the org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let Some(stream) = pnex_core::events::event_stream_name(q.stream.as_deref().unwrap_or(""))
    else {
        return Ok(super::field_error(
            "stream",
            pnex_core::err_codes::FIELD_INVALID,
        ));
    };
    let level = q
        .level
        .as_deref()
        .filter(|l| !l.is_empty())
        .map(|l| pnex_core::events::EventLevel::from_wire(l).map(|l| l.wire().to_string()));
    if matches!(level, Some(None)) {
        return Ok(super::field_error(
            "level",
            pnex_core::err_codes::FIELD_INVALID,
        ));
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
        .map_err(|e| o2_unavailable(&e))?
        .ok_or_else(o2_not_configured)?;
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
    format::json(pagination::envelope(
        "/api/v1/events",
        &filters,
        page,
        result.total,
        result.records,
    ))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(list, streams))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
