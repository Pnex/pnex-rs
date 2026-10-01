//! Device placements on POIs (D43): attach, move, detach, batch load.

use super::*;

/// POI de l'org, sinon `None` (→ 404 masqué côté contrôleur).
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

/// Device du registre de l'org par slug, sinon `None`.
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

/// Attache un device à un POI. L'attache est **délibérée** : le device doit
/// exister dans l'org (`DeviceUnknown`) et ne pas avoir de placement
/// (`DeviceAlreadyPlaced` portant le POI courant — même si c'est CE POI :
/// le front reconnaît `current_pin_id == pin.id` et traite sans dialog).
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
    // Fenêtre de course garde-puis-insère (école D36) : le perdant tombe sur
    // la contrainte UNIQUE (→ Db/500) — c'est l'index qui garantit.
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
    /// absent = inchangé, null = effacé, valeur = posée (école UpdatePin).
    pub location_detail: Option<Option<&'a str>>,
    /// Déplacement : POI cible validé dans l'org. no-op (pin courant)
    /// autorisé — un device n'a qu'un placement, un move ne crée jamais de
    /// doublon.
    pub pin_id: Option<Uuid>,
}

/// Édition/déplacement d'un placement (DELETE = détachement, voir
/// [`delete_placement`] ; supprimer le POI cascade les placements côté DB).
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

/// Détachement d'un device (amendement D43, 2026-09-13) : le placement
/// part, le device redevient **libre** — plaçable sur n'importe quel POI
/// ensuite. La règle « au plus un POI à la fois » reste portée par l'UNIQUE
/// `device_registry_id`. `false` = placement introuvable dans l'org (404
/// masqué cross-org, école `delete_link`).
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

/// Placements d'un lot de POI (batch anti-N+1 — école `links_for_pins`),
/// triés par created_at asc (ordre d'attachement stable pour le drawer).
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
