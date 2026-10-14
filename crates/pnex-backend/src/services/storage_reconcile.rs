//! Object storage ↔ database reconciliation (read-only report): objects no
//! row references (orphans, garbage-collection candidates) and referenced
//! keys with no object (lost on the way). Nothing is deleted here.

use std::collections::HashSet;

use loco_rs::app::AppContext;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, Statement};

use crate::services::artifact_store::S3Config;

/// Exact keys referenced by a row.
const KEY_COLUMNS: &str = "\
    SELECT storage_key AS k FROM media_versions \
    UNION ALL SELECT storage_key FROM media_segments WHERE storage_key IS NOT NULL \
    UNION ALL SELECT storage_key FROM video_segments \
    UNION ALL SELECT firmware_bin_s3_key FROM build_records WHERE firmware_bin_s3_key IS NOT NULL \
    UNION ALL SELECT artifact_key FROM ota_assignments";

/// What the database says the bucket holds.
#[derive(Debug, Default)]
pub struct Known {
    /// Keys that must exist.
    pub keys: HashSet<String>,
    /// Owned namespaces whose object count the database does not track
    /// (stitch job frames): anything under them is not an orphan.
    pub prefixes: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub objects: usize,
    pub referenced: usize,
    /// In the bucket, referenced by no row.
    pub orphans: Vec<String>,
    /// Referenced by a row, absent from the bucket.
    pub missing: Vec<String>,
}

pub async fn known(db: &DatabaseConnection) -> Result<Known, DbErr> {
    let backend = db.get_database_backend();
    let rows = |sql: &str| db.query_all_raw(Statement::from_string(backend, sql.to_string()));
    let mut known = Known::default();
    for row in rows(KEY_COLUMNS).await? {
        known.keys.insert(row.try_get::<String>("", "k")?);
    }
    // OTA artifacts are keyed by build, not stored in a column.
    for row in rows(
        "SELECT org_id, device_id, fw_version FROM build_records \
         WHERE ota_sha256 IS NOT NULL AND device_id IS NOT NULL AND fw_version IS NOT NULL",
    )
    .await?
    {
        known.keys.insert(pnex_firmware_builder::ota_artifact_key(
            row.try_get::<i64>("", "org_id")?,
            &row.try_get::<String>("", "device_id")?,
            &row.try_get::<String>("", "fw_version")?,
        ));
    }
    for row in rows("SELECT CAST(id AS TEXT) AS id FROM stitch_jobs").await? {
        known
            .prefixes
            .push(format!("stitch_jobs/{}/", row.try_get::<String>("", "id")?));
    }
    Ok(known)
}

/// Pure comparison of a bucket listing with the database view.
pub fn compare(objects: &[String], known: &Known) -> Report {
    let in_bucket: HashSet<&str> = objects.iter().map(String::as_str).collect();
    let mut orphans: Vec<String> = objects
        .iter()
        .filter(|k| !known.keys.contains(*k) && !known.prefixes.iter().any(|p| k.starts_with(p)))
        .cloned()
        .collect();
    let mut missing: Vec<String> = known
        .keys
        .iter()
        .filter(|k| !in_bucket.contains(k.as_str()))
        .cloned()
        .collect();
    orphans.sort();
    missing.sort();
    Report {
        objects: objects.len(),
        referenced: known.keys.len(),
        orphans,
        missing,
    }
}

pub async fn reconcile(ctx: &AppContext) -> Result<Report, String> {
    let store = S3Config::from_config(&ctx.config).store()?;
    // Bucket listed after the DB read: an object written in between shows
    // as an orphan, never a live row as missing.
    let known = known(&ctx.db).await.map_err(|e| e.to_string())?;
    let objects = store.list_keys().await?;
    Ok(compare(&objects, &known))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orphans_and_missing() {
        let known = Known {
            keys: ["org_1/media/a/1_x.jpg", "org_1/ota/d/9.bin"]
                .map(String::from)
                .into(),
            prefixes: vec!["stitch_jobs/j1/".into()],
        };
        let objects = [
            "org_1/media/a/1_x.jpg",
            "stitch_jobs/j1/frame_000.jpg",
            "stitch_jobs/j2/frame_000.jpg",
            "org_2/media/b/1_y.jpg",
        ]
        .map(String::from);
        let report = compare(&objects, &known);
        assert_eq!(
            report.orphans,
            ["org_2/media/b/1_y.jpg", "stitch_jobs/j2/frame_000.jpg"]
        );
        assert_eq!(report.missing, ["org_1/ota/d/9.bin"]);
        assert_eq!((report.objects, report.referenced), (4, 2));
    }

    /// The SQL runs on the real schema (every table and column exists).
    #[tokio::test]
    async fn known_reads_the_schema() {
        let db = crate::services::db_lock::migrated_test_db().await;
        known(&db).await.expect("known keys");
    }
}
