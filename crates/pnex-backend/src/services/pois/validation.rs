//! Field validation and write-error types for the POI service (D35-D43).

use super::*;

// ─────────────────────────── erreurs ───────────────────────────

/// D43 : conflit de placement — tout ce qu'il faut au front pour proposer
/// le déplacement en connaissance de cause (409).
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
    /// latitude hors [-90, 90] ou non finie.
    LatitudeInvalid,
    /// longitude hors [-180, 180] ou non finie.
    LongitudeInvalid,
    /// `device_id` absent du registre de l'org.
    DeviceUnknown,
    /// D43 : le device a déjà un placement — l'erreur porte tout ce qu'il
    /// faut au front pour la confirmation de déplacement (jamais silencieux).
    DeviceAlreadyPlaced(PlacementConflict),
    /// PATCH placement introuvable dans l'org (→ 404 masqué).
    PlacementUnknown,
    /// POI cible d'un déplacement inconnu dans l'org (→ 400 champ pin_id).
    PinUnknown,
    /// source_kind de lien non supporté ou POI source introuvable dans l'org.
    LinkSourceInvalid,
    /// target_kind non supporté ou cible introuvable dans l'org.
    LinkTargetInvalid,
    /// Lien déjà existant pour cette paire (arête D42 en double).
    LinkConflict,
    /// Aperçu épinglé : kind hors liste épinglable, ou kind sans id (le
    /// couple est posé/cleared ensemble).
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

/// Localisation libre dans le bâtiment (« Bât. A — Étage 2 — Salle 204 »).
pub(super) fn validate_location_detail(
    detail: Option<&str>,
) -> Result<Option<String>, VizWriteError> {
    match detail.map(str::trim).filter(|d| !d.is_empty()) {
        None => Ok(None),
        Some(d) if d.chars().count() <= 255 => Ok(Some(d.to_string())),
        Some(_) => Err(VizWriteError::LocationDetailTooLong),
    }
}

/// Pictogramme POI : ≤ 8 `char`s (un graphème emoji courant tient largement ;
/// varchar(16) en DB).
pub(super) fn validate_emoji(emoji: Option<&str>) -> Result<Option<String>, VizWriteError> {
    match emoji.map(str::trim).filter(|e| !e.is_empty()) {
        None => Ok(None),
        Some(e) if e.chars().count() <= 8 => Ok(Some(e.to_string())),
        Some(_) => Err(VizWriteError::EmojiTooLong),
    }
}

/// lat/lon bornés et finis (hardening — la DB ne vérifie que le CHECK
/// geo/plan, pas les plages WGS84).
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
