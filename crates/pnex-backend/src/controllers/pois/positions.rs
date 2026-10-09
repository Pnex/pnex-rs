use super::*;

// ─────────────────────────── positions GPS (D38) ───────────────────────────

/// `GET /api/v1/device-positions` — dernière position par device (couche
/// live de la carte, poll 15 s côté front).
pub(super) async fn list_positions(
    State(ctx): State<AppContext>,
    org: OrgContext,
) -> Result<Response> {
    let rows = svc::list_positions(&ctx.db, org.org.id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let results: Vec<DevicePositionDto> = rows.iter().map(position_dto).collect();
    format::json(serde_json::json!({ "count": results.len(), "results": results }))
}
