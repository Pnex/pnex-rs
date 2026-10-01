use super::*;

// ─────────────────────────── DTO ───────────────────────────

fn d2f(d: Option<sea_orm::entity::prelude::Decimal>) -> Option<f64> {
    d.and_then(|v| f64::try_from(v).ok())
}

/// Noms d'affichage des cibles d'arêtes, clé `(target_kind, target_id)`
/// (batch [`svc::link_target_labels`]).
pub(super) type TargetLabels = std::collections::HashMap<(String, String), Option<String>>;

#[derive(Serialize)]
pub(super) struct LinkDto {
    id: i64,
    source_kind: String,
    source_id: String,
    target_kind: String,
    target_id: String,
    /// Sac de métadonnées de l'arête (hotspot coords, `label`, overrides).
    placement: Option<serde_json::Value>,
    /// Nom résolu de la cible (batch org) — `None` si morte ou hydratation
    /// en échec.
    target_label: Option<String>,
    /// Cible disparue des tables (supprimée hors cascade) — le front
    /// désactive la ligne (D26 : tolérée au rendu, jamais panic).
    target_dead: bool,
}

pub(super) fn link_dto(
    l: &crate::models::_entities::resource_edges::Model,
    labels: Option<&TargetLabels>,
) -> LinkDto {
    // `labels` absente = hydratation en échec → ligne « grise » (label None,
    // dead false — jamais « supprimée » par précaution) ; clé absente de la
    // map résolue = cible introuvable dans l'org → `target_dead`.
    let resolved = labels.and_then(|m| m.get(&(l.target_kind.clone(), l.target_id.clone())));
    let (target_label, target_dead) = match (labels.is_some(), resolved) {
        (_, Some(Some(name))) => (Some(name.clone()), false),
        (_, Some(None)) => (None, true),
        (true, None) => (None, true),
        (false, None) => (None, false),
    };
    LinkDto {
        id: l.id,
        source_kind: l.source_kind.clone(),
        source_id: l.source_id.clone(),
        target_kind: l.target_kind.clone(),
        target_id: l.target_id.clone(),
        placement: l.placement.clone(),
        target_label,
        target_dead,
    }
}

/// D43 : placement d'un device sur le POI (1 placement par device).
#[derive(Serialize)]
pub(super) struct DevicePlacementDto {
    id: i64,
    device_id: String,
    location_detail: Option<String>,
}

pub(super) fn placement_dto(pl: &device_placements::Model) -> DevicePlacementDto {
    DevicePlacementDto {
        id: pl.id,
        device_id: pl.device_id.clone(),
        location_detail: pl.location_detail.clone(),
    }
}

#[derive(Serialize)]
pub(super) struct PoiDto {
    id: Uuid,
    mode: String,
    latitude: Option<f64>,
    longitude: Option<f64>,
    /// Devices placés sur ce POI (D43 : plusieurs possibles).
    devices: Vec<DevicePlacementDto>,
    pub(super) label: String,
    pub(super) emoji: String,
    location_detail: Option<String>,
    /// Aperçu épinglé : objet attaché montré en priorité à
    /// l'ouverture du POI (kind ∈ media_asset | dashboard | tour).
    preview_kind: Option<String>,
    preview_id: Option<String>,
    metadata: Option<serde_json::Value>,
    /// Liens dont ce POI est la source (résolus — D32 : zéro requête au clic).
    links: Vec<LinkDto>,
    created_at: String,
    updated_at: String,
}

pub(super) fn poi_dto(
    p: &map_pins::Model,
    placements: &[device_placements::Model],
    links: &[crate::models::_entities::resource_edges::Model],
    labels: Option<&TargetLabels>,
) -> PoiDto {
    PoiDto {
        id: p.id,
        mode: p.mode.clone(),
        latitude: d2f(p.latitude),
        longitude: d2f(p.longitude),
        devices: placements
            .iter()
            .filter(|pl| pl.pin_id == p.id)
            .map(placement_dto)
            .collect(),
        label: p.label.clone(),
        emoji: p
            .emoji
            .clone()
            .unwrap_or_else(|| svc::PIN_EMOJI_DEFAULT.to_string()),
        location_detail: p.location_detail.clone(),
        preview_kind: p.preview_kind.clone(),
        preview_id: p.preview_id.clone(),
        metadata: p.metadata.clone(),
        links: links
            .iter()
            .filter(|l| l.source_id == p.id.to_string())
            .map(|l| link_dto(l, labels))
            .collect(),
        created_at: p.created_at.to_rfc3339(),
        updated_at: p.updated_at.to_rfc3339(),
    }
}

/// Un item de la réponse cluster : point individuel (`count == 1`,
/// `id` renseigné) ou cluster numéroté (centroïde + échantillon).
#[derive(Serialize)]
pub(super) struct ClusterItemDto {
    pub(super) lat: f64,
    pub(super) lon: f64,
    pub(super) count: u32,
    pub(super) id: Option<Uuid>,
    pub(super) label: String,
    pub(super) emoji: String,
}

#[derive(Serialize)]
pub(super) struct ClusterResponse {
    pub(super) zoom: i32,
    /// Nombre de POI géo dans la bbox (pins individuels si ≤ seuil).
    pub(super) total: usize,
    pub(super) items: Vec<ClusterItemDto>,
}

#[derive(Serialize)]
pub(super) struct DevicePositionDto {
    device_id: String,
    latitude: f64,
    longitude: f64,
    altitude_m: Option<f64>,
    accuracy_m: Option<f64>,
    speed_mps: Option<f64>,
    heading_deg: Option<f64>,
    source: String,
    positioned_at: String,
}

pub(super) fn position_dto(p: &device_positions::Model) -> DevicePositionDto {
    DevicePositionDto {
        device_id: p.device_id.clone(),
        latitude: d2f(Some(p.latitude)).unwrap_or_default(),
        longitude: d2f(Some(p.longitude)).unwrap_or_default(),
        altitude_m: d2f(p.altitude_m),
        accuracy_m: d2f(p.accuracy_m),
        speed_mps: d2f(p.speed_mps),
        heading_deg: d2f(p.heading_deg),
        source: p.source.clone(),
        positioned_at: p.positioned_at.to_rfc3339(),
    }
}
