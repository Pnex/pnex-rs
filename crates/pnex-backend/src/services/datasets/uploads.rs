//! A file sent in several requests (data-pipelines.md D569). A proxy limits each
//! request (Cloudflare: 100 MB) and the time of its answer (about 100 s): parts wait on
//! S3, one object each, and any pod converts them afterwards through [`load_csv`]. A part
//! can be sent again, and a conversion lost with its pod starts again.

use std::collections::BTreeMap;
use std::time::Duration;

use axum::body::Bytes;
use futures_util::{Stream, StreamExt};
use pnex_core::datasets::{
    DatasetTransactionKind, DatasetUpload, DatasetUploadState, SOURCE_MAX_BYTES, UPLOAD_PARTS_MAX,
    UPLOAD_PART_MAX_BYTES,
};
use pnex_core::err_codes;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, QueryResult};
use uuid::Uuid;

use super::{check_loadable, dataset_prefix, load_csv, remove_prefix, stmt, storage, DatasetError};
use crate::services::artifact_store::{S3Store, StreamWriteError};

/// Waiting parts an org can hold: each upload holds up to [`SOURCE_MAX_BYTES`], outside
/// the quota until it is converted.
const ACTIVE_MAX: i64 = 4;
const CONVERT_EVERY: Duration = Duration::from_secs(5);
const HEARTBEAT_EVERY: Duration = Duration::from_secs(60);
const CONVERT_ATTEMPTS: i32 = 3;
const SELECT: &str = "\
    SELECT id, dataset_id, kind, delimiter, state, transaction_id, code, args, parts, \
    (SELECT count(*) FROM jsonb_object_keys(parts))::int AS part_count, \
    (SELECT coalesce(sum(value::bigint), 0) FROM jsonb_each_text(parts))::bigint AS received, \
    to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_at \
    FROM dataset_uploads";

fn upload_prefix(org_id: i64, dataset_id: Uuid, id: Uuid) -> String {
    format!("{}uploads/{id}/", dataset_prefix(org_id, dataset_id))
}

fn part_key(org_id: i64, dataset_id: Uuid, id: Uuid, part: i32) -> String {
    format!("{}{part:05}", upload_prefix(org_id, dataset_id, id))
}

fn upload_of(row: &QueryResult) -> Result<DatasetUpload, DbErr> {
    let kind: String = row.try_get("", "kind")?;
    let state: String = row.try_get("", "state")?;
    Ok(DatasetUpload {
        id: row.try_get::<Uuid>("", "id")?.to_string(),
        dataset_id: row.try_get::<Uuid>("", "dataset_id")?.to_string(),
        kind: match kind.as_str() {
            "append" => DatasetTransactionKind::Append,
            _ => DatasetTransactionKind::Snapshot,
        },
        delimiter: row.try_get("", "delimiter")?,
        state: match state.as_str() {
            "open" => DatasetUploadState::Open,
            "ready" => DatasetUploadState::Ready,
            "converting" => DatasetUploadState::Converting,
            "done" => DatasetUploadState::Done,
            _ => DatasetUploadState::Failed,
        },
        parts: row.try_get("", "part_count")?,
        bytes: row.try_get("", "received")?,
        transaction_id: row
            .try_get::<Option<Uuid>>("", "transaction_id")?
            .map(|t| t.to_string()),
        code: row.try_get("", "code")?,
        args: row.try_get("", "args")?,
        created_at: row.try_get("", "created_at")?,
    })
}

pub async fn get(
    db: &DatabaseConnection,
    org_id: i64,
    dataset_id: Uuid,
    id: Uuid,
) -> Result<DatasetUpload, DatasetError> {
    let row = db
        .query_one_raw(stmt(
            &format!("{SELECT} WHERE id = $1 AND org_id = $2 AND dataset_id = $3"),
            vec![id.into(), org_id.into(), dataset_id.into()],
        ))
        .await?
        .ok_or(DatasetError::UploadNotFound)?;
    Ok(upload_of(&row)?)
}

/// Opens an upload. One still receiving parts for this table is replaced: its client
/// is gone, or it would have finished; a second person sending at once loses theirs.
pub async fn start(
    db: &DatabaseConnection,
    store: &S3Store,
    org_id: i64,
    dataset_id: Uuid,
    kind: DatasetTransactionKind,
    delimiter: Option<&str>,
) -> Result<DatasetUpload, DatasetError> {
    check_loadable(db, org_id, dataset_id).await?;
    let replaced = db
        .query_all_raw(stmt(
            "UPDATE dataset_uploads SET state = 'failed', code = $3, updated_at = now() \
             WHERE org_id = $1 AND dataset_id = $2 AND state = 'open' RETURNING id",
            vec![
                org_id.into(),
                dataset_id.into(),
                err_codes::DATASET_UPLOAD_EXPIRED.into(),
            ],
        ))
        .await?;
    for row in &replaced {
        let id: Uuid = row.try_get("", "id")?;
        remove_prefix(store, &upload_prefix(org_id, dataset_id, id)).await?;
    }
    let active = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM dataset_uploads \
             WHERE org_id = $1 AND state IN ('open', 'ready', 'converting')",
            vec![org_id.into()],
        ))
        .await?
        .map_or(Ok(0), |r| r.try_get::<i64>("", "n"))?;
    if active >= ACTIVE_MAX {
        return Err(DatasetError::TooManyUploads);
    }
    let row = db
        .query_one_raw(stmt(
            "INSERT INTO dataset_uploads (org_id, dataset_id, kind, delimiter) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (dataset_id) WHERE state IN ('open', 'ready', 'converting') DO NOTHING \
             RETURNING id",
            vec![
                org_id.into(),
                dataset_id.into(),
                kind.as_str().into(),
                delimiter.map(str::to_string).into(),
            ],
        ))
        .await?
        // The one before is being converted.
        .ok_or(DatasetError::TransactionOpen)?;
    get(db, org_id, dataset_id, row.try_get("", "id")?).await
}

/// Stores one part. Sending a part again replaces it.
pub async fn put_part<S, E>(
    db: &DatabaseConnection,
    store: &S3Store,
    org_id: i64,
    dataset_id: Uuid,
    id: Uuid,
    part: i32,
    body: S,
) -> Result<DatasetUpload, DatasetError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    if !(1..=UPLOAD_PARTS_MAX).contains(&part) {
        return Err(DatasetError::Invalid {
            field: "part".into(),
            token: format!("max:{UPLOAD_PARTS_MAX}"),
        });
    }
    let before = get(db, org_id, dataset_id, id).await?;
    if before.state != DatasetUploadState::Open {
        return Err(DatasetError::UploadClosed);
    }
    let key = part_key(org_id, dataset_id, id, part);
    let size = store
        .write_stream(&key, body, UPLOAD_PART_MAX_BYTES)
        .await
        .map_err(|e| match e {
            StreamWriteError::Interrupted => DatasetError::SourceInterrupted,
            StreamWriteError::TooLarge => DatasetError::PartTooLarge,
            StreamWriteError::Storage(e) => storage(e),
        })?;
    let row = db
        .query_one_raw(stmt(
            "UPDATE dataset_uploads \
             SET parts = parts || jsonb_build_object($4::text, $5::bigint), updated_at = now() \
             WHERE id = $1 AND org_id = $2 AND dataset_id = $3 AND state = 'open' \
             AND (SELECT coalesce(sum(value::bigint), 0) FROM jsonb_each_text(parts - $4::text)) \
                 + $5::bigint <= $6::bigint \
             RETURNING id",
            vec![
                id.into(),
                org_id.into(),
                dataset_id.into(),
                part.to_string().into(),
                (size as i64).into(),
                (SOURCE_MAX_BYTES as i64).into(),
            ],
        ))
        .await?;
    if row.is_none() {
        store.delete_key(&key).await.map_err(storage)?;
        let now = get(db, org_id, dataset_id, id).await?;
        return Err(if now.state == DatasetUploadState::Open {
            DatasetError::SourceTooLarge
        } else {
            DatasetError::UploadClosed
        });
    }
    get(db, org_id, dataset_id, id).await
}

/// Parts from 1 to `count` are all there, and no other.
pub(crate) fn missing_parts(parts: &BTreeMap<String, i64>, count: i32) -> Vec<i32> {
    (1..=count)
        .filter(|n| !parts.contains_key(&n.to_string()))
        .collect()
}

/// No more parts: the file goes to a pod for conversion. `count` is what the client
/// sent, so that a part it believes stored and the server lost is not a shorter table.
pub async fn complete(
    db: &DatabaseConnection,
    org_id: i64,
    dataset_id: Uuid,
    id: Uuid,
    count: i32,
) -> Result<DatasetUpload, DatasetError> {
    check_loadable(db, org_id, dataset_id).await?;
    let row = db
        .query_one_raw(stmt(
            &format!("{SELECT} WHERE id = $1 AND org_id = $2 AND dataset_id = $3"),
            vec![id.into(), org_id.into(), dataset_id.into()],
        ))
        .await?
        .ok_or(DatasetError::UploadNotFound)?;
    let upload = upload_of(&row)?;
    if upload.state != DatasetUploadState::Open {
        return Err(DatasetError::UploadClosed);
    }
    let parts: BTreeMap<String, i64> =
        serde_json::from_value(row.try_get("", "parts")?).unwrap_or_default();
    let missing = missing_parts(&parts, count);
    if count < 1 || !missing.is_empty() || parts.len() != count as usize {
        return Err(DatasetError::PartsMissing { missing, count });
    }
    let done = db
        .execute_raw(stmt(
            "UPDATE dataset_uploads SET state = 'ready', updated_at = now() \
             WHERE id = $1 AND state = 'open'",
            vec![id.into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(DatasetError::UploadClosed);
    }
    get(db, org_id, dataset_id, id).await
}

/// Takes one upload to convert, ready or lost with its pod, and converts it. `Ok(false)`
/// when there is nothing to do now.
pub async fn convert_next(db: &DatabaseConnection, store: &S3Store) -> Result<bool, DatasetError> {
    let Some(row) = db
        .query_one_raw(stmt(
            "UPDATE dataset_uploads \
             SET state = 'converting', attempts = attempts + 1, heartbeat_at = now(), updated_at = now() \
             WHERE id = (SELECT id FROM dataset_uploads \
               WHERE state = 'ready' \
               OR (state = 'converting' AND heartbeat_at < now() - interval '10 minutes') \
               ORDER BY updated_at FOR UPDATE SKIP LOCKED LIMIT 1) \
             RETURNING id, org_id, dataset_id, kind, delimiter, attempts, \
               (SELECT count(*) FROM jsonb_object_keys(parts))::int AS part_count",
            vec![],
        ))
        .await?
    else {
        return Ok(false);
    };
    let id: Uuid = row.try_get("", "id")?;
    let org_id: i64 = row.try_get("", "org_id")?;
    let dataset_id: Uuid = row.try_get("", "dataset_id")?;
    let attempts: i32 = row.try_get("", "attempts")?;
    if attempts > CONVERT_ATTEMPTS {
        // One that keeps killing its pod does not get another one.
        finish(
            db,
            store,
            org_id,
            dataset_id,
            id,
            Err(DatasetError::UploadLost),
        )
        .await?;
        return Ok(true);
    }
    let kind: String = row.try_get("", "kind")?;
    let kind = match kind.as_str() {
        "append" => DatasetTransactionKind::Append,
        _ => DatasetTransactionKind::Snapshot,
    };
    let delimiter: Option<String> = row.try_get("", "delimiter")?;
    let delimiter = delimiter.and_then(|d| d.bytes().next());
    let count: i32 = row.try_get("", "part_count")?;

    let beat = {
        let db = db.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(HEARTBEAT_EVERY).await;
                let update = stmt(
                    "UPDATE dataset_uploads SET heartbeat_at = now() WHERE id = $1",
                    vec![id.into()],
                );
                if let Err(e) = db.execute_raw(update).await {
                    tracing::warn!("dataset upload heartbeat: {e}");
                }
            }
        })
    };
    let parts = futures_util::stream::iter(1..=count).then(|part| {
        let key = part_key(org_id, dataset_id, id, part);
        async move {
            match store.get_bytes(&key).await {
                Ok(Some(bytes)) => Ok(Bytes::from(bytes)),
                Ok(None) => Err(format!("part {part} is gone")),
                Err(e) => Err(e),
            }
        }
    });
    let result = match check_loadable(db, org_id, dataset_id).await {
        Ok(()) => load_csv(
            db,
            store,
            org_id,
            dataset_id,
            kind,
            delimiter,
            Box::pin(parts),
        )
        .await
        .map(|t| t.id),
        Err(e) => Err(e),
    };
    beat.abort();
    match result {
        // Someone else is loading this table, or the store is away: its parts wait.
        Err(DatasetError::TransactionOpen) => {
            requeue(db, id, 1).await?;
            Ok(false)
        }
        Err(DatasetError::Storage(e)) if attempts < CONVERT_ATTEMPTS => {
            tracing::warn!("dataset upload {id}: {e}");
            requeue(db, id, 0).await?;
            Ok(false)
        }
        Err(DatasetError::Db(e)) if attempts < CONVERT_ATTEMPTS => {
            tracing::warn!("dataset upload {id}: {e}");
            requeue(db, id, 0).await?;
            Ok(false)
        }
        result => {
            finish(db, store, org_id, dataset_id, id, result).await?;
            Ok(true)
        }
    }
}

async fn requeue(db: &DatabaseConnection, id: Uuid, refund: i32) -> Result<(), DbErr> {
    db.execute_raw(stmt(
        "UPDATE dataset_uploads SET state = 'ready', attempts = attempts - $2, updated_at = now() \
         WHERE id = $1",
        vec![id.into(), refund.into()],
    ))
    .await?;
    Ok(())
}

/// Records the outcome, then frees the parts: raw rows do not stay beside the table.
async fn finish(
    db: &DatabaseConnection,
    store: &S3Store,
    org_id: i64,
    dataset_id: Uuid,
    id: Uuid,
    result: Result<String, DatasetError>,
) -> Result<(), DatasetError> {
    let update = match result {
        Ok(transaction) => stmt(
            "UPDATE dataset_uploads SET state = 'done', transaction_id = $2::uuid, \
             updated_at = now() WHERE id = $1",
            vec![id.into(), transaction.into()],
        ),
        Err(e) => stmt(
            "UPDATE dataset_uploads SET state = 'failed', code = $2, args = $3, \
             updated_at = now() WHERE id = $1",
            vec![id.into(), e.code().into(), e.args().into()],
        ),
    };
    db.execute_raw(update).await?;
    remove_prefix(store, &upload_prefix(org_id, dataset_id, id)).await?;
    Ok(())
}

/// Uploads nobody finished lose their parts after a day, rows go after a week. The
/// second removal catches a part that landed after its upload was closed.
pub(super) async fn sweep(db: &DatabaseConnection, store: &S3Store) -> Result<usize, DatasetError> {
    let expired = db
        .query_all_raw(stmt(
            "UPDATE dataset_uploads SET state = 'failed', code = $1, updated_at = now() \
             WHERE state = 'open' AND updated_at < now() - interval '1 day' \
             RETURNING id, org_id, dataset_id",
            vec![err_codes::DATASET_UPLOAD_EXPIRED.into()],
        ))
        .await?;
    let old = db
        .query_all_raw(stmt(
            "DELETE FROM dataset_uploads \
             WHERE state IN ('done', 'failed') AND updated_at < now() - interval '7 days' \
             RETURNING id, org_id, dataset_id",
            vec![],
        ))
        .await?;
    let mut removed = 0;
    for row in expired.iter().chain(&old) {
        let prefix = upload_prefix(
            row.try_get("", "org_id")?,
            row.try_get("", "dataset_id")?,
            row.try_get("", "id")?,
        );
        removed += remove_prefix(store, &prefix).await?;
    }
    Ok(removed)
}

/// Each pod converts one file at a time: the conversion holds little memory, and a
/// waiting file costs nothing.
pub fn spawn(ctx: &loco_rs::app::AppContext) {
    let db = ctx.db.clone();
    let config = ctx.config.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(CONVERT_EVERY).await;
            if crate::services::ontology::imports::stop::stopping() {
                continue;
            }
            let Ok(store) = super::store_of(&config) else {
                continue;
            };
            loop {
                match convert_next(&db, &store).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(e) => {
                        tracing::warn!("dataset upload conversion failed: {e:?}");
                        break;
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_part_is_missing_until_every_number_is_there() {
        let parts = BTreeMap::from([("1".to_string(), 5), ("3".to_string(), 5)]);
        assert_eq!(missing_parts(&parts, 3), vec![2]);
        assert_eq!(missing_parts(&parts, 1), Vec::<i32>::new());
    }
}
