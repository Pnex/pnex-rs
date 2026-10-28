//! Device placements on POIs (D43): attach, move, detach, batch load.

use super::*;

/// The org's POI, otherwise `None` (→ masked 404 in the controller).
pub async fn find_pin(
    db: &DatabaseConnection,
    org_id: i64,
    pin_id: Uuid,
) -> Result<Option<map_pins::Model>, DbErr> {
    map_pins::Entity::find_by_id(pin_id)
        .filter(map_pins::Column::OrgId.eq(org_id))
        .one(db)
        .await
}

/// Device of the org's registry by slug, otherwise `None`.
pub(super) async fn find_device(
    db: &DatabaseConnection,
    org_id: i64,
    device_id: &str,
) -> Result<Option<device_registries::Model>, DbErr> {
    device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(org_id))
        .filter(device_registries::Column::DeviceId.eq(device_id))
        .one(db)
        .await
}

// ─────────────────────────── placements devices (D43) ─────────────────────

pub struct NewPlacement<'a> {
    pub device_id: &'a str,
    pub location_detail: Option<&'a str>,
}

/// Attaches a device to a POI. Attaching is **deliberate**: the device must
/// exist in the org (`DeviceUnknown`) and have no placement
/// (`DeviceAlreadyPlaced` carrying the current POI — even if it is THIS POI:
/// the front recognises `current_pin_id == pin.id` and handles it without a dialog).
pub async fn attach_device(
    db: &DatabaseConnection,
    org_id: i64,
    pin: map_pins::Model,
    p: NewPlacement<'_>,
) -> Result<device_placements::Model, VizWriteError> {
    let slug = p.device_id.trim();
    if slug.is_empty() {
        return Err(VizWriteError::DeviceUnknown);
    }
    let device = find_device(db, org_id, slug)
        .await
        .map_err(|_| VizWriteError::Db)?
        .ok_or(VizWriteError::DeviceUnknown)?;
    // Guard-then-insert race window (D36 pattern): the loser hits
    // the UNIQUE constraint (→ Db/500) — the index is the guarantee.
    if let Some(existing) = device_placements::Entity::find()
        .filter(device_placements::Column::OrgId.eq(org_id))
        .filter(device_placements::Column::DeviceRegistryId.eq(device.id))
        .one(db)
        .await
        .map_err(|_| VizWriteError::Db)?
    {
        let current = map_pins::Entity::find_by_id(existing.pin_id)
            .one(db)
            .await
            .map_err(|_| VizWriteError::Db)?;
        return Err(VizWriteError::DeviceAlreadyPlaced(PlacementConflict {
            device_id: existing.device_id,
            current_placement_id: existing.id,
            current_pin_id: existing.pin_id,
            current_pin_label: current.map(|c| c.label).unwrap_or_default(),
        }));
    }
    let location_detail = validate_location_detail(p.location_detail)?;
    device_placements::ActiveModel {
        org_id: Set(org_id),
        device_registry_id: Set(device.id),
        device_id: Set(slug.to_string()),
        pin_id: Set(pin.id),
        location_detail: Set(location_detail),
        ..Default::default()
    }
    .insert(db)
    .await
    .map_err(|_| VizWriteError::Db)
}

pub struct UpdatePlacement<'a> {
    /// absent = unchanged, null = cleared, value = set (UpdatePin pattern).
    pub location_detail: Option<Option<&'a str>>,
    /// Move: target POI validated in the org. no-op (current pin)
    /// allowed — a device has only one placement, a move never creates a
    /// duplicate.
    pub pin_id: Option<Uuid>,
}

/// Edit/move of a placement (DELETE = detaching, see
/// [`delete_placement`]; deleting the POI cascades the placements in the DB).
pub async fn update_placement(
    db: &DatabaseConnection,
    org_id: i64,
    placement_id: i64,
    p: UpdatePlacement<'_>,
) -> Result<device_placements::Model, VizWriteError> {
    let existing = device_placements::Entity::find()
        .filter(device_placements::Column::OrgId.eq(org_id))
        .filter(device_placements::Column::Id.eq(placement_id))
        .one(db)
        .await
        .map_err(|_| VizWriteError::Db)?
        .ok_or(VizWriteError::PlacementUnknown)?;
    let mut am: device_placements::ActiveModel = existing.into();
    if let Some(pin_id) = p.pin_id {
        find_pin(db, org_id, pin_id)
            .await
            .map_err(|_| VizWriteError::Db)?
            .ok_or(VizWriteError::PinUnknown)?;
        am.pin_id = Set(pin_id);
    }
    match p.location_detail {
        Some(None) => am.location_detail = Set(None),
        Some(Some(d)) => am.location_detail = Set(validate_location_detail(Some(d))?),
        None => {}
    }
    am.update(db).await.map_err(|_| VizWriteError::Db)
}

/// Detaching a device (D43 amendment, 2026-09-13): the placement
/// goes, the device becomes **free** again — placeable on any POI
/// afterwards. The « at most one POI at a time » rule is still carried by the UNIQUE
/// `device_registry_id`. `false` = placement not found in the org (masked
/// cross-org 404, `delete_link` pattern).
pub async fn delete_placement(
    db: &DatabaseConnection,
    org_id: i64,
    placement_id: i64,
) -> Result<bool, DbErr> {
    let Some(existing) = device_placements::Entity::find()
        .filter(device_placements::Column::OrgId.eq(org_id))
        .filter(device_placements::Column::Id.eq(placement_id))
        .one(db)
        .await?
    else {
        return Ok(false);
    };
    device_placements::Entity::delete_by_id(existing.id)
        .exec(db)
        .await
        .map(|_| ())?;
    Ok(true)
}

/// Placements of a batch of POIs (anti-N+1 batch — `links_for_pins` pattern),
/// sorted by created_at asc (stable attach order for the drawer).
pub async fn placements_for_pins(
    db: &DatabaseConnection,
    org_id: i64,
    pin_ids: &[Uuid],
) -> Result<Vec<device_placements::Model>, DbErr> {
    if pin_ids.is_empty() {
        return Ok(vec![]);
    }
    device_placements::Entity::find()
        .filter(device_placements::Column::OrgId.eq(org_id))
        .filter(device_placements::Column::PinId.is_in(pin_ids.to_vec()))
        .order_by_asc(device_placements::Column::CreatedAt)
        .all(db)
        .await
}
