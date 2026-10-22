//! Imported tracks of a case (cases-workspace.md K26): a GPX or CSV file of
//! past positions put onto a case member, previewed (points, span, rejected
//! rows and why) before it is written. The points live in Postgres as case
//! evidence: no ingestion window and no telemetry retention apply, and they
//! go when the member object goes (purge). The same file onto the same
//! member is one import (SHA-256).
//! shortcut: the original file is not stored, only its SHA-256; keep it in
//! the media store if a case must hand over the file itself.

use chrono_tz::Tz;
use pnex_core::err_codes::{
    FIELD_INVALID, FIELD_REQUIRED, TRACK_IMPORT_EMPTY, TRACK_IMPORT_TIMEZONE_REQUIRED,
    TRACK_IMPORT_UNREADABLE,
};
use pnex_core::ontology::api::{TrackImport, TrackImportInput, TrackImportPreview};
use pnex_core::track::TrackPoint;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, QueryResult, Statement, TransactionTrait,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::super::{invalid, links, objects, ts, Actor, OntologyError, Result};
use super::{guard_person, membership};
use crate::services::pois::track_import::{self, Format, ImportError};

/// Largest file accepted, in bytes of text.
pub const MAX_BYTES: usize = 20 * 1024 * 1024;
const MAX_POINTS: usize = 500_000;
/// Rejected rows listed in a preview (the count is always complete).
const REJECTED_SHOWN: usize = 100;
/// Points written per statement.
const CHUNK: usize = 10_000;

fn ms_ts(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}

fn row_of(r: &QueryResult, object: pnex_core::ontology::api::ObjectRef) -> Option<TrackImport> {
    let points: i32 = r.try_get("", "point_count").ok()?;
    let rejected: i32 = r.try_get("", "rejected_count").ok()?;
    Some(TrackImport {
        id: r.try_get::<Uuid>("", "id").ok()?.to_string(),
        object,
        file_name: r.try_get("", "file_name").ok()?,
        format: r.try_get("", "format").ok()?,
        sha256: r.try_get("", "sha256").ok()?,
        time_zone: r.try_get("", "time_zone").ok()?,
        points: points as usize,
        rejected: rejected as usize,
        t_start: ts(&r.try_get::<DateTimeWithTimeZone>("", "t_start").ok()?),
        t_end: ts(&r.try_get::<DateTimeWithTimeZone>("", "t_end").ok()?),
        imported_at: ts(&r.try_get::<DateTimeWithTimeZone>("", "imported_at").ok()?),
    })
}

const COLUMNS: &str = "id, object_id, file_name, format, sha256, time_zone, point_count, \
                       rejected_count, t_start, t_end, imported_at";

/// Previews or writes an import onto a member of the case.
pub async fn import(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    input: &TrackImportInput,
) -> Result<TrackImportPreview> {
    crate::services::ontology::changes::deferred(async {
        let m = membership(db, org_id, case_id).await?;
        let object_id = Uuid::parse_str(&input.object_id).map_err(|_| OntologyError::NotFound)?;
        if !m.member_ids().contains(&object_id) {
            return Err(OntologyError::NotFound);
        }
        let obj = objects::find(db, org_id, object_id).await?;
        // K38: a person's movements are personal data, as its attachment is.
        guard_person(&m.case, &obj)?;
        let format = Format::parse(&input.format).ok_or_else(|| invalid("format", FIELD_INVALID))?;
        let file_name = input.file_name.trim();
        if file_name.is_empty() {
            return Err(invalid("file_name", FIELD_REQUIRED));
        }
        if file_name.len() > 255 {
            return Err(invalid("file_name", "max_length:255"));
        }
        if input.content.len() > MAX_BYTES {
            return Err(invalid("content", format!("max_length:{MAX_BYTES}")));
        }
        let tz: Option<Tz> = match input
            .time_zone
            .as_deref()
            .map(str::trim)
            .filter(|z| !z.is_empty())
        {
            Some(z) => Some(z.parse().map_err(|_| invalid("time_zone", FIELD_INVALID))?),
            None => None,
        };
        let sha256 = format!("{:x}", Sha256::digest(input.content.as_bytes()));
        let parsed = track_import::parse(format, &input.content)
            .map_err(|_| OntologyError::TrackImport(TRACK_IMPORT_UNREADABLE))?;
        let needs_time_zone = parsed.needs_time_zone();
        let (points, rejected) = match parsed.resolve(tz) {
            Ok(r) => r,
            Err(ImportError::TimezoneRequired) if input.dry_run => {
                return Ok(TrackImportPreview {
                    needs_time_zone: true,
                    sha256,
                    ..Default::default()
                })
            }
            Err(ImportError::TimezoneRequired) => {
                return Err(OntologyError::TrackImport(TRACK_IMPORT_TIMEZONE_REQUIRED))
            }
            Err(ImportError::Unreadable) => {
                return Err(OntologyError::TrackImport(TRACK_IMPORT_UNREADABLE))
            }
        };
        if points.len() > MAX_POINTS {
            return Err(invalid("content", format!("max_points:{MAX_POINTS}")));
        }
        let mut preview = TrackImportPreview {
            points: points.len(),
            rejected_count: rejected.len(),
            rejected: rejected.iter().take(REJECTED_SHOWN).copied().collect(),
            t_start: points.first().map(|p| ms_ts(p.t_ms)),
            t_end: points.last().map(|p| ms_ts(p.t_ms)),
            needs_time_zone,
            sha256: sha256.clone(),
            import: None,
        };
        if input.dry_run {
            return Ok(preview);
        }
        let (Some(first), Some(last)) = (points.first(), points.last()) else {
            return Err(OntologyError::TrackImport(TRACK_IMPORT_EMPTY));
        };
        let object = links::object_ref(&obj);
        // Same file onto the same member: the existing import.
        if let Some(r) = db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                format!("SELECT {COLUMNS} FROM track_imports WHERE org_id = $1 AND object_id = $2 AND sha256 = $3"),
                [org_id.into(), object_id.into(), sha256.clone().into()],
            ))
            .await?
        {
            preview.import = row_of(&r, object);
            return Ok(preview);
        }
        let id = Uuid::new_v4();
        let txn = db.begin().await?;
        txn.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO track_imports (id, org_id, object_id, file_name, format, sha256, time_zone, \
               point_count, rejected_count, t_start, t_end, imported_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, to_timestamp($10 / 1000.0), to_timestamp($11 / 1000.0), $12)",
            [
                id.into(),
                org_id.into(),
                object_id.into(),
                file_name.into(),
                input.format.clone().into(),
                sha256.into(),
                tz.map(|z| z.name().to_string()).into(),
                (points.len() as i32).into(),
                (rejected.len() as i32).into(),
                first.t_ms.into(),
                last.t_ms.into(),
                actor.user_id.into(),
            ],
        ))
        .await?;
        for chunk in points.chunks(CHUNK) {
            insert_points(&txn, id, chunk).await?;
        }
        txn.commit().await?;
            crate::services::ontology::changes::flush();
        let r = db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                format!("SELECT {COLUMNS} FROM track_imports WHERE id = $1"),
                [id.into()],
            ))
            .await?
            .ok_or(OntologyError::NotFound)?;
        preview.import = row_of(&r, object);
        Ok(preview)
    })
    .await
}

/// One multi-row insert through arrays (a missing altitude travels as NaN).
async fn insert_points<C: ConnectionTrait>(db: &C, id: Uuid, points: &[TrackPoint]) -> Result<()> {
    let t: Vec<i64> = points.iter().map(|p| p.t_ms).collect();
    let lon: Vec<f64> = points.iter().map(|p| p.lon).collect();
    let lat: Vec<f64> = points.iter().map(|p| p.lat).collect();
    let alt: Vec<f64> = points.iter().map(|p| p.alt.unwrap_or(f64::NAN)).collect();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO track_points (import_id, t, lon, lat, alt) \
         SELECT $1, to_timestamp(u.t / 1000.0), u.lon, u.lat, NULLIF(u.alt, 'NaN') \
         FROM UNNEST($2::bigint[], $3::float8[], $4::float8[], $5::float8[]) AS u(t, lon, lat, alt)",
        [id.into(), t.into(), lon.into(), lat.into(), alt.into()],
    ))
    .await?;
    Ok(())
}

/// The imports onto the members of a case, newest first.
pub async fn imports(
    db: &DatabaseConnection,
    org_id: i64,
    case_id: Uuid,
) -> Result<Vec<TrackImport>> {
    let ids = membership(db, org_id, case_id).await?.member_ids();
    let objs = links::refs(db, org_id, ids.iter().copied()).await?;
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!(
                "SELECT {COLUMNS} FROM track_imports WHERE org_id = $1 AND object_id = ANY($2) \
                 ORDER BY imported_at DESC, id"
            ),
            [org_id.into(), ids.into()],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let o = objs.get(&r.try_get::<Uuid>("", "object_id").ok()?)?;
            row_of(r, links::object_ref(o))
        })
        .collect())
}

/// Deletes an import of a member of the case (a wrong file, a wrong zone).
pub async fn delete_import(
    db: &DatabaseConnection,
    org_id: i64,
    case_id: Uuid,
    id: Uuid,
) -> Result<()> {
    let ids = membership(db, org_id, case_id).await?.member_ids();
    let r = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM track_imports WHERE id = $1 AND org_id = $2 AND object_id = ANY($3)",
            [id.into(), org_id.into(), ids.into()],
        ))
        .await?;
    if r.rows_affected() == 0 {
        return Err(OntologyError::NotFound);
    }
    Ok(())
}

/// Imported points of `objects` over `[from_ms, to_ms]`, by object, sorted.
pub(super) async fn points_of(
    db: &DatabaseConnection,
    org_id: i64,
    objects: &[Uuid],
    (from_ms, to_ms): (i64, i64),
    cap: i64,
) -> Result<Vec<(Uuid, TrackPoint)>> {
    if objects.is_empty() {
        return Ok(Vec::new());
    }
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT i.object_id, (extract(epoch FROM p.t) * 1000)::bigint AS t_ms, p.lon, p.lat, p.alt \
             FROM track_points p JOIN track_imports i ON i.id = p.import_id \
             WHERE i.org_id = $1 AND i.object_id = ANY($2) \
               AND p.t >= to_timestamp($3 / 1000.0) AND p.t <= to_timestamp($4 / 1000.0) \
             ORDER BY p.t LIMIT $5",
            [org_id.into(), objects.to_vec().into(), from_ms.into(), to_ms.into(), cap.into()],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some((
                r.try_get("", "object_id").ok()?,
                TrackPoint {
                    t_ms: r.try_get("", "t_ms").ok()?,
                    lon: r.try_get("", "lon").ok()?,
                    lat: r.try_get("", "lat").ok()?,
                    alt: r.try_get("", "alt").ok()?,
                },
            ))
        })
        .collect())
}
