use super::*;

// ─────────────────────────── payloads ───────────────────────────

#[derive(Debug, Deserialize)]
pub(super) struct PoiPayload {
    pub(super) label: Option<String>,
    pub(super) emoji: Option<String>,
    pub(super) location_detail: Option<String>,
    pub(super) latitude: Option<f64>,
    pub(super) longitude: Option<f64>,
    pub(super) metadata: Option<serde_json::Value>,
}

/// Idiome `Option<Option<T>>` : absent → `None`, `null` → `Some(None)`,
/// valeur → `Some(Some(v))` (serde pur confond null et absent).
fn deserialize_some<'de, T, D>(d: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    serde::Deserialize::deserialize(d).map(Some)
}

#[derive(Debug, Deserialize)]
pub(super) struct PoiPatchPayload {
    pub(super) label: Option<String>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub(super) emoji: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub(super) location_detail: Option<Option<String>>,
    pub(super) latitude: Option<f64>,
    pub(super) longitude: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub(super) metadata: Option<Option<serde_json::Value>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub(super) preview_kind: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub(super) preview_id: Option<Option<String>>,
}

/// D43 : corps du POST `/pois/{id}/devices`.
#[derive(Debug, Deserialize)]
pub(super) struct DeviceAttachPayload {
    pub(super) device_id: String,
    pub(super) location_detail: Option<String>,
}

/// D43 : corps du PATCH `/pois/placements/{id}` — location et/ou move
/// (le retrait passe par DELETE `/pois/placements/{id}`, amendement
/// 2026-09-13).
#[derive(Debug, Deserialize)]
pub(super) struct PlacementPatchPayload {
    #[serde(default, deserialize_with = "deserialize_some")]
    pub(super) location_detail: Option<Option<String>>,
    /// Move : POI cible. absent ≡ null = pas de move (jamais un clear).
    pub(super) pin_id: Option<Uuid>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct PoiListQuery {
    pub(super) search: Option<String>,
    pub(super) emoji: Option<String>,
    /// "true" = uniquement les POI attachés à un device.
    pub(super) has_device: Option<String>,
    /// "true" = uniquement les POI dont le device transmet une position GPS.
    pub(super) has_position: Option<String>,
    pub(super) limit: Option<String>,
    pub(super) offset: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ClusterQuery {
    /// bbox `west,south,east,north` (degrés WGS84).
    pub(super) bbox: Option<String>,
    pub(super) zoom: Option<String>,
    pub(super) search: Option<String>,
    pub(super) emoji: Option<String>,
    pub(super) has_device: Option<String>,
    pub(super) has_position: Option<String>,
}

/// Filtres communs liste/cluster depuis les query params.
pub(super) fn pin_filters<'a>(
    search: &'a Option<String>,
    emoji: &'a Option<String>,
    has_device: &Option<String>,
    has_position: &Option<String>,
) -> svc::PinFilters<'a> {
    svc::PinFilters {
        search: search.as_deref(),
        emoji: emoji.as_deref(),
        has_device: has_device.as_deref().is_some_and(|v| v == "true"),
        has_position: has_position.as_deref().is_some_and(|v| v == "true"),
    }
}
