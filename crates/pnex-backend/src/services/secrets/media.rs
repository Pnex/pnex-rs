//! Media streams as vault consumers (media-ingest.md D159).
//!
//! The credential of a `media_streams` row (HTTP header or basic auth)
//! lives in the vault (`secret_id`); a typed value goes to the dedicated
//! secret `media/<slug>/auth`. The slug is frozen at creation, so the
//! dedicated secret never follows a rename. The value is decrypted in
//! memory by the capture only, and sent to the URL's origin only (R9).

use pnex_core::{SecretConsumerKind, SecretFieldInput};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

use super::crypto::Keyring;
use super::store::{self, StoreError, Writer};

/// Field name of the credential in `secret_usages`.
const FIELD: &str = "auth";

/// Name prefix of the secret dedicated to a stream.
pub fn dedicated_prefix(slug: &str) -> String {
    format!("media/{slug}/")
}

/// Stores the credential of stream `id` (`slug`) and rewrites its usage.
/// `input = None` keeps `current`. Returns the secret now referenced.
#[allow(clippy::too_many_arguments)] // call sites stay explicit
pub async fn save<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    writer: Writer,
    can_write_secrets: bool,
    id: Uuid,
    slug: &str,
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
            kind: SecretConsumerKind::MediaStream,
            id: &consumer_id,
            field: FIELD,
            prefix: dedicated_prefix(slug),
            previous_prefix: None,
        },
        current,
        input,
    )
    .await
}

/// Drops the usage of a deleted stream (or of a cleared credential) and
/// its dedicated secret.
pub async fn release<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
    slug: &str,
) -> Result<(), StoreError> {
    store::release_consumer(
        db,
        Some(org_id),
        SecretConsumerKind::MediaStream,
        &id.to_string(),
        &dedicated_prefix(slug),
    )
    .await
}
