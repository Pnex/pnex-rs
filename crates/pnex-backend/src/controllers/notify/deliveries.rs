use super::*;
use crate::services::notify_journal::DeliveryQuery;

/// Journal filters (`GET /api/v1/notify/deliveries`).
#[derive(Debug, Deserialize)]
pub(super) struct DeliveriesQuery {
    channel_id: Option<String>,
    status: Option<String>,
    source: Option<String>,
    q: Option<String>,
    /// RFC 3339 bounds.
    from: Option<String>,
    to: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

const STATUSES: [&str; 3] = [
    pnex_core::DELIVERY_SENT,
    pnex_core::DELIVERY_FAILED,
    pnex_core::DELIVERY_BLOCKED,
];
const SOURCES: [&str; 4] = ["flow", "test", "ota", "direct"];

fn parse_us(raw: Option<&str>) -> Option<i64> {
    raw.and_then(|s| chrono::DateTime::parse_from_rfc3339(s.trim()).ok())
        .map(|d| d.timestamp_micros())
}

fn journal_unavailable(e: &str) -> Error {
    tracing::warn!(error = %e, "notify journal: openobserve read failed");
    Error::CustomError(
        StatusCode::BAD_GATEWAY,
        loco_rs::controller::ErrorDetail::new(
            err_codes::NOTIFY_JOURNAL_UNAVAILABLE,
            "The notification journal (OpenObserve) is unavailable",
        ),
    )
}

/// Shared by the org-wide journal and the per-channel alias.
async fn journal_page(
    ctx: &AppContext,
    org: &OrgContext,
    path: &str,
    channel_id: Option<Uuid>,
    q: DeliveriesQuery,
) -> Result<Response> {
    let status = q.status.clone().filter(|v| !v.is_empty());
    if status.as_deref().is_some_and(|s| !STATUSES.contains(&s)) {
        return Ok(field_status("status", "invalid"));
    }
    let source = q.source.clone().filter(|v| !v.is_empty());
    if source.as_deref().is_some_and(|s| !SOURCES.contains(&s)) {
        return Ok(field_status("source", "invalid"));
    }
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let query = DeliveryQuery {
        channel_id,
        status,
        source,
        text: q.q.clone(),
        from_us: parse_us(q.from.as_deref()),
        to_us: parse_us(q.to.as_deref()),
        offset: page.offset,
        limit: page.limit,
    };
    let result = notify_journal::search(ctx, org.org.id, &query)
        .await
        .map_err(|e| journal_unavailable(&e))?;
    let mut filters = Vec::new();
    for (k, v) in [
        ("channel_id", &q.channel_id),
        ("status", &q.status),
        ("source", &q.source),
        ("q", &q.q),
        ("from", &q.from),
        ("to", &q.to),
    ] {
        if let Some(v) = v.as_deref().filter(|v| !v.is_empty()) {
            filters.push((k.to_string(), v.to_string()));
        }
    }
    let (available, total, results) = match result {
        Some((total, rows)) => (true, total, rows),
        None => (false, 0, Vec::new()),
    };
    let mut body = pagination::envelope(path, &filters, page, total, results);
    body["available"] = serde_json::Value::Bool(available);
    Ok(format::json(body).into_response())
}

/// `GET /api/v1/notify/deliveries?channel_id=&status=&source=&q=&from=&to=`
/// — every delivery attempt of the org, newest first (O2, D86).
pub(super) async fn list_deliveries(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<DeliveriesQuery>,
) -> Result<Response> {
    let channel_id = match q.channel_id.as_deref().filter(|v| !v.is_empty()) {
        None => None,
        Some(raw) => match Uuid::parse_str(raw) {
            Ok(id) => Some(id),
            Err(_) => return Ok(field_status("channel_id", "invalid")),
        },
    };
    journal_page(&ctx, &org, "/api/v1/notify/deliveries", channel_id, q).await
}

/// `GET /api/v1/notify/channels/{id}/deliveries` — the journal of one
/// channel (same envelope).
pub(super) async fn channel_deliveries(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<DeliveriesQuery>,
) -> Result<Response> {
    let Some(_) = find_channel(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let path = format!("/api/v1/notify/channels/{id}/deliveries");
    journal_page(&ctx, &org, &path, Some(id), q).await
}
