use super::*;

/// Conflict with the machine code + canonical English description
/// (frontend resolves `err-<code>` at render time, verbatim fallback).
pub(super) fn conflict(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::CONFLICT,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

/// Conflict carrying interpolation data in `errors.args`.
fn conflict_args(code: &str, msg: &str, args: serde_json::Value) -> Error {
    Error::CustomError(
        StatusCode::CONFLICT,
        loco_rs::controller::ErrorDetail {
            error: Some(code.to_string()),
            description: Some(msg.to_string()),
            errors: Some(serde_json::json!({ "args": args })),
        },
    )
}

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

/// Mapping service → HTTP : formes de réponse **historiques** conservées
/// (les tests d'intégration en sont la référence) — 400 champ-par-champ
/// `{"name": ...}`, 400 `{"violations": [...]}`, 409 conflit, 500.
pub(super) fn flow_write_error_response(e: crate::services::flow::FlowWriteError) -> Response {
    use crate::services::flow::FlowWriteError as E;
    match e {
        E::NameRequired => field_status("name", pnex_core::err_codes::FIELD_REQUIRED),
        E::NameTooLong => field_status(
            "name",
            &format!("{}:200", pnex_core::err_codes::FIELD_MAX_LENGTH),
        ),
        E::DeviceUnknown => field_status("device_id", "Device inconnu pour cette organisation."),
        E::Violations(v) => (
            StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({ "violations": v })),
        )
            .into_response(),
        E::Conflict { expected, current } => conflict_args(
            "flow-stale-version",
            "Stale version — reload the latest version before saving.",
            serde_json::json!({
                "expected": expected.to_string(),
                "current": current.to_string(),
            }),
        )
        .into_response(),
        // Only the assistant's writer refuses a deployed flow; never an
        // HTTP save. Kept total for the match.
        E::Db | E::Deployed => Error::InternalServerError.into_response(),
        E::Secret(e) => match crate::controllers::secrets::store_error(e) {
            Ok(r) => r,
            Err(e) => e.into_response(),
        },
    }
}
