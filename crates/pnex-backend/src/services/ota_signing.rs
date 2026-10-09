//! Ed25519 key of the instance that signs OTA images (SEC-18,
//! security-tiers.md EX-B1).
//!
//! The 32-byte seed is a **platform secret of the vault** (`org_secrets`,
//! `org_id = NULL`, name [`KEY_NAME`]): encrypted at rest by the keyring and
//! rewritten by the master key rotation like any other row. It is generated
//! on first use, under a cluster-wide lock so that two pods never mint two
//! keys. The public key is compiled into every firmware build
//! (`PNEX_OTA_PUBKEY`); each `OtaAvailable` carries the signature of
//! `pnex_core::ota_sig::signed_message`. Losing the row means every device
//! must be reflashed over USB: the key is never rotated automatically.
//!
//! Profile `open` (D150): the server holds the key. Signing outside the
//! builder process (EX-B7) and customer-held keys come with the later
//! profiles.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use rand::RngExt;
use sea_orm::DatabaseConnection;

use crate::services::db_lock::{ns, TenantLock};
use crate::services::secrets::store::{self, StoreError, Writer};
use crate::services::secrets::Keyring;

/// Vault name of the seed (platform secret).
pub const KEY_NAME: &str = "pnex-ota-signing-key";

#[derive(Debug, thiserror::Error)]
pub enum OtaKeyError {
    #[error("secrets keyring: {0}")]
    Keyring(#[from] crate::services::secrets::KeyringError),
    #[error("vault: {0}")]
    Store(#[from] StoreError),
    #[error("db: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("stored OTA signing key is not a base64 32-byte seed")]
    Malformed,
}

/// Signing key of the instance, created on first use.
pub async fn signing_key(
    db: &DatabaseConnection,
    ring: &Keyring,
) -> Result<SigningKey, OtaKeyError> {
    if let Some(key) = load(db, ring).await? {
        return Ok(key);
    }
    // First use: one pod mints the key, the others wait then read it.
    let lock = TenantLock::acquire(
        db,
        ns::OTA_SIGNING_KEY,
        0,
        std::time::Duration::from_secs(10),
    )
    .await?;
    let result = async {
        if let Some(key) = load(db, ring).await? {
            return Ok(key);
        }
        let mut seed = [0u8; 32];
        rand::rng().fill(&mut seed);
        store::create(
            db,
            ring,
            Writer {
                org_id: None,
                user_id: None,
            },
            KEY_NAME,
            Some("Ed25519 key signing the OTA images of this instance (SEC-18). Deleting it forces a USB reflash of every device."),
            &B64.encode(seed),
        )
        .await?;
        tracing::info!("OTA signing key created");
        Ok(SigningKey::from_bytes(&seed))
    }
    .await;
    lock.release().await;
    result
}

async fn load(db: &DatabaseConnection, ring: &Keyring) -> Result<Option<SigningKey>, OtaKeyError> {
    let Some(row) = store::find_by_name(db, None, KEY_NAME).await? else {
        return Ok(None);
    };
    let seed_b64 = store::reveal(db, ring, None, row.id).await?;
    let seed: [u8; 32] = B64
        .decode(seed_b64.trim())
        .ok()
        .and_then(|raw| raw.try_into().ok())
        .ok_or(OtaKeyError::Malformed)?;
    Ok(Some(SigningKey::from_bytes(&seed)))
}

/// Public key (hex) compiled into the firmware.
pub fn public_key_hex(key: &SigningKey) -> String {
    pnex_core::ota_sig::to_hex(key.verifying_key().as_bytes())
}

/// Signature (hex) of an image for `device_id` at `version`, `None` when
/// the stored digest is not a SHA-256 hex (the device then refuses the OTA).
pub fn sign_image(
    key: &SigningKey,
    device_id: &str,
    version: &str,
    sha256_hex: &str,
) -> Option<String> {
    let digest = pnex_core::ota_sig::parse_sha256_hex(sha256_hex)?;
    let msg = pnex_core::ota_sig::signed_message(device_id, version, &digest)?;
    Some(pnex_core::ota_sig::to_hex(&key.sign(&msg).to_bytes()))
}

/// Signature for an OTA order, logged and `None` on any failure: the order
/// is then not sent (the device would refuse an unsigned image anyway).
pub async fn sign_for_order(
    db: &DatabaseConnection,
    config: &loco_rs::config::Config,
    device_id: &str,
    version: &str,
    sha256_hex: &str,
) -> Option<String> {
    let key = match Keyring::from_config(config) {
        Ok(ring) => signing_key(db, &ring).await,
        Err(e) => Err(e.into()),
    };
    match key {
        Ok(key) => {
            let sig = sign_image(&key, device_id, version, sha256_hex);
            if sig.is_none() {
                tracing::error!(
                    device = device_id,
                    version,
                    "ota: stored sha256 is not a digest, image left unsigned"
                );
            }
            sig
        }
        Err(e) => {
            tracing::error!(device = device_id, "ota: signing key unavailable: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Verifier;

    #[test]
    fn signature_covers_device_version_and_digest() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let sha = "ab".repeat(32);
        let sig_hex = sign_image(&key, "dev-1", "42", &sha).unwrap();
        assert_eq!(sig_hex.len(), 128);
        let mut sig = [0u8; 64];
        for (i, b) in sig.iter_mut().enumerate() {
            *b = u8::from_str_radix(&sig_hex[i * 2..i * 2 + 2], 16).unwrap();
        }
        let sig = ed25519_dalek::Signature::from_bytes(&sig);
        let digest = pnex_core::ota_sig::parse_sha256_hex(&sha).unwrap();
        let msg = pnex_core::ota_sig::signed_message("dev-1", "42", &digest).unwrap();
        assert!(key.verifying_key().verify(&msg, &sig).is_ok());
        let other = pnex_core::ota_sig::signed_message("dev-1", "41", &digest).unwrap();
        assert!(key.verifying_key().verify(&other, &sig).is_err());
        assert_eq!(public_key_hex(&key).len(), 64);
        assert!(sign_image(&key, "dev-1", "42", "not-a-digest").is_none());
    }
}
