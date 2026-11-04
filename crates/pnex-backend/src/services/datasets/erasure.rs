//! Erasure in datasets (data-pipelines.md D569). A person's row is removed from
//! every version still readable, not only from the current view. A dataset is
//! reached by a declared identity column; what its rows fed is recomputed.
//!
//! The object is erased at once and the files later: the queue row is written in
//! the transaction of the object, so neither can happen without the other. The row
//! stays afterwards, holding a hash and not the value: each later load of the dataset
//! leaves the person out, even from a source that still holds them.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use pnex_core::datasets::{
    fingerprint, Dataset, DatasetIdentityColumn, IDENTITY_COLUMNS_MAX, IDENTITY_TITLE,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, QueryResult, TransactionTrait};
use uuid::Uuid;

use super::parquet::{self, Targets};
use super::{
    get, remove_prefix, stmt, storage, transaction_prefix, DatasetError, DEAD_AFTER_MINUTES,
};
use crate::services::artifact_store::S3Store;
use crate::services::ontology;

const ERASE_EVERY: Duration = Duration::from_secs(60);
/// Longest chain of pipelines followed downstream; lineage can hold a cycle
/// across pipelines.
const DOWNSTREAM_DEPTH_MAX: i32 = 64;
/// Any value: identifies this pass among advisory locks.
const PASS_LOCK: i64 = 0x7064_6572;

/// What loads of this dataset leave out, erasures already applied included.
pub async fn targets_of(db: &DatabaseConnection, dataset_id: Uuid) -> Result<Targets, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT column_name, value FROM dataset_erasures WHERE dataset_id = $1",
            vec![dataset_id.into()],
        ))
        .await?;
    let mut out = Targets::new();
    for r in &rows {
        out.entry(r.try_get("", "column_name")?)
            .or_default()
            .insert(r.try_get("", "value")?);
    }
    Ok(out)
}

/// Declares which columns of a dataset identify which objects. Replaces the list.
pub async fn set_identity(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    columns: &[DatasetIdentityColumn],
) -> Result<Dataset, DatasetError> {
    let dataset = get(db, org_id, id).await?;
    // Its mapping declares what erasure reaches; removing that is not for a person.
    super::check_loadable(db, org_id, id).await?;
    let invalid = |path: String, token: &str| DatasetError::Invalid {
        field: path,
        token: token.to_string(),
    };
    if columns.len() > IDENTITY_COLUMNS_MAX {
        return Err(invalid(
            "columns".into(),
            &format!("max_items:{IDENTITY_COLUMNS_MAX}"),
        ));
    }
    let schema = ontology::schema(db, org_id)
        .await
        .map_err(|e| DatasetError::Db(sea_orm::DbErr::Custom(format!("{e:?}"))))?;
    for (i, c) in columns.iter().enumerate() {
        if !dataset.columns.iter().any(|d| d.name == c.column) {
            return Err(invalid(format!("columns.{i}.column"), "invalid"));
        }
        let Some(def) = schema.object_type(&c.type_key) else {
            return Err(invalid(format!("columns.{i}.type_key"), "invalid"));
        };
        if c.property != IDENTITY_TITLE && !def.properties.iter().any(|p| p.key == c.property) {
            return Err(invalid(format!("columns.{i}.property"), "invalid"));
        }
    }
    let tx = db.begin().await?;
    tx.execute_raw(stmt(
        "DELETE FROM dataset_identity_columns WHERE dataset_id = $1",
        vec![id.into()],
    ))
    .await?;
    for c in columns {
        tx.execute_raw(stmt(
            "INSERT INTO dataset_identity_columns (dataset_id, column_name, type_key, property) \
             VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
            vec![
                id.into(),
                c.column.clone().into(),
                c.type_key.clone().into(),
                c.property.clone().into(),
            ],
        ))
        .await?;
    }
    tx.commit().await?;
    get(db, org_id, id).await
}

/// Queues the rows of an object in the datasets that declare its type. Called in
/// the transaction that erases it, with the values it held.
pub async fn enqueue<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    type_key: &str,
    title: &str,
    properties: &serde_json::Value,
) -> Result<(), sea_orm::DbErr> {
    let declared = db
        .query_all_raw(stmt(
            "SELECT i.dataset_id, i.column_name, i.property FROM dataset_identity_columns i \
             JOIN datasets d ON d.id = i.dataset_id WHERE d.org_id = $1 AND i.type_key = $2",
            vec![org_id.into(), type_key.into()],
        ))
        .await?;
    for row in &declared {
        let property: String = row.try_get("", "property")?;
        let value = match property.as_str() {
            IDENTITY_TITLE => Some(title.to_string()),
            key => match properties.get(key) {
                Some(serde_json::Value::String(s)) => Some(s.clone()),
                Some(serde_json::Value::Number(n)) => Some(n.to_string()),
                _ => None,
            },
        };
        let Some(value) = value.and_then(|v| fingerprint(&v)) else {
            continue;
        };
        db.execute_raw(stmt(
            "INSERT INTO dataset_erasures (org_id, dataset_id, column_name, value, type_key) \
             VALUES ($1, $2, $3, $4, $5)",
            vec![
                org_id.into(),
                row.try_get::<Uuid>("", "dataset_id")?.into(),
                row.try_get::<String>("", "column_name")?.into(),
                value.into(),
                type_key.into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

/// What is pending on one dataset.
#[derive(Default)]
struct Pending {
    org_id: i64,
    ids: Vec<Uuid>,
    targets: Targets,
}

fn pending_of(rows: &[QueryResult]) -> Result<BTreeMap<Uuid, Pending>, sea_orm::DbErr> {
    let mut out: BTreeMap<Uuid, Pending> = BTreeMap::new();
    for row in rows {
        let p = out.entry(row.try_get("", "dataset_id")?).or_default();
        p.org_id = row.try_get("", "org_id")?;
        p.ids.push(row.try_get("", "id")?);
        p.targets
            .entry(row.try_get("", "column_name")?)
            .or_default()
            .insert(row.try_get("", "value")?);
    }
    Ok(out)
}

/// One pass over the queue. Returns the datasets finished. An org with a run in
/// progress waits: the run may have read the rows and would write them back. The
/// worker takes no new run of an org with an erasure pending.
pub async fn erase_pass(db: &DatabaseConnection, store: &S3Store) -> Result<usize, DatasetError> {
    // Held for the whole pass: two passes on one file would remove each other's
    // output as unreferenced.
    let lock = db.begin().await?;
    let mine = lock
        .query_one_raw(stmt(
            "SELECT pg_try_advisory_xact_lock($1) AS mine",
            vec![PASS_LOCK.into()],
        ))
        .await?
        .map_or(Ok(false), |r| r.try_get("", "mine"))?;
    if !mine {
        return Ok(0);
    }
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "SELECT e.id, e.org_id, e.dataset_id, e.column_name, e.value \
                 FROM dataset_erasures e WHERE e.done_at IS NULL AND NOT EXISTS ( \
                   SELECT 1 FROM pipeline_jobs j WHERE j.org_id = e.org_id AND j.state = 'running' \
                   AND j.heartbeat_at > now() - interval '{DEAD_AFTER_MINUTES} minutes') \
                 AND NOT EXISTS ( \
                   SELECT 1 FROM dataset_transactions t WHERE t.dataset_id = e.dataset_id \
                   AND t.state = 'open' \
                   AND t.heartbeat_at > now() - interval '{DEAD_AFTER_MINUTES} minutes')"
            ),
            vec![],
        ))
        .await?;
    let mut done = 0;
    for (dataset_id, pending) in pending_of(&rows)? {
        // One bad file must not stop the erasures of every other org.
        match erase_dataset(db, store, dataset_id, &pending).await {
            Ok(()) => done += 1,
            Err(e) => tracing::error!(dataset = %dataset_id, "dataset erasure failed: {e:?}"),
        }
    }
    lock.rollback().await?;
    Ok(done)
}

async fn erase_dataset(
    db: &DatabaseConnection,
    store: &S3Store,
    dataset_id: Uuid,
    pending: &Pending,
) -> Result<(), DatasetError> {
    let transactions = db
        .query_all_raw(stmt(
            "SELECT id FROM dataset_transactions \
             WHERE dataset_id = $1 AND state = 'committed' AND purged_at IS NULL",
            vec![dataset_id.into()],
        ))
        .await?;
    for row in &transactions {
        let transaction_id: Uuid = row.try_get("", "id")?;
        erase_transaction(db, store, dataset_id, transaction_id, pending).await?;
    }
    let hit = db
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM dataset_erasures WHERE id = ANY($1) AND hit",
            vec![pending.ids.clone().into()],
        ))
        .await?
        .is_some();
    if hit {
        recompute_downstream(db, store, dataset_id, pending).await?;
    }
    db.execute_raw(stmt(
        "UPDATE dataset_erasures SET done_at = now() WHERE id = ANY($1)",
        vec![pending.ids.clone().into()],
    ))
    .await?;
    Ok(())
}

async fn erase_transaction(
    db: &DatabaseConnection,
    store: &S3Store,
    dataset_id: Uuid,
    transaction_id: Uuid,
    pending: &Pending,
) -> Result<(), DatasetError> {
    let prefix = transaction_prefix(pending.org_id, dataset_id, transaction_id);
    let files = db
        .query_all_raw(stmt(
            "SELECT id, key, bytes FROM dataset_files WHERE transaction_id = $1",
            vec![transaction_id.into()],
        ))
        .await?;
    let mut kept = HashSet::new();
    for file in &files {
        let (file_id, key, size): (Uuid, String, i64) = (
            file.try_get("", "id")?,
            file.try_get("", "key")?,
            file.try_get("", "bytes")?,
        );
        if !parquet::has_match(store, &key, size as u64, &pending.targets)
            .await
            .map_err(storage)?
        {
            kept.insert(key);
            continue;
        }
        let new_key = format!("{prefix}erased-{}.parquet", Uuid::new_v4());
        let out = parquet::rewrite_without(store, &key, size as u64, &new_key, &pending.targets)
            .await
            .map_err(storage)?;
        let tx = db.begin().await?;
        tx.execute_raw(stmt(
            "UPDATE dataset_files SET key = $2, bytes = $3, rows = $4, sha256 = $5 WHERE id = $1",
            vec![
                file_id.into(),
                new_key.clone().into(),
                (out.bytes as i64).into(),
                (out.rows as i64).into(),
                out.sha256.into(),
            ],
        ))
        .await?;
        tx.execute_raw(stmt(
            "UPDATE dataset_transactions SET rows = rows - $2, bytes = bytes - $3 + $4 WHERE id = $1",
            vec![
                transaction_id.into(),
                (out.removed as i64).into(),
                size.into(),
                (out.bytes as i64).into(),
            ],
        ))
        .await?;
        tx.execute_raw(stmt(
            "UPDATE dataset_erasures SET hit = true WHERE id = ANY($1)",
            vec![pending.ids.clone().into()],
        ))
        .await?;
        tx.commit().await?;
        kept.insert(new_key);
    }
    // By listing, not from the file just replaced: a pass that died after its
    // commit left the old file, and the retry finds nothing left to rewrite.
    for (key, _, _) in store.list_prefix(&prefix).await.map_err(storage)? {
        if !kept.contains(&key) {
            store.delete_key(&key).await.map_err(storage)?;
        }
    }
    Ok(())
}

/// What the removed rows fed. A dataset with a queue row of the same erasure (the
/// rows of one object share their instant) is cleaned by it. Any other may hold the
/// person under a form no one declared: all its versions go and its pipeline computes
/// it again from the clean inputs.
async fn recompute_downstream(
    db: &DatabaseConnection,
    store: &S3Store,
    dataset_id: Uuid,
    pending: &Pending,
) -> Result<(), DatasetError> {
    let down = db
        .query_all_raw(stmt(
            &format!(
                "WITH RECURSIVE down(dataset_id, depth) AS ( \
                   SELECT $1::uuid, 0 \
                   UNION \
                   SELECT o.dataset_id, d.depth + 1 FROM down d \
                   JOIN dataset_transactions i ON i.dataset_id = d.dataset_id \
                   JOIN dataset_transaction_inputs l ON l.input_transaction_id = i.id \
                   JOIN dataset_transactions o ON o.id = l.transaction_id \
                   WHERE d.depth < {DOWNSTREAM_DEPTH_MAX} \
                 ) \
                 SELECT dataset_id FROM down WHERE dataset_id <> $1 AND NOT EXISTS ( \
                   SELECT 1 FROM dataset_erasures c \
                   WHERE c.dataset_id = down.dataset_id AND c.created_at IN ( \
                     SELECT created_at FROM dataset_erasures WHERE id = ANY($2))) \
                 GROUP BY dataset_id ORDER BY max(depth)"
            ),
            vec![dataset_id.into(), pending.ids.clone().into()],
        ))
        .await?;
    for row in &down {
        let id: Uuid = row.try_get("", "dataset_id")?;
        let versions = db
            .query_all_raw(stmt(
                "SELECT id FROM dataset_transactions \
                 WHERE dataset_id = $1 AND state = 'committed' AND purged_at IS NULL",
                vec![id.into()],
            ))
            .await?;
        for version in &versions {
            let transaction: Uuid = version.try_get("", "id")?;
            remove_prefix(store, &transaction_prefix(pending.org_id, id, transaction)).await?;
            db.execute_raw(stmt(
                "UPDATE dataset_transactions SET purged_at = now() WHERE id = $1",
                vec![transaction.into()],
            ))
            .await?;
        }
        // One statement per dataset, nearest first: the queue runs by creation time.
        db.execute_raw(stmt(
            "INSERT INTO pipeline_jobs (org_id, pipeline_id, version, trigger) \
             SELECT p.org_id, p.id, p.version, 'erasure' FROM pipelines p WHERE p.id IN ( \
               SELECT l.pipeline_id FROM dataset_transaction_inputs l \
               JOIN dataset_transactions o ON o.id = l.transaction_id WHERE o.dataset_id = $1) \
             ON CONFLICT DO NOTHING",
            vec![id.into()],
        ))
        .await?;
    }
    Ok(())
}

pub fn spawn(ctx: &loco_rs::app::AppContext) {
    let db = ctx.db.clone();
    let config = ctx.config.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(ERASE_EVERY).await;
            match super::store_of(&config) {
                Ok(store) => match erase_pass(&db, &store).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(datasets = n, "dataset erasure"),
                    // Stays queued. Values never reach the logs.
                    Err(e) => tracing::error!("dataset erasure failed: {e:?}"),
                },
                Err(e) => tracing::debug!("dataset erasure skipped: {e:?}"),
            }
        }
    });
}
