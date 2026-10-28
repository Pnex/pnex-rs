use super::*;

// ─────────────────────────── positions GPS (D38) ───────────────────────────

/// Page of a list (`limit`/`offset`, D14).
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct PositionsQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/device-positions` — last position per device (live
/// map layer, 15 s poll on the front).
#[utoipa::path(
    get,
    path = "/api/v1/device-positions",
    tag = "device-positions",
    operation_id = "device_positions_list",
    summary = "Last known GPS position of each device",
    description = "Latest position per device of the org (live layer of the map, polled by the UI). Paginated (`limit`/`offset`). Any org member.",
    params(PositionsQuery),
    responses((status = 200, body = pnex_core::Paginated<DevicePositionDto>)),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn list_positions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PositionsQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = svc::list_positions(&ctx.db, org.org.id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let (skip, take) = page.slice(rows.len());
    let results: Vec<DevicePositionDto> = rows
        .iter()
        .skip(skip)
        .take(take)
        .map(position_dto)
        .collect();
    format::json(pagination::envelope(
        "/api/v1/device-positions",
        &[],
        page,
        rows.len() as i64,
        results,
    ))
}
