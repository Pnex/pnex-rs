use super::*;

// ─────────────────────────── erreurs ───────────────────────────

// Error helpers carry a machine code (`pnex_core::err_codes` registry) so
// the frontend can resolve `err-<kebab>` at render time; the English text is
// the verbatim fallback for unregistered codes.
pub(super) fn forbidden(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

/// 400 per-field error shape: `{"<field>": "..."}`.
pub(super) fn field_status(field: &str, msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        format::json(serde_json::json!({ field: msg })),
    )
        .into_response()
}

/// Mapping des erreurs de service → HTTP (formes cohérentes flows/media).
pub(super) fn write_error_response(e: svc::VizWriteError) -> Response {
    use svc::VizWriteError as E;
    match e {
        E::LabelRequired => field_status("label", err_codes::FIELD_REQUIRED),
        E::LabelTooLong => field_status(
            "label",
            &format!("{}:255", err_codes::FIELD_MAX_LENGTH),
        ),
        E::LocationDetailTooLong => field_status(
            "location_detail",
            &format!("{}:255", err_codes::FIELD_MAX_LENGTH),
        ),
        E::EmojiTooLong => field_status(
            "emoji",
            &format!("{}:8", err_codes::FIELD_MAX_LENGTH),
        ),
        E::LatitudeInvalid => {
            field_status("latitude", "Latitude invalide (bornes WGS84 −90..90).")
        }
        E::LongitudeInvalid => {
            field_status("longitude", "Longitude invalide (bornes WGS84 −180..180).")
        }
        E::DeviceUnknown => {
            field_status("device_id", "Device inconnu pour cette organisation.")
        }
        // D43 : 409 porteur du placement courant — le front propose le
        // déplacement en connaissance de cause (jamais silencieux).
        // Raw JSON body (dedicated device_id/current_placement_id/… keys):
        // the shape is a frontend contract — only the code and the canonical
        // English description migrate.
        E::DeviceAlreadyPlaced(c) => (
            StatusCode::CONFLICT,
            format::json(serde_json::json!({
                "error": "poi-device-already-placed",
                "description": "This device is already placed on a POI — attaching it here moves it (confirmation required).",
                "device_id": c.device_id,
                "current_placement_id": c.current_placement_id,
                "current_pin_id": c.current_pin_id,
                "current_pin_label": c.current_pin_label,
            })),
        )
            .into_response(),
        E::PlacementUnknown => Error::NotFound.into_response(),
        E::PinUnknown => field_status("pin_id", "POI cible inconnu pour cette organisation."),
        E::LinkSourceInvalid => field_status("source", "Source de lien invalide."),
        E::LinkTargetInvalid => {
            field_status("target", "Cible de lien inconnue pour cette organisation.")
        }
        E::LinkConflict => (
            StatusCode::CONFLICT,
            format::json(serde_json::json!({
                "error": "poi-link-duplicate",
                "description": "A link already exists for this pair."
            })),
        )
            .into_response(),
        E::PreviewKindInvalid => field_status(
            "preview_kind",
            "Aperçu épinglé invalide (kind ∈ media_asset | dashboard | tour, posé avec preview_id).",
        ),
        E::Db => Error::InternalServerError.into_response(),
    }
}

/// Contenu de l'org, sinon 404 (masqué — école device_of_org).
pub(super) fn or_not_found<T>(row: Option<T>) -> Result<T> {
    row.ok_or(Error::NotFound)
}
