//! Master key rotation (secrets.md lot S8, §7).
//!
//! Procedure: put the new key first in `PNEX_SECRETS_KEYS`, restart every
//! pod, then run [`rekey`] (platform admin button in /admin/status, or the
//! `secrets_rekey` task). It rewrites every row whose `key_id` is not the
//! write key; once [`KeyStats::stale`] is zero the old key can leave the
//! keyring. Rekeying before every pod knows the new key would make the
//! rewritten rows unreadable on the lagging pods: hence a manual step,
//! never a boot hook.
//!
//! A rekey changes neither the value nor `updated_at`/`updated_by`: the
//! row is rewritten with a conditional `UPDATE` on its previous nonce, so a
//! concurrent value change (which also draws a new nonce) is never
//! overwritten with the old value.

use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use uuid::Uuid;

use super::crypto::{Keyring, Sealed};
use super::store::StoreError;
use crate::models::_entities::org_secrets;
use crate::services::db_lock::{ns, TenantLock};

/// Rows read per round trip.
const BATCH: u64 = 200;

/// Vault rows by key, as seen by the current keyring.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyStats {
    pub total: u64,
    /// Rows under another key than the write key (rekey candidates).
    pub stale: u64,
    /// Rows under a key absent from the keyring: unreadable until that key
    /// comes back.
    pub unknown_key: u64,
}

/// Outcome of one [`rekey`] run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RekeyReport {
    pub rewritten: u64,
    /// Rows under a key absent from the keyring, or failing to decrypt:
    /// left untouched (ids are logged, never values).
    pub unreadable: u64,
    /// Rows changed by someone else while being rekeyed: already under the
    /// write key, nothing to do.
    pub skipped: u64,
}

/// Counts the vault rows per key class.
pub async fn stats<C: ConnectionTrait>(db: &C, ring: &Keyring) -> Result<KeyStats, StoreError> {
    let total = org_secrets::Entity::find().count(db).await?;
    let stale = org_secrets::Entity::find()
        .filter(org_secrets::Column::KeyId.ne(ring.primary_id()))
        .count(db)
        .await?;
    let known: Vec<String> = ring.key_ids().map(str::to_string).collect();
    let unknown_key = org_secrets::Entity::find()
        .filter(org_secrets::Column::KeyId.is_not_in(known))
        .count(db)
        .await?;
    Ok(KeyStats {
        total,
        stale,
        unknown_key,
    })
}

/// Rewrites every row not under the write key with the write key.
/// Idempotent; walks the rows by id so an unreadable row cannot loop.
pub async fn rekey<C: ConnectionTrait>(db: &C, ring: &Keyring) -> Result<RekeyReport, StoreError> {
    let mut report = RekeyReport::default();
    let mut after: Option<Uuid> = None;
    loop {
        let mut query = org_secrets::Entity::find()
            .filter(org_secrets::Column::KeyId.ne(ring.primary_id()))
            .order_by_asc(org_secrets::Column::Id)
            .limit(BATCH);
        if let Some(last) = after {
            query = query.filter(org_secrets::Column::Id.gt(last));
        }
        let rows = query.all(db).await?;
        let Some(last) = rows.last() else {
            break;
        };
        after = Some(last.id);
        for row in rows {
            let previous = Sealed {
                ciphertext: row.ciphertext,
                nonce: row.nonce,
                key_id: row.key_id,
            };
            let value = match ring.open(row.org_id, row.id, &previous) {
                Ok(value) => value,
                Err(e) => {
                    tracing::warn!(secret_id = %row.id, key_id = %previous.key_id, error = %e,
                        "secrets rekey: row left as is");
                    report.unreadable += 1;
                    continue;
                }
            };
            let sealed = ring.seal(row.org_id, row.id, &value);
            let done = org_secrets::Entity::update_many()
                .col_expr(
                    org_secrets::Column::Ciphertext,
                    Expr::value(sealed.ciphertext),
                )
                .col_expr(org_secrets::Column::Nonce, Expr::value(sealed.nonce))
                .col_expr(org_secrets::Column::KeyId, Expr::value(sealed.key_id))
                .filter(org_secrets::Column::Id.eq(row.id))
                .filter(org_secrets::Column::KeyId.eq(previous.key_id))
                .filter(org_secrets::Column::Nonce.eq(previous.nonce))
                .exec(db)
                .await?;
            if done.rows_affected == 1 {
                report.rewritten += 1;
            } else {
                report.skipped += 1;
            }
        }
    }
    Ok(report)
}

/// [`rekey`] as a cluster-wide singleton: a second run waits for the first
/// (bounded), then finds nothing left to do.
pub async fn rekey_singleton(
    db: &DatabaseConnection,
    ring: &Keyring,
) -> Result<RekeyReport, StoreError> {
    let lock =
        TenantLock::acquire(db, ns::SECRETS_REKEY, 0, std::time::Duration::from_secs(30)).await?;
    let report = rekey(db, ring).await;
    lock.release().await;
    let report = report?;
    tracing::info!(
        rewritten = report.rewritten,
        unreadable = report.unreadable,
        skipped = report.skipped,
        write_key = ring.primary_id(),
        "secrets rekey done"
    );
    Ok(report)
}
