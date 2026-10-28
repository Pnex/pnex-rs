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

pub(super) fn field_status(field: &str, msg: &str) -> Response {
    crate::controllers::field_error(field, msg)
}

/// Asset of the current org, otherwise None (→ 404 masking cross-org,
/// device_of_org school). Uuid in Path: UUID PK (sites school).
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
            tracing::error!("media: read asset {id}: {e}");
            Error::InternalServerError
        })
}

/// Versions of an asset, ascending numbers.
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
            tracing::error!("media: read asset versions {asset_id}: {e}");
            Error::InternalServerError
        })
}
