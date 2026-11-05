//! Image registries as vault consumers (agent-environments.md D703).
//!
//! Each registry holds up to two secrets, read and write. A typed value goes to the
//! dedicated secret `image-registry/<id>/<field>`: named by id, so a rename touches
//! nothing. Decrypted in memory for the time of a test only.

use pnex_core::{SecretConsumerKind, SecretFieldInput};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

use super::crypto::Keyring;
use super::store::{self, StoreError, Writer};

pub const READ: &str = "read";
pub const WRITE: &str = "write";

fn prefix(id: Uuid) -> String {
    format!("image-registry/{id}/")
}

/// One secret field: what the registry holds now, and what the request gives.
pub struct Field<'a> {
    pub name: &'static str,
    pub current: Option<Uuid>,
    pub input: Option<&'a SecretFieldInput>,
}

/// Stores one field. `None` keeps the current secret. Picking another secret stays
/// reserved to who may write secrets (strict R9, as in `store::save_single`).
pub async fn resolve<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    writer: Writer,
    can_write_secrets: bool,
    id: Uuid,
    field: &Field<'_>,
) -> Result<Option<Uuid>, StoreError> {
    let Some(input) = field.input else {
        return Ok(field.current);
    };
    if !can_write_secrets {
        if let SecretFieldInput::Pick { secret_id } = input {
            if Some(*secret_id) != field.current {
                return Err(StoreError::DestinationLocked {
                    field: field.name.to_string(),
                });
            }
        }
    }
    let dedicated = format!("{}{}", prefix(id), field.name);
    store::resolve_field(db, ring, writer, can_write_secrets, input, &dedicated)
        .await
        .map(Some)
}

/// Rewrites the usages of the registry, then drops a dedicated secret it let go.
pub async fn commit<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
    read: (Option<Uuid>, Option<Uuid>),
    write: (Option<Uuid>, Option<Uuid>),
) -> Result<(), StoreError> {
    let refs: Vec<(String, Uuid)> = [(READ, read.1), (WRITE, write.1)]
        .into_iter()
        .filter_map(|(field, secret)| secret.map(|s| (field.to_string(), s)))
        .collect();
    let consumer = id.to_string();
    store::set_usages(db, SecretConsumerKind::ImageRegistry, &consumer, &refs).await?;
    let dropped: Vec<Uuid> = [read, write]
        .into_iter()
        .filter_map(|(old, new)| old.filter(|o| Some(*o) != new))
        .collect();
    store::prune_dedicated(db, Some(org_id), &dropped, &prefix(id)).await
}

/// Drops the usages of a deleted registry and its dedicated secrets.
pub async fn release<C: ConnectionTrait>(db: &C, org_id: i64, id: Uuid) -> Result<(), StoreError> {
    store::release_consumer(
        db,
        Some(org_id),
        SecretConsumerKind::ImageRegistry,
        &id.to_string(),
        &prefix(id),
    )
    .await
}
