//! Browser endpoints as vault consumers (site-recipes.md D325).
//!
//! The token of a `browser_endpoints` row lives in the vault (`secret_id`);
//! a typed value goes to the dedicated secret `browser/<endpoint>/auth`,
//! owned by the endpoint's org. Decrypted in memory when connecting only.

use pnex_core::{SecretConsumerKind, SecretFieldInput};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

use super::crypto::Keyring;
use super::store::{self, StoreError, Writer};

/// Field name of the key in `secret_usages`.
const FIELD: &str = "auth";

/// Name prefix of the secret dedicated to an endpoint.
pub fn dedicated_prefix(endpoint_name: &str) -> String {
    format!("browser/{endpoint_name}/")
}

/// Stores the token of endpoint `id` (`name`) and rewrites its usage.
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
            kind: SecretConsumerKind::BrowserEndpoint,
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

/// Drops the usage of a deleted endpoint and its dedicated secret.
pub async fn release<C: ConnectionTrait>(
    db: &C,
    org_id: Option<i64>,
    id: Uuid,
    name: &str,
) -> Result<(), StoreError> {
    store::release_consumer(
        db,
        org_id,
        SecretConsumerKind::BrowserEndpoint,
        &id.to_string(),
        &dedicated_prefix(name),
    )
    .await
}
