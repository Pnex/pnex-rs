use super::*;
use rand::RngExt;

fn random_bytes(n: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; n];
    rand::rng().fill(&mut bytes);
    bytes
}

/// Parité `secrets.token_urlsafe(32)`.
pub(crate) fn generate_token() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes(32))
}

/// Parité `crypto_utils.generate_device_key` — base64 standard (44 chars).
pub(crate) fn generate_device_key() -> String {
    STANDARD.encode(random_bytes(32))
}

/// Device token: reactivated if it exists inactive, generated if missing
/// (parity with the legacy `get_or_create` on reactivation).
pub(super) async fn ensure_token(
    db: &sea_orm::DatabaseTransaction,
    device: &device_registries::Model,
) -> Result<device_tokens::Model> {
    if let Some(existing) = device_tokens::Entity::find()
        .filter(device_tokens::Column::DeviceRegistryId.eq(device.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
    {
        if !existing.is_active {
            let mut active: device_tokens::ActiveModel = existing.into();
            active.is_active = Set(true);
            return active
                .update(db)
                .await
                .map_err(|_| Error::InternalServerError);
        }
        return Ok(existing);
    }
    device_tokens::ActiveModel {
        token: Set(generate_token()),
        encryption_key: Set(generate_device_key()),
        is_active: Set(true),
        device_registry_id: Set(device.id),
        ..Default::default()
    }
    .insert(db)
    .await
    .map_err(|_| Error::InternalServerError)
}
