use super::*;

// ─────────────────────────── errors ───────────────────────────

// Error helpers carry a machine code (`pnex_core::err_codes` registry) so
// the frontend can resolve `err-<kebab>` at render time; the English text is
// the verbatim fallback for unregistered codes.

/// `field-invalid` 400 on one field.
pub(super) fn field_status(field: &str, msg: &str) -> Response {
    crate::controllers::field_error(field, msg)
}

/// Mapping of service errors → HTTP (shapes consistent with flows/media).
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
        E::LatitudeInvalid => field_status("latitude", err_codes::FIELD_INVALID),
        E::LongitudeInvalid => field_status("longitude", err_codes::FIELD_INVALID),
        E::DeviceUnknown => field_status("device_id", err_codes::FIELD_NOT_FOUND),
        // D43: 409 carrying the current placement in `errors.args` (strings)
        // so the UI offers the move knowingly (never silent).
        E::DeviceAlreadyPlaced(c) => crate::controllers::coded_error(
            StatusCode::CONFLICT,
            err_codes::POI_DEVICE_ALREADY_PLACED,
            "This device is already placed on a POI; attaching it here moves it (confirmation required).",
            Some(serde_json::json!({
                "device_id": c.device_id,
                "current_placement_id": c.current_placement_id.to_string(),
                "current_pin_id": c.current_pin_id.to_string(),
                "current_pin_label": c.current_pin_label,
            })),
        )
        .into_response(),
        E::PlacementUnknown => Error::NotFound.into_response(),
        E::PinUnknown => field_status("pin_id", err_codes::FIELD_NOT_FOUND),
        E::LinkSourceInvalid => field_status("source", err_codes::FIELD_INVALID),
        E::LinkTargetInvalid => field_status("target", err_codes::FIELD_NOT_FOUND),
        E::LinkConflict => crate::controllers::coded_error(
            StatusCode::CONFLICT,
            err_codes::POI_LINK_DUPLICATE,
            "A link already exists for this pair.",
            None,
        )
        .into_response(),
        E::PreviewKindInvalid => field_status("preview_kind", err_codes::FIELD_INVALID),
        E::Db => Error::InternalServerError.into_response(),
    }
}

/// Org content, otherwise 404 (hidden — device_of_org pattern).
pub(super) fn or_not_found<T>(row: Option<T>) -> Result<T> {
    row.ok_or(Error::NotFound)
}
