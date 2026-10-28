//! Field validation and write-error types for the POI service (D35-D43).

use super::*;

// ─────────────────────────── errors ───────────────────────────

/// D43: placement conflict — everything the front needs to offer
/// the move knowingly (409).
#[derive(Debug)]
pub struct PlacementConflict {
    pub device_id: String,
    pub current_placement_id: i64,
    pub current_pin_id: Uuid,
    pub current_pin_label: String,
}

#[derive(Debug)]
pub enum VizWriteError {
    LabelRequired,
    LabelTooLong,
    LocationDetailTooLong,
    EmojiTooLong,
    /// latitude outside [-90, 90] or not finite.
    LatitudeInvalid,
    /// longitude outside [-180, 180] or not finite.
    LongitudeInvalid,
    /// `device_id` absent from the org's registry.
    DeviceUnknown,
    /// D43: the device already has a placement — the error carries everything
    /// the front needs for the move confirmation (never silent).
    DeviceAlreadyPlaced(PlacementConflict),
    /// PATCH placement not found in the org (→ masked 404).
    PlacementUnknown,
    /// Target POI of a move unknown in the org (→ 400 on field pin_id).
    PinUnknown,
    /// Unsupported link source_kind or source POI not found in the org.
    LinkSourceInvalid,
    /// Unsupported target_kind or target not found in the org.
    LinkTargetInvalid,
    /// Link already exists for this pair (duplicate D42 edge).
    LinkConflict,
    /// Pinned preview: kind not in the pinnable list, or kind without id (the
    /// pair is set/cleared together).
    PreviewKindInvalid,
    Db,
}

// ─────────────────────────── aides communes ───────────────────────────

pub(super) fn validate_label(label: Option<&str>) -> Result<String, VizWriteError> {
    let label = label
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .ok_or(VizWriteError::LabelRequired)?;
    if label.chars().count() > 255 {
        return Err(VizWriteError::LabelTooLong);
    }
    Ok(label.to_string())
}

/// Free location inside the building (« Bldg A — Floor 2 — Room 204 »).
pub(super) fn validate_location_detail(
    detail: Option<&str>,
) -> Result<Option<String>, VizWriteError> {
    match detail.map(str::trim).filter(|d| !d.is_empty()) {
        None => Ok(None),
        Some(d) if d.chars().count() <= 255 => Ok(Some(d.to_string())),
        Some(_) => Err(VizWriteError::LocationDetailTooLong),
    }
}

/// POI pictogram: ≤ 8 `char`s (a common emoji grapheme fits easily;
/// varchar(16) in the DB).
pub(super) fn validate_emoji(emoji: Option<&str>) -> Result<Option<String>, VizWriteError> {
    match emoji.map(str::trim).filter(|e| !e.is_empty()) {
        None => Ok(None),
        Some(e) if e.chars().count() <= 8 => Ok(Some(e.to_string())),
        Some(_) => Err(VizWriteError::EmojiTooLong),
    }
}

/// Bounded and finite lat/lon (hardening — the DB only checks the
/// geo/plan CHECK, not the WGS84 ranges).
pub(super) fn validate_lat(lat: f64) -> Result<Decimal, VizWriteError> {
    if !(-90.0..=90.0).contains(&lat) || !lat.is_finite() {
        return Err(VizWriteError::LatitudeInvalid);
    }
    Decimal::try_from(lat).map_err(|_| VizWriteError::LatitudeInvalid)
}

pub(super) fn validate_lon(lon: f64) -> Result<Decimal, VizWriteError> {
    if !(-180.0..=180.0).contains(&lon) || !lon.is_finite() {
        return Err(VizWriteError::LongitudeInvalid);
    }
    Decimal::try_from(lon).map_err(|_| VizWriteError::LongitudeInvalid)
}

pub fn d2f(d: Option<Decimal>) -> Option<f64> {
    d.and_then(|v| f64::try_from(v).ok())
}
