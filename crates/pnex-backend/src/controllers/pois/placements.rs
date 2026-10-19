use super::*;

// ─────────────────────── placements devices (D43) ───────────────────────

/// `POST /api/v1/pois/{id}/devices` — attache un device (délibéré) :
/// 201 placement ; 400 device inconnu / location trop longue ; **409
/// conflit** porteur du POI courant (le front propose le déplacement —
/// jamais silencieux) ; 404 masqué cross-org.
#[utoipa::path(
    post,
    path = "/api/v1/pois/{id}/devices",
    tag = "pois",
    operation_id = "pois_attach_device",
    summary = "Place a device on a POI",
    description = "Attaches a device of the org to this POI (one placement per device). If the device is already placed elsewhere, answers 409 with the current placement so the caller can confirm a move (PATCH the placement). Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "POI id (UUID)")),
    request_body = DeviceAttachPayload,
    responses(
        (status = 201, body = DevicePlacementDto),
        (status = 400, description = "`field-invalid`: `device_id` not found in the org, `location_detail` too long", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown POI or POI of another org", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`poi-device-already-placed`: `errors.args` carries `device_id`, `current_placement_id`, `current_pin_id`, `current_pin_label` (strings)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn attach_device(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(p): Json<DeviceAttachPayload>,
) -> Result<Response> {
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
#[utoipa::path(
    patch,
    path = "/api/v1/pois/placements/{id}",
    tag = "pois",
    operation_id = "pois_placements_update",
    summary = "Edit or move a device placement",
    description = "Changes the placement `location_detail` and/or moves the device to another POI (`pin_id`). Requires owner, admin or member role.",
    params(("id" = i64, Path, description = "Placement id")),
    request_body = PlacementPatchPayload,
    responses(
        (status = 200, body = DevicePlacementDto),
        (status = 400, description = "`field-invalid`: `pin_id` not found in the org, `location_detail` too long", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown placement or placement of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn update_placement(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(p): Json<PlacementPatchPayload>,
) -> Result<Response> {
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
#[utoipa::path(
    delete,
    path = "/api/v1/pois/placements/{id}",
    tag = "pois",
    operation_id = "pois_placements_delete",
    summary = "Detach a device from its POI",
    description = "Deletes the placement: the device is free to be placed anywhere again. Requires owner, admin or member role.",
    params(("id" = i64, Path, description = "Placement id")),
    responses(
        (status = 204, description = "Detached"),
        (status = 404, description = "Unknown placement or placement of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
pub(super) async fn delete_placement(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    match svc::delete_placement(&ctx.db, org.org.id, id)
        .await
        .map_err(|_| Error::InternalServerError)?
    {
        true => Ok(StatusCode::NO_CONTENT.into_response()),
        false => Err(Error::NotFound),
    }
}
