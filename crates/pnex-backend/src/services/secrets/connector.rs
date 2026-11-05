//! Connectors as vault consumers (data-pipelines.md D352).
//!
//! The password of a `connectors` row lives in the vault (`secret_id`);
//! a typed value goes to the dedicated secret `connector/<name>/auth`,
//! owned by its org. Decrypted in memory for the time of a connection only.

use pnex_core::{SecretConsumerKind, SecretFieldInput};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

use super::crypto::Keyring;
use super::store::{self, StoreError, Writer};

/// Field name of the key in `secret_usages`.
const FIELD: &str = "auth";

/// Name prefix of the secret dedicated to a connector.
pub fn dedicated_prefix(name: &str) -> String {
    format!("connector/{name}/")
}

/// Stores the password of connector `id` (`name`) and rewrites its usage.
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
            kind: SecretConsumerKind::Connector,
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

/// Drops the usage of a deleted connector and its dedicated secret.
pub async fn release<C: ConnectionTrait>(
    db: &C,
    org_id: Option<i64>,
    id: Uuid,
    name: &str,
) -> Result<(), StoreError> {
    store::release_consumer(
        db,
        org_id,
        SecretConsumerKind::Connector,
        &id.to_string(),
        &dedicated_prefix(name),
    )
    .await
}
