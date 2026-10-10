use super::*;

// ─────────────────────────── Aides ───────────────────────────

// Error helpers carry a machine code (`pnex_core::err_codes` registry) so
// the frontend can resolve `err-<kebab>` at render time; the English text is
// the verbatim fallback for unregistered codes.
pub(super) fn conflict(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::CONFLICT,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

/// 400 of a file whose content is not an allowed format, whose extension
/// lies about it, or that does not fit the asset's kind.
pub(crate) fn format_unsupported() -> Error {
    Error::CustomError(
        StatusCode::BAD_REQUEST,
        loco_rs::controller::ErrorDetail::new(
            pnex_core::err_codes::MEDIA_FORMAT_UNSUPPORTED,
            "File format not allowed, or its extension does not match its content".to_string(),
        ),
    )
}

pub(super) fn forbidden(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

pub(super) fn field_status(field: &str, msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        format::json(serde_json::json!({ field: msg })),
    )
        .into_response()
}

/// Asset de l'org courante, sinon None (→ 404 masquant le cross-org,
/// école device_of_org). Uuid en Path : PK UUID (école sites).
pub(super) async fn find_asset(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: Uuid,
) -> Result<Option<media_assets::Model>> {
    media_assets::Entity::find_by_id(id)
        .filter(media_assets::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|e| {
            tracing::error!("media : lecture asset {id} : {e}");
            Error::InternalServerError
        })
}

/// Versions d'un asset, numéros croissants.
pub(super) async fn versions_of(
    db: &DatabaseConnection,
    asset_id: Uuid,
) -> Result<Vec<media_versions::Model>> {
    media_versions::Entity::find()
        .filter(media_versions::Column::AssetId.eq(asset_id))
        .order_by_asc(media_versions::Column::VersionNumber)
        .all(db)
        .await
        .map_err(|e| {
            tracing::error!("media : lecture versions asset {asset_id} : {e}");
            Error::InternalServerError
        })
}
