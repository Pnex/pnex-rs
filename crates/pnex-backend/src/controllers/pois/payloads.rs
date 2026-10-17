use super::*;

// ─────────────────────────── payloads ───────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
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

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(super) struct PoiPatchPayload {
    pub(super) label: Option<String>,
    /// `null` resets to the default emoji.
    #[serde(default, deserialize_with = "deserialize_some")]
    #[schema(value_type = Option<String>)]
    pub(super) emoji: Option<Option<String>>,
    /// Absent = unchanged, `null` = cleared.
    #[serde(default, deserialize_with = "deserialize_some")]
    #[schema(value_type = Option<String>)]
    pub(super) location_detail: Option<Option<String>>,
    pub(super) latitude: Option<f64>,
    pub(super) longitude: Option<f64>,
    /// Absent = unchanged, `null` = cleared.
    #[serde(default, deserialize_with = "deserialize_some")]
    #[schema(value_type = Option<Object>)]
    pub(super) metadata: Option<Option<serde_json::Value>>,
    /// Absent = unchanged, `null` = cleared.
    #[serde(default, deserialize_with = "deserialize_some")]
    #[schema(value_type = Option<PoiPreviewKind>)]
    pub(super) preview_kind: Option<Option<String>>,
    /// Absent = unchanged, `null` = cleared.
    #[serde(default, deserialize_with = "deserialize_some")]
    #[schema(value_type = Option<String>)]
    pub(super) preview_id: Option<Option<String>>,
}

/// D43: body of `POST /pois/{id}/devices`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(super) struct DeviceAttachPayload {
    pub(super) device_id: String,
    pub(super) location_detail: Option<String>,
}

/// D43: body of `PATCH /pois/placements/{id}`: location and/or move
/// (detaching goes through `DELETE /pois/placements/{id}`).
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(super) struct PlacementPatchPayload {
    /// Absent = unchanged, `null` = cleared.
    #[serde(default, deserialize_with = "deserialize_some")]
    #[schema(value_type = Option<String>)]
    pub(super) location_detail: Option<Option<String>>,
    /// Move target POI; absent or `null` = no move (never a clear).
    #[schema(value_type = Option<String>, format = "uuid")]
    pub(super) pin_id: Option<Uuid>,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct PoiListQuery {
    pub(super) search: Option<String>,
    pub(super) emoji: Option<String>,
    /// `true` = only POIs carrying a device.
    #[param(value_type = Option<bool>)]
    pub(super) has_device: Option<String>,
    /// `true` = only POIs whose device reports a GPS position.
    #[param(value_type = Option<bool>)]
    pub(super) has_position: Option<String>,
    /// Label filter `name` / `name:value` (D218).
    pub(super) label: Option<String>,
    #[param(value_type = Option<i64>)]
    pub(super) limit: Option<String>,
    #[param(value_type = Option<i64>)]
    pub(super) offset: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct ClusterQuery {
    /// Required bbox `west,south,east,north` (WGS84 degrees).
    pub(super) bbox: Option<String>,
    /// Required map zoom level (integer 0..22).
    #[param(value_type = Option<i32>)]
    pub(super) zoom: Option<String>,
    pub(super) search: Option<String>,
    pub(super) emoji: Option<String>,
    #[param(value_type = Option<bool>)]
    pub(super) has_device: Option<String>,
    #[param(value_type = Option<bool>)]
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
        ids: None,
    }
}
