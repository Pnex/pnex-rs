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

/// `PUT /api/v1/device-positions/{device_id}` — position manuelle (POC /
/// tests ; `source = manual`). La vraie voie = télémétrie GPS (D38).
pub(super) async fn set_manual_position(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(device_id): Path<String>,
    Json(p): Json<ManualPositionPayload>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "poi-position-write-forbidden",
            "Owner, admin or member role required to update device positions",
        ));
    }
    let (Some(latitude), Some(longitude)) = (p.latitude, p.longitude) else {
        return Ok(field_status(
            "coordinates",
            "latitude et longitude sont requis.",
        ));
    };
    match svc::set_manual(
        &ctx.db,
        org.org.id,
        &device_id,
        latitude,
        longitude,
        chrono::Utc::now().into(),
    )
    .await
    {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => Ok(write_error_response(e)),
    }
}
