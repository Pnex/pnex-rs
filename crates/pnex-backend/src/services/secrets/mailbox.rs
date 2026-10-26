//! Mailboxes as vault consumers (mail.md D453).
//!
//! The password of a `mailboxes` row lives in the vault (`secret_id`);
//! a typed value goes to the dedicated secret `mailbox/<id>/password`,
//! owned by the mailbox org. Decrypted in memory for one connection only.

use pnex_core::{SecretConsumerKind, SecretFieldInput};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

use super::crypto::Keyring;
use super::store::{self, StoreError, Writer};

/// Field name of the password in `secret_usages`.
const FIELD: &str = "password";

/// Name prefix of the secret dedicated to a mailbox (by id: names change).
pub fn dedicated_prefix(id: &str) -> String {
    format!("mailbox/{id}/")
}

/// Stores the password of mailbox `id` and rewrites its usage.
/// `input = None` keeps `current`. Returns the secret now referenced.
pub async fn save<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    writer: Writer,
    can_write_secrets: bool,
    id: Uuid,
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
            kind: SecretConsumerKind::Mailbox,
            id: &consumer_id,
            field: FIELD,
            prefix: dedicated_prefix(&consumer_id),
            previous_prefix: None,
        },
        current,
        input,
    )
    .await
}

/// Drops the usage of a deleted mailbox and its dedicated secret.
pub async fn release<C: ConnectionTrait>(
    db: &C,
    org_id: Option<i64>,
    id: Uuid,
) -> Result<(), StoreError> {
    let consumer_id = id.to_string();
    store::release_consumer(
        db,
        org_id,
        SecretConsumerKind::Mailbox,
        &consumer_id,
        &dedicated_prefix(&consumer_id),
    )
    .await
}
