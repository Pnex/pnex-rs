//! WiFi credentials as vault consumers (secrets.md lot S6).
//!
//! The password of a `wifi_credentials` row lives in the vault
//! (`secret_id`); a typed value goes to the dedicated secret
//! `wifi/<ssid>/password`. Builds reference the credential: the queued job
//! only carries the secret id and the build worker decrypts it at run
//! time, like it reads the device token from the database (D118: changing
//! the value rebuilds nothing, flashed devices keep the old one).

use pnex_core::{SecretConsumerKind, SecretFieldInput};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

use super::crypto::Keyring;
use super::store::{self, StoreError, Writer};

/// Field name of the password in `secret_usages`.
const FIELD: &str = "password";

/// Name prefix of the secret dedicated to a WiFi entry.
pub fn dedicated_prefix(ssid: &str) -> String {
    format!("wifi/{ssid}/")
}

/// Stores the password of credential `id` (`ssid`) and rewrites its usage.
/// `input = None` keeps `current`. `previous_ssid`: the dedicated secret
/// follows a rename. Returns the secret now referenced.
#[allow(clippy::too_many_arguments)] // call sites stay explicit
pub async fn save<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    writer: Writer,
    can_write_secrets: bool,
    id: i64,
    ssid: &str,
    previous_ssid: Option<&str>,
    current: Option<Uuid>,
    input: Option<&SecretFieldInput>,
) -> Result<Option<Uuid>, StoreError> {
    let consumer_id = id.to_string();
    store::save_single(
        db,
        ring,
        writer,
        can_write_secrets,
        &store::SingleConsumer {
            kind: SecretConsumerKind::Wifi,
            id: &consumer_id,
            field: FIELD,
            prefix: dedicated_prefix(ssid),
            previous_prefix: previous_ssid.map(dedicated_prefix),
        },
        current,
        input,
    )
    .await
}

/// Drops the usage of a deleted credential and its dedicated secret.
pub async fn release<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: i64,
    ssid: &str,
) -> Result<(), StoreError> {
    store::release_consumer(
        db,
        Some(org_id),
        SecretConsumerKind::Wifi,
        &id.to_string(),
        &dedicated_prefix(ssid),
    )
    .await
}

/// The password for a build (worker side). Memory only.
pub async fn reveal<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    org_id: i64,
    secret_id: Uuid,
) -> Result<String, StoreError> {
    store::reveal(db, ring, Some(org_id), secret_id).await
}
