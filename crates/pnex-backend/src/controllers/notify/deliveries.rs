use super::*;
use crate::services::notify_journal::DeliveryQuery;

/// Journal filters (`GET /api/v1/notify/deliveries`).
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct DeliveriesQuery {
    /// Only this channel.
    #[param(format = "uuid")]
    channel_id: Option<String>,
    /// `sent` | `failed` | `blocked`.
    status: Option<String>,
    /// `flow` | `test` | `ota` | `direct`.
    source: Option<String>,
    /// Free-text search.
    q: Option<String>,
    /// Lower bound (RFC 3339).
    #[param(format = DateTime)]
    from: Option<String>,
    /// Upper bound (RFC 3339).
    #[param(format = DateTime)]
    to: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
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

/// 503 `notify-journal-unavailable`: OpenObserve is not configured, the org
/// has no journal yet, or the read failed.
fn journal_unavailable() -> Error {
    crate::controllers::coded_error(
        StatusCode::SERVICE_UNAVAILABLE,
        err_codes::NOTIFY_JOURNAL_UNAVAILABLE,
        "The notification journal (OpenObserve) is unavailable",
        None,
    )
}

/// The org-wide journal, optionally filtered by channel.
async fn journal_page(
    ctx: &AppContext,
    org: &OrgContext,
    path: &str,
    channel_id: Option<Uuid>,
    q: DeliveriesQuery,
) -> Result<Response> {
    let status = q.status.clone().filter(|v| !v.is_empty());
    if status.as_deref().is_some_and(|s| !STATUSES.contains(&s)) {
        return Ok(field_status("status", err_codes::FIELD_INVALID));
    }
    let source = q.source.clone().filter(|v| !v.is_empty());
    if source.as_deref().is_some_and(|s| !SOURCES.contains(&s)) {
        return Ok(field_status("source", err_codes::FIELD_INVALID));
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
    let (total, results) = notify_journal::search(ctx, org.org.id, &query)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "notify journal: openobserve read failed");
            journal_unavailable()
        })?
        .ok_or_else(journal_unavailable)?;
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
    Ok(format::json(pagination::envelope(path, &filters, page, total, results)).into_response())
}

/// `GET /api/v1/notify/deliveries?channel_id=&status=&source=&q=&from=&to=`
/// — every delivery attempt of the org, newest first (O2, D86).
#[utoipa::path(
    get,
    path = "/api/v1/notify/deliveries",
    tag = "notify",
    operation_id = "notify_deliveries_list",
    summary = "List notification delivery attempts",
    description = "Paginated delivery journal of the organisation, newest first, read from OpenObserve. Filters: channel, status, source, free text, time range. 503 `notify-journal-unavailable` when OpenObserve is not configured, the organisation has no journal yet (nothing ever sent) or the read failed.",
    params(DeliveriesQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::NotifyDelivery>),
        (status = 400, description = "`field-invalid`: invalid `channel_id`, `status` or `source` filter", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`notify-journal-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn list_deliveries(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<DeliveriesQuery>,
) -> Result<Response> {
    let channel_id = match q.channel_id.as_deref().filter(|v| !v.is_empty()) {
        None => None,
        Some(raw) => match Uuid::parse_str(raw) {
            Ok(id) => Some(id),
            Err(_) => return Ok(field_status("channel_id", err_codes::FIELD_INVALID_UUID)),
        },
    };
    journal_page(&ctx, &org, "/api/v1/notify/deliveries", channel_id, q).await
}
