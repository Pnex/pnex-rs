//! Geo providers as vault consumers (geo-layers.md L19).
//!
//! The credential of a `geo_providers` row lives in the vault (`secret_id`);
//! a typed value goes to the dedicated secret `geo/<provider>/auth`,
//! owned by the provider's org. Decrypted in memory by the proxy only (L20).

use pnex_core::{SecretConsumerKind, SecretFieldInput};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

use super::crypto::Keyring;
use super::store::{self, StoreError, Writer};

/// Field name of the key in `secret_usages`.
const FIELD: &str = "auth";

/// Name prefix of the secret dedicated to a provider.
pub fn dedicated_prefix(provider_name: &str) -> String {
    format!("geo/{provider_name}/")
}

/// Stores the key of provider `id` (`name`) and rewrites its usage.
/// `input = None` keeps `current`; `previous_name`: the dedicated secret
/// follows a rename. Returns the secret now referenced.
#[allow(clippy::too_many_arguments)] // call sites stay explicit
pub async fn save<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    writer: Writer,
    can_write_secrets: bool,
    id: Uuid,
    name: &str,
    previous_name: Option<&str>,
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
            kind: SecretConsumerKind::GeoProvider,
            id: &consumer_id,
            field: FIELD,
            prefix: dedicated_prefix(name),
            previous_prefix: previous_name.map(dedicated_prefix),
        },
        current,
        input,
    )
    .await
}

/// Drops the usage of a deleted provider and its dedicated secret.
pub async fn release<C: ConnectionTrait>(
    db: &C,
    org_id: Option<i64>,
    id: Uuid,
    name: &str,
) -> Result<(), StoreError> {
    store::release_consumer(
        db,
        org_id,
        SecretConsumerKind::GeoProvider,
        &id.to_string(),
        &dedicated_prefix(name),
    )
    .await
}
