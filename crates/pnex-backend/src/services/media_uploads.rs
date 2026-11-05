//! A media file sent in several requests (data-pipelines.md §9.9). A proxy limits each
//! request (Cloudflare: 100 MB) and the time of its answer (about 100 s): parts wait on
//! S3, one object each, and any pod joins them afterwards into one version, streaming
//! part by part (one part in memory at a time). Same shape as dataset uploads
//! (`services::datasets::uploads`).

use std::collections::BTreeMap;
use std::time::Duration;

use axum::body::Bytes;
use futures_util::Stream;
use loco_rs::app::AppContext;
use pnex_core::err_codes;
use pnex_core::media_upload::{
    MediaUpload, MediaUploadInput, MediaUploadState, PARTS_MAX, PART_MAX_BYTES,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    DbErr, EntityTrait, QueryResult, Statement,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::controllers::media::{next_version_number, record_version, StoredVersion};
use crate::models::_entities::media_assets;
use crate::services::artifact_store::{S3Store, StreamWriteError};
use crate::services::datasets::uploads::missing_parts;
use crate::services::media::{MediaSettings, MediaStore};
use crate::services::media_sniff;

/// Uploads an org can hold at once: each holds up to the media limit outside any quota.
const ACTIVE_MAX: i64 = 4;
const JOIN_EVERY: Duration = Duration::from_secs(5);
const HEARTBEAT_EVERY: Duration = Duration::from_secs(60);
const JOIN_ATTEMPTS: i32 = 3;
const SWEEP_EVERY: u32 = 720;
const SELECT: &str = "\
    SELECT id, asset_id, filename, state, version_number, code, args, \
    (SELECT count(*) FROM jsonb_object_keys(parts))::int AS part_count, \
    (SELECT coalesce(sum(value::bigint), 0) FROM jsonb_each_text(parts))::bigint AS received, \
    to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_at \
    FROM media_uploads";

#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("upload not found")]
    NotFound,
    #[error("upload closed")]
    Closed,
    #[error("part too large")]
    PartTooLarge,
    #[error("part interrupted")]
    Interrupted,
    #[error("parts missing")]
    PartsMissing { missing: Vec<i32>, count: i32 },
    #[error("too many uploads")]
    TooMany,
    #[error("joining lost")]
    Lost,
    #[error("file too large")]
    FileTooLarge { max: u64 },
    #[error("format not allowed")]
    Format,
    #[error("invalid {field}")]
    Invalid { field: String, token: String },
    #[error("storage: {0}")]
    Storage(String),
    #[error("db: {0}")]
    Db(#[from] DbErr),
}

impl UploadError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => err_codes::MEDIA_UPLOAD_NOT_FOUND,
            Self::Closed => err_codes::MEDIA_UPLOAD_CLOSED,
            Self::PartTooLarge => err_codes::MEDIA_UPLOAD_PART_TOO_LARGE,
            Self::Interrupted => err_codes::MEDIA_UPLOAD_INTERRUPTED,
            Self::PartsMissing { .. } => err_codes::MEDIA_UPLOAD_PARTS_MISSING,
            Self::TooMany => err_codes::MEDIA_UPLOAD_TOO_MANY,
            // Storage and database failures end there once the attempts are spent.
            Self::Lost | Self::Storage(_) | Self::Db(_) => err_codes::MEDIA_UPLOAD_LOST,
            Self::FileTooLarge { .. } => err_codes::MEDIA_FILE_TOO_LARGE,
            Self::Format => err_codes::MEDIA_FORMAT_UNSUPPORTED,
            Self::Invalid { .. } => err_codes::FIELD_INVALID,
        }
    }

    pub fn args(&self) -> Option<serde_json::Value> {
        use serde_json::json;
        match self {
            Self::PartTooLarge => Some(json!({ "max": PART_MAX_BYTES.to_string() })),
            Self::PartsMissing { missing, count } => {
                let missing: Vec<String> = missing.iter().take(20).map(i32::to_string).collect();
                Some(json!({ "missing": missing.join(", "), "count": count.to_string() }))
            }
            Self::TooMany => Some(json!({ "max": ACTIVE_MAX.to_string() })),
            Self::FileTooLarge { max } => Some(json!({ "max_bytes": max.to_string() })),
            _ => None,
        }
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn upload_prefix(org_id: i64, id: Uuid) -> String {
    format!("org_{org_id}/media/uploads/{id}/")
}

fn part_key(org_id: i64, id: Uuid, part: i32) -> String {
    format!("{}{part:05}", upload_prefix(org_id, id))
}

fn upload_of(row: &QueryResult) -> Result<MediaUpload, DbErr> {
    let state: String = row.try_get("", "state")?;
    Ok(MediaUpload {
        id: row.try_get::<Uuid>("", "id")?.to_string(),
        asset_id: row
            .try_get::<Option<Uuid>>("", "asset_id")?
            .map(|a| a.to_string()),
        filename: row.try_get("", "filename")?,
        state: match state.as_str() {
            "open" => MediaUploadState::Open,
            "ready" => MediaUploadState::Ready,
            "joining" => MediaUploadState::Joining,
            "done" => MediaUploadState::Done,
            _ => MediaUploadState::Failed,
        },
        parts: row.try_get("", "part_count")?,
        bytes: row.try_get("", "received")?,
        version: row.try_get("", "version_number")?,
        code: row.try_get("", "code")?,
        args: row.try_get("", "args")?,
        created_at: row.try_get("", "created_at")?,
    })
}

pub async fn get(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<MediaUpload, UploadError> {
    let row = db
        .query_one_raw(stmt(
            &format!("{SELECT} WHERE id = $1 AND org_id = $2"),
            vec![id.into(), org_id.into()],
        ))
        .await?
        .ok_or(UploadError::NotFound)?;
    Ok(upload_of(&row)?)
}

fn trimmed(v: Option<&str>) -> Option<String> {
    v.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Opens an upload. The asset of a new version must be of this org; the format is only
/// judged once the file is whole.
pub async fn start(
    db: &DatabaseConnection,
    org_id: i64,
    input: &MediaUploadInput,
) -> Result<MediaUpload, UploadError> {
    let invalid = |field: &str, token: &str| UploadError::Invalid {
        field: field.into(),
        token: token.into(),
    };
    let filename = pnex_firmware_builder::sanitize_segment(input.filename.trim());
    if input.filename.trim().is_empty() {
        return Err(invalid("filename", err_codes::FIELD_REQUIRED));
    }
    // The controller checked the kind against the known ones.
    let kind = trimmed(input.kind.as_deref());
    let asset_id = match trimmed(input.asset_id.as_deref()) {
        None => None,
        Some(raw) => {
            let id = Uuid::parse_str(&raw).map_err(|_| invalid("asset_id", "invalid"))?;
            let asset = media_assets::Entity::find_by_id(id).one(db).await?;
            // Another org's asset is not told apart from a missing one (R1).
            if asset.is_none_or(|a| a.org_id != org_id) {
                return Err(invalid("asset_id", "invalid"));
            }
            Some(id)
        }
    };
    let row = db
        .query_one_raw(stmt(
            "INSERT INTO media_uploads (org_id, asset_id, filename, name, kind, content_type, note) \
             SELECT $1, $2, $3, $4, $5, $6, $7 \
             WHERE (SELECT count(*) FROM media_uploads \
                    WHERE org_id = $1 AND state IN ('open', 'ready', 'joining')) < $8 \
             RETURNING id",
            vec![
                org_id.into(),
                asset_id.into(),
                filename.into(),
                trimmed(input.name.as_deref()).into(),
                kind.into(),
                trimmed(input.content_type.as_deref()).into(),
                trimmed(input.note.as_deref()).into(),
                ACTIVE_MAX.into(),
            ],
        ))
        .await?
        .ok_or(UploadError::TooMany)?;
    get(db, org_id, row.try_get("", "id")?).await
}

/// Stores one part. Sending a part again replaces it.
pub async fn put_part<S, E>(
    db: &DatabaseConnection,
    store: &S3Store,
    max_bytes: u64,
    org_id: i64,
    id: Uuid,
    part: i32,
    body: S,
) -> Result<MediaUpload, UploadError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    if !(1..=PARTS_MAX).contains(&part) {
        return Err(UploadError::Invalid {
            field: "part".into(),
            token: format!("max:{PARTS_MAX}"),
        });
    }
    if get(db, org_id, id).await?.state != MediaUploadState::Open {
        return Err(UploadError::Closed);
    }
    let key = part_key(org_id, id, part);
    let size = store
        .write_stream(&key, body, PART_MAX_BYTES)
        .await
        .map_err(|e| match e {
            StreamWriteError::Interrupted => UploadError::Interrupted,
            StreamWriteError::TooLarge => UploadError::PartTooLarge,
            StreamWriteError::Storage(e) => UploadError::Storage(e),
        })?;
    let row = db
        .query_one_raw(stmt(
            "UPDATE media_uploads \
             SET parts = parts || jsonb_build_object($3::text, $4::bigint), updated_at = now() \
             WHERE id = $1 AND org_id = $2 AND state = 'open' \
             AND (SELECT coalesce(sum(value::bigint), 0) FROM jsonb_each_text(parts - $3::text)) \
                 + $4::bigint <= $5::bigint \
             RETURNING id",
            vec![
                id.into(),
                org_id.into(),
                part.to_string().into(),
                (size as i64).into(),
                (max_bytes as i64).into(),
            ],
        ))
        .await?;
    if row.is_none() {
        store.delete_key(&key).await.map_err(UploadError::Storage)?;
        return Err(
            if get(db, org_id, id).await?.state == MediaUploadState::Open {
                UploadError::FileTooLarge { max: max_bytes }
            } else {
                UploadError::Closed
            },
        );
    }
    get(db, org_id, id).await
}

/// No more parts: the file goes to a pod to be joined. `count` is what the client sent,
/// so that a part it believes stored and the server lost is not a shorter file.
pub async fn complete(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    count: i32,
) -> Result<MediaUpload, UploadError> {
    let row = db
        .query_one_raw(stmt(
            "SELECT state, parts FROM media_uploads WHERE id = $1 AND org_id = $2",
            vec![id.into(), org_id.into()],
        ))
        .await?
        .ok_or(UploadError::NotFound)?;
    let state: String = row.try_get("", "state")?;
    if state != "open" {
        return Err(UploadError::Closed);
    }
    let parts: BTreeMap<String, i64> =
        serde_json::from_value(row.try_get("", "parts")?).unwrap_or_default();
    let missing = missing_parts(&parts, count);
    if count < 1 || !missing.is_empty() || parts.len() != count as usize {
        return Err(UploadError::PartsMissing { missing, count });
    }
    let done = db
        .execute_raw(stmt(
            "UPDATE media_uploads SET state = 'ready', updated_at = now() \
             WHERE id = $1 AND state = 'open'",
            vec![id.into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(UploadError::Closed);
    }
    get(db, org_id, id).await
}

/// Takes one upload to join, ready or lost with its pod, and joins it. `Ok(false)` when
/// there is nothing to do now.
pub async fn join_next(ctx: &AppContext, store: &S3Store) -> Result<bool, UploadError> {
    let db = &ctx.db;
    let Some(row) = db
        .query_one_raw(stmt(
            "UPDATE media_uploads \
             SET state = 'joining', attempts = attempts + 1, heartbeat_at = now(), updated_at = now() \
             WHERE id = (SELECT id FROM media_uploads \
               WHERE state = 'ready' \
               OR (state = 'joining' AND heartbeat_at < now() - interval '10 minutes') \
               ORDER BY updated_at FOR UPDATE SKIP LOCKED LIMIT 1) \
             RETURNING id, org_id, asset_id, filename, name, kind, content_type, note, attempts, \
               (SELECT count(*) FROM jsonb_object_keys(parts))::int AS part_count, \
               (SELECT coalesce(sum(value::bigint), 0) FROM jsonb_each_text(parts))::bigint AS total",
            vec![],
        ))
        .await?
    else {
        return Ok(false);
    };
    let id: Uuid = row.try_get("", "id")?;
    let org_id: i64 = row.try_get("", "org_id")?;
    let attempts: i32 = row.try_get("", "attempts")?;
    if attempts > JOIN_ATTEMPTS {
        // One that keeps killing its pod does not get another one.
        finish(db, store, org_id, id, Err(UploadError::Lost)).await?;
        return Ok(true);
    }
    let beat = {
        let db = db.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(HEARTBEAT_EVERY).await;
                let update = stmt(
                    "UPDATE media_uploads SET heartbeat_at = now() WHERE id = $1",
                    vec![id.into()],
                );
                if let Err(e) = db.execute_raw(update).await {
                    tracing::warn!("media upload heartbeat: {e}");
                }
            }
        })
    };
    let result = join(ctx, store, &row).await;
    beat.abort();
    match result {
        Err(e @ (UploadError::Storage(_) | UploadError::Db(_))) if attempts < JOIN_ATTEMPTS => {
            tracing::warn!("media upload {id}: {e}");
            db.execute_raw(stmt(
                "UPDATE media_uploads SET state = 'ready', updated_at = now() WHERE id = $1",
                vec![id.into()],
            ))
            .await?;
            Ok(false)
        }
        result => {
            finish(db, store, org_id, id, result).await?;
            Ok(true)
        }
    }
}

/// Joins the parts into the next version of the asset, or of a new one.
async fn join(
    ctx: &AppContext,
    store: &S3Store,
    row: &QueryResult,
) -> Result<(Uuid, i64), UploadError> {
    let id: Uuid = row.try_get("", "id")?;
    let org_id: i64 = row.try_get("", "org_id")?;
    let filename: String = row.try_get("", "filename")?;
    let count: i32 = row.try_get("", "part_count")?;
    let total: i64 = row.try_get("", "total")?;
    let part = |n: i32| async move {
        store
            .get_bytes(&part_key(org_id, id, n))
            .await
            .map_err(UploadError::Storage)?
            .ok_or_else(|| UploadError::Storage(format!("part {n} is gone")))
    };
    let head = part(1).await?;
    let view = media_sniff::head_view(&filename, &head, total as u64).ok_or(UploadError::Format)?;
    let sniff = media_sniff::detect(&filename, view).ok_or(UploadError::Format)?;

    let existing = match row.try_get::<Option<Uuid>>("", "asset_id")? {
        Some(asset_id) => Some(
            media_assets::Entity::find_by_id(asset_id)
                .one(&ctx.db)
                .await?
                .ok_or(UploadError::Invalid {
                    field: "asset_id".into(),
                    token: "invalid".into(),
                })?,
        ),
        None => None,
    };
    let kind = match &existing {
        Some(asset) => asset.kind.clone(),
        None => row
            .try_get::<Option<String>>("", "kind")?
            .unwrap_or_else(|| sniff.kind.to_string()),
    };
    if !media_sniff::accepts(&kind, &filename, view) {
        return Err(UploadError::Format);
    }
    let asset_id = existing.as_ref().map_or_else(Uuid::new_v4, |a| a.id);
    let number = match &existing {
        Some(asset) => next_version_number(ctx, asset.id)
            .await
            .map_err(|e| UploadError::Storage(e.to_string()))?,
        None => 1,
    };

    // Streamed part by part: hashed as it goes, one part in memory.
    let key = MediaSettings::storage_key(org_id, asset_id, number, &filename);
    let mut writer = store.writer(&key).await.map_err(UploadError::Storage)?;
    let mut hasher = Sha256::new();
    let written = async {
        let mut bytes = head;
        for n in 1..=count {
            if n > 1 {
                bytes = part(n).await?;
            }
            hasher.update(&bytes);
            writer
                .write(std::mem::take(&mut bytes))
                .await
                .map_err(|e| UploadError::Storage(e.to_string()))?;
        }
        writer
            .close()
            .await
            .map_err(|e| UploadError::Storage(e.to_string()))?;
        Ok::<_, UploadError>(())
    }
    .await;
    if let Err(e) = written {
        if let Err(abort) = writer.abort().await {
            tracing::debug!("{key} not aborted: {abort}");
        }
        return Err(e);
    }

    let asset = match existing {
        Some(asset) => asset,
        None => media_assets::ActiveModel {
            id: Set(asset_id),
            org_id: Set(org_id),
            kind: Set(kind),
            name: Set(row
                .try_get::<Option<String>>("", "name")?
                .unwrap_or_else(|| filename.clone())),
            description: Set(None),
            metadata: Set(None),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .inspect_err(|_| {
            let (store, key) = (store.clone(), key.clone());
            tokio::spawn(async move { store.delete_key(&key).await });
        })?,
    };
    let media_store: std::sync::Arc<dyn MediaStore> = std::sync::Arc::new(store.clone());
    let stored = StoredVersion {
        number,
        filename,
        content_type: row
            .try_get::<Option<String>>("", "content_type")?
            .unwrap_or_default(),
        note: row.try_get("", "note")?,
        key,
        size: total,
        sha256: format!("{:x}", hasher.finalize()),
        sniff,
    };
    if let Err(e) = record_version(ctx, org_id, &asset, stored, media_store).await {
        if number == 1 {
            let _ = media_assets::Entity::delete_by_id(asset.id)
                .exec(&ctx.db)
                .await;
        }
        return Err(UploadError::Storage(e.to_string()));
    }
    Ok((asset.id, number))
}

/// Records the outcome, then frees the parts.
async fn finish(
    db: &DatabaseConnection,
    store: &S3Store,
    org_id: i64,
    id: Uuid,
    result: Result<(Uuid, i64), UploadError>,
) -> Result<(), UploadError> {
    let update = match result {
        Ok((asset_id, number)) => stmt(
            "UPDATE media_uploads SET state = 'done', asset_id = $2, version_number = $3, \
             updated_at = now() WHERE id = $1",
            vec![id.into(), asset_id.into(), number.into()],
        ),
        Err(e) => {
            if matches!(e, UploadError::Storage(_) | UploadError::Db(_)) {
                tracing::warn!("media upload {id} failed: {e}");
            }
            stmt(
                "UPDATE media_uploads SET state = 'failed', code = $2, args = $3, \
                 updated_at = now() WHERE id = $1",
                vec![id.into(), e.code().into(), e.args().into()],
            )
        }
    };
    db.execute_raw(update).await?;
    remove_parts(store, org_id, id).await
}

async fn remove_parts(store: &S3Store, org_id: i64, id: Uuid) -> Result<(), UploadError> {
    let prefix = upload_prefix(org_id, id);
    for (key, _, _) in store
        .list_prefix(&prefix)
        .await
        .map_err(UploadError::Storage)?
    {
        store.delete_key(&key).await.map_err(UploadError::Storage)?;
    }
    Ok(())
}

/// Uploads nobody finished lose their parts after a day, rows go after a week. The
/// second removal catches a part that landed after its upload was closed.
pub async fn sweep(db: &DatabaseConnection, store: &S3Store) -> Result<usize, UploadError> {
    let expired = db
        .query_all_raw(stmt(
            "UPDATE media_uploads SET state = 'failed', code = $1, updated_at = now() \
             WHERE state = 'open' AND updated_at < now() - interval '1 day' \
             RETURNING id, org_id",
            vec![err_codes::MEDIA_UPLOAD_EXPIRED.into()],
        ))
        .await?;
    let old = db
        .query_all_raw(stmt(
            "DELETE FROM media_uploads \
             WHERE state IN ('done', 'failed') AND updated_at < now() - interval '7 days' \
             RETURNING id, org_id",
            vec![],
        ))
        .await?;
    for row in expired.iter().chain(&old) {
        remove_parts(store, row.try_get("", "org_id")?, row.try_get("", "id")?).await?;
    }
    Ok(expired.len() + old.len())
}

/// Each pod joins one file at a time, and sweeps about once an hour.
pub fn spawn(ctx: &AppContext) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let mut ticks = 0u32;
        loop {
            tokio::time::sleep(JOIN_EVERY).await;
            if crate::services::ontology::imports::stop::stopping() {
                continue;
            }
            let Ok(store) = MediaSettings::from_config(&ctx.config).storage.store() else {
                continue;
            };
            ticks = ticks.wrapping_add(1);
            if ticks.is_multiple_of(SWEEP_EVERY) {
                if let Err(e) = sweep(&ctx.db, &store).await {
                    tracing::warn!("media upload sweep failed: {e}");
                }
            }
            loop {
                match join_next(&ctx, &store).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(e) => {
                        tracing::warn!("media upload join failed: {e}");
                        break;
                    }
                }
            }
        }
    });
}
