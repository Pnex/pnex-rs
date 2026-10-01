use super::*;

// ─────────────────────── placements devices (D43) ───────────────────────

/// `POST /api/v1/pois/{id}/devices` — attache un device (délibéré) :
/// 201 placement ; 400 device inconnu / location trop longue ; **409
/// conflit** porteur du POI courant (le front propose le déplacement —
/// jamais silencieux) ; 404 masqué cross-org.
pub(super) async fn attach_device(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(p): Json<DeviceAttachPayload>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "poi-write-forbidden",
            "Owner, admin or member role required to manage POIs",
        ));
    }
    let pin = svc::find_pin(&ctx.db, org.org.id, id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let pin = or_not_found(pin)?;
    match svc::attach_device(
        &ctx.db,
        org.org.id,
        pin,
        svc::NewPlacement {
            device_id: &p.device_id,
            location_detail: p.location_detail.as_deref(),
        },
    )
    .await
    {
        Ok(placement) => {
            let dto = placement_dto(&placement);
            Ok((StatusCode::CREATED, format::json(dto)).into_response())
        }
        Err(e) => Ok(write_error_response(e)),
    }
}

/// `PATCH /api/v1/pois/placements/{id}` — édition de `location_detail`
/// et/ou **déplacement** (`pin_id` cible). Le retrait passe par
/// [`delete_placement`] (amendement D43, 2026-09-13) ; supprimer le POI
/// cascade les placements côté DB.
pub(super) async fn update_placement(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(p): Json<PlacementPatchPayload>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "poi-write-forbidden",
            "Owner, admin or member role required to manage POIs",
        ));
    }
    match svc::update_placement(
        &ctx.db,
        org.org.id,
        id,
        svc::UpdatePlacement {
            location_detail: p.location_detail.as_ref().map(|o| o.as_deref()),
            pin_id: p.pin_id,
        },
    )
    .await
    {
        Ok(placement) => format::json(placement_dto(&placement)),
        Err(e) => Ok(write_error_response(e)),
    }
}

/// `DELETE /api/v1/pois/placements/{id}` — détache le device (amendement
/// D43, 2026-09-13) : 204, le device redevient libre, plaçable n'importe
/// où. 404 masqué cross-org.
pub(super) async fn delete_placement(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "poi-write-forbidden",
            "Owner, admin or member role required to manage POIs",
        ));
    }
    match svc::delete_placement(&ctx.db, org.org.id, id)
        .await
        .map_err(|_| Error::InternalServerError)?
    {
        true => Ok(StatusCode::NO_CONTENT.into_response()),
        false => Err(Error::NotFound),
    }
}
