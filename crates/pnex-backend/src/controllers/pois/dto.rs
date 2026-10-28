use super::*;

// ─────────────────────────── DTO ───────────────────────────

fn d2f(d: Option<sea_orm::entity::prelude::Decimal>) -> Option<f64> {
    d.and_then(|v| f64::try_from(v).ok())
}

/// Display names of edge targets, key `(target_kind, target_id)`
/// (batch [`svc::link_target_labels`]).
pub(super) type TargetLabels = std::collections::HashMap<(String, String), Option<String>>;

#[derive(Serialize, utoipa::ToSchema)]
pub(super) struct LinkDto {
    id: i64,
    source_kind: String,
    source_id: String,
    target_kind: String,
    target_id: String,
    /// Edge metadata bag (hotspot coords, `label`, overrides).
    placement: Option<serde_json::Value>,
    /// Resolved target name; `None` when the target is dead or resolution failed.
    target_label: Option<String>,
    /// The target no longer exists (deleted outside the cascade).
    target_dead: bool,
}

pub(super) fn link_dto(
    l: &crate::models::_entities::resource_edges::Model,
    labels: Option<&TargetLabels>,
) -> LinkDto {
    // `labels` absent = hydration failed → "grey" row (label None,
    // dead false — never "deleted" out of caution); key absent from the
    // resolved map = target not found in the org → `target_dead`.
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

/// D43: placement of a device on a POI (one placement per device).
#[derive(Serialize, utoipa::ToSchema)]
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

/// Placement mode of a POI (geographic only for now).
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum PoiMode {
    Geo,
}

/// Kind of the pinned preview of a POI.
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum PoiPreviewKind {
    MediaAsset,
    Dashboard,
    Tour,
}

impl PoiPreviewKind {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "media_asset" => Some(Self::MediaAsset),
            "dashboard" => Some(Self::Dashboard),
            "tour" => Some(Self::Tour),
            _ => None,
        }
    }
}

/// Origin of a device position.
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum PositionSource {
    Telemetry,
    Manual,
}

#[derive(Serialize, utoipa::ToSchema)]
pub(super) struct PoiDto {
    #[schema(value_type = String, format = "uuid")]
    id: Uuid,
    mode: PoiMode,
    latitude: Option<f64>,
    longitude: Option<f64>,
    /// Devices placed on this POI (D43: several allowed).
    devices: Vec<DevicePlacementDto>,
    pub(super) label: String,
    pub(super) emoji: String,
    location_detail: Option<String>,
    /// Pinned preview: attached object shown first when the POI opens
    /// (kind ∈ media_asset | dashboard | tour).
    preview_kind: Option<PoiPreviewKind>,
    preview_id: Option<String>,
    metadata: Option<serde_json::Value>,
    /// Links whose source is this POI, with resolved targets (D32).
    links: Vec<LinkDto>,
    #[schema(format = DateTime)]
    created_at: String,
    #[schema(format = DateTime)]
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
        // `map_pins.mode` only ever holds `geo` (V1).
        mode: PoiMode::Geo,
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
        preview_kind: p.preview_kind.as_deref().and_then(PoiPreviewKind::parse),
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

/// Cluster answer item: single POI (`count == 1`, `id` set) or numbered
/// cluster (centroid + sample label).
#[derive(Serialize, utoipa::ToSchema)]
pub(super) struct ClusterItemDto {
    pub(super) lat: f64,
    pub(super) lon: f64,
    pub(super) count: u32,
    #[schema(value_type = Option<String>, format = "uuid")]
    pub(super) id: Option<Uuid>,
    pub(super) label: String,
    pub(super) emoji: String,
    /// Colour token of the pin's coloured status (`green`, `amber`…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) color: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub(super) struct ClusterResponse {
    pub(super) zoom: i32,
    /// Number of POIs in the bbox.
    pub(super) total: usize,
    pub(super) items: Vec<ClusterItemDto>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub(super) struct DevicePositionDto {
    device_id: String,
    latitude: f64,
    longitude: f64,
    altitude_m: Option<f64>,
    accuracy_m: Option<f64>,
    speed_mps: Option<f64>,
    heading_deg: Option<f64>,
    source: PositionSource,
    #[schema(format = DateTime)]
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
        source: if p.source == "manual" {
            PositionSource::Manual
        } else {
            PositionSource::Telemetry
        },
        positioned_at: p.positioned_at.to_rfc3339(),
    }
}
