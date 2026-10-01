//! LLM providers as vault consumers (secrets.md lot S7, D116).
//!
//! The API key of an `llm_providers` row lives in the vault (`secret_id`);
//! a typed value goes to the dedicated secret `llm/<provider>/api_key`,
//! owned by the provider's org (D119). The key is decrypted in memory for
//! each LLM call.

use pnex_core::{SecretConsumerKind, SecretFieldInput};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

use super::crypto::Keyring;
use super::store::{self, StoreError, Writer};

/// Field name of the key in `secret_usages`.
const FIELD: &str = "api_key";

/// Name prefix of the secret dedicated to a provider.
pub fn dedicated_prefix(provider_name: &str) -> String {
    format!("llm/{provider_name}/")
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
            kind: SecretConsumerKind::LlmProvider,
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
        SecretConsumerKind::LlmProvider,
        &id.to_string(),
        &dedicated_prefix(name),
    )
    .await
}
