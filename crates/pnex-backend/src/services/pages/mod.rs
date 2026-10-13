//! Pages (`docs/architecture/pages.md`, decision #21): storage of the Yjs
//! state of a page and its named versions. The live collaboration (rooms,
//! websocket protocol, cross-pod fan-out) is in [`room`].
//!
//! Storage (P4): `docs.snapshot` (compacted state) + `doc_updates`
//! (append-only log). The database is the source of truth: a pod persists
//! an update **before** applying or relaying it, so any pod rebuilds the
//! page from the database alone. Applying an update twice is a no-op in a
//! CRDT, which keeps loading and catch-up simple (no sequence bookkeeping).

pub mod flow_write;
pub mod pdf;
pub mod room;
pub mod schema;
pub mod seed;

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait,
};
use uuid::Uuid;
use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

use crate::models::_entities::{doc_updates, doc_versions, docs};

/// Updates logged since the snapshot before a compaction is triggered.
pub const COMPACT_AFTER: u64 = 500;

/// Error of the storage layer.
#[derive(Debug, thiserror::Error)]
pub enum PageError {
    #[error(transparent)]
    Db(#[from] DbErr),
    #[error("stored page state is unreadable: {0}")]
    Corrupt(String),
    /// A server-side write refused by the size or schema checks.
    #[error("refused page content: {0}")]
    Refused(String),
}

/// Valkey URL of the cross-pod fan-out, `None` on a single node.
pub fn valkey_url(config: &loco_rs::config::Config) -> Option<String> {
    crate::services::last_cache::ValkeySettings::from_config(config)
        .and_then(|s| s.url)
        .filter(|u| !u.trim().is_empty())
}

/// Applies a v1-encoded update to `doc`.
pub fn apply(doc: &Doc, update: &[u8]) -> Result<(), PageError> {
    let u = Update::decode_v1(update).map_err(|e| PageError::Corrupt(e.to_string()))?;
    doc.transact_mut()
        .apply_update(u)
        .map_err(|e| PageError::Corrupt(e.to_string()))
}

/// Full state of `doc`, v1 encoding.
pub fn encode(doc: &Doc) -> Vec<u8> {
    doc.transact()
        .encode_state_as_update_v1(&StateVector::default())
}

/// Rebuilds the page from the database: snapshot, then every logged update.
/// Read under a shared row lock so a compaction (exclusive lock) cannot
/// commit between the two reads (old snapshot + already folded log).
pub async fn load(db: &DatabaseConnection, doc_id: Uuid) -> Result<Doc, PageError> {
    let txn = db.begin().await?;
    let doc = load_locked(&txn, doc_id, true).await?;
    txn.commit().await?;
    Ok(doc)
}

/// [`load`] inside the caller's transaction (`share`: take the row lock).
async fn load_locked<C: ConnectionTrait>(
    db: &C,
    doc_id: Uuid,
    share: bool,
) -> Result<Doc, PageError> {
    let mut find = docs::Entity::find_by_id(doc_id);
    if share {
        find = find.lock_shared();
    }
    let row = find
        .one(db)
        .await?
        .ok_or_else(|| PageError::Corrupt(format!("page {doc_id} not found")))?;
    let doc = Doc::new();
    if let Some(snapshot) = row.snapshot.as_deref() {
        apply(&doc, snapshot)?;
    }
    for u in updates_of(db, doc_id).await? {
        apply(&doc, &u.data)?;
    }
    Ok(doc)
}

async fn updates_of<C: ConnectionTrait>(
    db: &C,
    doc_id: Uuid,
) -> Result<Vec<doc_updates::Model>, DbErr> {
    doc_updates::Entity::find()
        .filter(doc_updates::Column::DocId.eq(doc_id))
        .order_by_asc(doc_updates::Column::Id)
        .all(db)
        .await
}

/// Logs one update (already validated) and touches the page.
pub async fn append(
    db: &DatabaseConnection,
    doc_id: Uuid,
    data: Vec<u8>,
    user_id: i64,
) -> Result<(), DbErr> {
    doc_updates::ActiveModel {
        doc_id: Set(doc_id),
        data: Set(data),
        user_id: Set(Some(user_id)),
        ..Default::default()
    }
    .insert(db)
    .await?;
    docs::Entity::update_many()
        .col_expr(
            docs::Column::UpdatedAt,
            sea_orm::sea_query::Expr::current_timestamp(),
        )
        .filter(docs::Column::Id.eq(doc_id))
        .exec(db)
        .await?;
    Ok(())
}

/// Folds the logged updates into the snapshot. Rebuilt from the database
/// under the page's row lock; only the rows actually read are deleted, so
/// an update committed meanwhile (even with a lower id) is never lost.
pub async fn compact(db: &DatabaseConnection, doc_id: Uuid) -> Result<(), PageError> {
    let txn = db.begin().await?;
    let Some(row) = docs::Entity::find_by_id(doc_id)
        .lock_exclusive()
        .one(&txn)
        .await?
    else {
        return Ok(());
    };
    let updates = updates_of(&txn, doc_id).await?;
    if updates.is_empty() {
        return Ok(());
    }
    let doc = Doc::new();
    if let Some(snapshot) = row.snapshot.as_deref() {
        apply(&doc, snapshot)?;
    }
    for u in &updates {
        apply(&doc, &u.data)?;
    }
    let ids: Vec<i64> = updates.iter().map(|u| u.id).collect();
    let max = ids.iter().copied().max().unwrap_or(row.snapshot_seq);
    let mut active: docs::ActiveModel = row.into();
    active.snapshot = Set(Some(encode(&doc)));
    active.snapshot_seq = Set(max);
    active.update(&txn).await?;
    doc_updates::Entity::delete_many()
        .filter(doc_updates::Column::Id.is_in(ids))
        .exec(&txn)
        .await?;
    txn.commit().await?;
    Ok(())
}

/// Server-side write (flows, D209): `update` is checked like a browser
/// session's (size, applies on the stored page, schema allowlist, page
/// size), logged without a user, then applied to this pod's open room and
/// relayed to the other pods.
pub async fn write_server(
    db: &DatabaseConnection,
    valkey_url: Option<String>,
    doc_id: Uuid,
    update: Vec<u8>,
) -> Result<(), PageError> {
    use pnex_core::pages::{MAX_DOC_BYTES, MAX_UPDATE_BYTES};
    if update.len() > MAX_UPDATE_BYTES {
        return Err(PageError::Refused("update too large".into()));
    }
    let doc = load(db, doc_id).await?;
    apply(&doc, &update)?;
    schema::validate(&doc).map_err(|e| PageError::Refused(e.0))?;
    if encode(&doc).len() > MAX_DOC_BYTES {
        return Err(PageError::Refused("page too large".into()));
    }
    doc_updates::ActiveModel {
        doc_id: Set(doc_id),
        data: Set(update.clone()),
        user_id: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await?;
    docs::Entity::update_many()
        .col_expr(
            docs::Column::UpdatedAt,
            sea_orm::sea_query::Expr::current_timestamp(),
        )
        .filter(docs::Column::Id.eq(doc_id))
        .exec(db)
        .await?;
    room::server_update(valkey_url, doc_id, update);
    Ok(())
}

/// Who froze a named version.
#[derive(Debug, Clone, Copy, Default)]
pub struct VersionAuthor {
    pub user_id: Option<i64>,
    /// Flow that wrote the page (`page_write`, D209).
    pub flow_id: Option<i64>,
    /// The content holds AI-generated text (D210).
    pub ai_generated: bool,
}

/// Freezes the current content as named version `label`; returns it.
pub async fn create_version(
    db: &DatabaseConnection,
    doc_id: Uuid,
    org_id: i64,
    label: &str,
    user_id: Option<i64>,
) -> Result<doc_versions::Model, PageError> {
    create_version_by(
        db,
        doc_id,
        org_id,
        label,
        VersionAuthor {
            user_id,
            ..Default::default()
        },
    )
    .await
}

/// [`create_version`] with the flow and AI provenance of the version.
pub async fn create_version_by(
    db: &DatabaseConnection,
    doc_id: Uuid,
    org_id: i64,
    label: &str,
    by: VersionAuthor,
) -> Result<doc_versions::Model, PageError> {
    let txn = db.begin().await?;
    // Row lock: serializes version numbering of this page.
    docs::Entity::find_by_id(doc_id)
        .lock_exclusive()
        .one(&txn)
        .await?;
    let state = encode(&load_locked(&txn, doc_id, false).await?);
    let last: Option<i64> = doc_versions::Entity::find()
        .select_only()
        .column_as(doc_versions::Column::VersionNumber.max(), "n")
        .filter(doc_versions::Column::DocId.eq(doc_id))
        .into_tuple()
        .one(&txn)
        .await?
        .flatten();
    let version = doc_versions::ActiveModel {
        doc_id: Set(doc_id),
        org_id: Set(org_id),
        version_number: Set(last.unwrap_or(0) + 1),
        label: Set(label.to_string()),
        snapshot: Set(state),
        created_by: Set(by.user_id),
        flow_id: Set(by.flow_id),
        ai_generated: Set(by.ai_generated),
        ..Default::default()
    }
    .insert(&txn)
    .await?;
    txn.commit().await?;
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::{GetString, Text};

    #[test]
    fn encode_apply_round_trip() {
        let a = Doc::new();
        let t = a.get_or_insert_text("t");
        t.insert(&mut a.transact_mut(), 0, "hello");
        let b = Doc::new();
        apply(&b, &encode(&a)).unwrap();
        // Idempotent: the same state applied twice changes nothing.
        apply(&b, &encode(&a)).unwrap();
        let tb = b.get_or_insert_text("t");
        assert_eq!(tb.get_string(&b.transact()), "hello");
        assert!(apply(&b, b"\xff\xff").is_err());
    }
}
