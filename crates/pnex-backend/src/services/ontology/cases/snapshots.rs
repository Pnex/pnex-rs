//! Frozen states of a case (cases-workspace.md K33): the export of the case,
//! its reviewed detections and its generalised tracks, stored as canonical
//! JSON with its SHA-256 and an increasing number. A frozen state is never
//! modified (no route writes one twice) and reads back byte for byte, even
//! once the telemetry it was drawn from has expired. Purging a person drops
//! the frozen states that mention them (K16).

use pnex_core::err_codes::FIELD_INVALID;
use pnex_core::ontology::api::{
    CaseSnapshot, CaseSnapshotContent, CaseSnapshotFull, CaseSnapshotInput, CaseTracksQuery,
};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, QueryResult, Statement, TransactionTrait,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::super::{invalid, ts, Actor, OntologyError, Result};
use super::signals::events_span;
use super::tracks::MAX_SPAN_DAYS;
use super::{case_of, export, membership, proposals, tracks};
use crate::services::openobserve::Client;

/// Tracks kept in a frozen state at most, serialised.
const MAX_TRACK_BYTES: usize = 2 * 1024 * 1024;
const LABEL_MAX: usize = 255;
/// Around the events: the tracks just before and after them.
const TRACK_PAD_MIN: i64 = 60;

const COLUMNS: &str = "s.seq, s.label, s.taken_at, s.sha256, s.size_bytes, \
                       COALESCE(u.full_name, u.email) AS taken_by";

fn row_of(r: &QueryResult) -> Option<CaseSnapshot> {
    let size: i32 = r.try_get("", "size_bytes").ok()?;
    Some(CaseSnapshot {
        seq: r.try_get("", "seq").ok()?,
        label: r.try_get("", "label").ok()?,
        taken_at: ts(&r.try_get::<DateTimeWithTimeZone>("", "taken_at").ok()?),
        taken_by: r.try_get("", "taken_by").ok()?,
        sha256: r.try_get("", "sha256").ok()?,
        size_bytes: size as usize,
    })
}

/// Freezes the case as it is now.
pub async fn freeze(
    db: &DatabaseConnection,
    client: Option<&Client>,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    input: &CaseSnapshotInput,
) -> Result<CaseSnapshot> {
    let label = input
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty());
    if label.is_some_and(|l| l.len() > LABEL_MAX) {
        return Err(invalid("label", format!("max_length:{LABEL_MAX}")));
    }
    let export = export(db, org_id, case_id).await?;
    let reviewed = proposals(db, org_id, case_id)
        .await?
        .into_iter()
        .filter(|p| p.status != "proposed")
        .collect();
    let m = membership(db, org_id, case_id).await?;
    let events: Vec<_> = m.events.iter().map(|(_, o)| o.clone()).collect();
    let mut tracks_out = None;
    let mut tracks_omitted = false;
    if let Some((from, to)) = events_span(&events) {
        let pad = chrono::Duration::minutes(TRACK_PAD_MIN);
        let (from, to) = (from - pad, to + pad);
        if to - from <= chrono::Duration::days(MAX_SPAN_DAYS) {
            let q = CaseTracksQuery {
                from: ts(&from),
                to: ts(&to),
                // About one point per pixel of a 1000 px wide timeline.
                resolution_ms: Some((to - from).num_milliseconds() / 1_000),
                signal_gap_s: None,
            };
            let reader = super::reads::Reader {
                user_id: actor.user_id,
                purpose: "snapshot",
            };
            let t = tracks(db, client, org_id, reader, case_id, &q).await?;
            let size = serde_json::to_vec(&t)
                .map(|b| b.len())
                .unwrap_or(usize::MAX);
            if size > MAX_TRACK_BYTES {
                tracks_omitted = true;
            } else if t.available {
                tracks_out = Some(t);
            }
        } else {
            tracks_omitted = true;
        }
    }
    let content = CaseSnapshotContent {
        export,
        proposals: reviewed,
        tracks: tracks_out,
        tracks_omitted,
    }
    .canonical();
    let sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));
    let size = content.len() as i32;

    let txn = db.begin().await?;
    // The case row orders concurrent freezes: one number each.
    txn.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT id FROM objects WHERE org_id = $1 AND id = $2 FOR UPDATE",
        [org_id.into(), case_id.into()],
    ))
    .await?
    .ok_or(OntologyError::NotFound)?;
    let r = txn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO case_snapshots (id, org_id, case_id, seq, label, taken_by, content, sha256, size_bytes) \
             SELECT $1, $2, $3, COALESCE(MAX(seq), 0) + 1, $4, $5, $6, $7, $8 \
             FROM case_snapshots WHERE case_id = $3 \
             RETURNING seq",
            [
                Uuid::new_v4().into(),
                org_id.into(),
                case_id.into(),
                label.map(str::to_string).into(),
                actor.user_id.into(),
                content.into(),
                sha256.into(),
                size.into(),
            ],
        ))
        .await?
        .ok_or(OntologyError::NotFound)?;
    txn.commit().await?;
    let seq: i32 = r.try_get("", "seq")?;
    Ok(snapshot(db, org_id, case_id, seq).await?.snapshot)
}

/// Frozen states of a case, newest first.
pub async fn snapshots(
    db: &DatabaseConnection,
    org_id: i64,
    case_id: Uuid,
) -> Result<Vec<CaseSnapshot>> {
    case_of(db, org_id, case_id).await?;
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!(
                "SELECT {COLUMNS} FROM case_snapshots s LEFT JOIN users u ON u.id = s.taken_by \
                 WHERE s.org_id = $1 AND s.case_id = $2 ORDER BY s.seq DESC"
            ),
            [org_id.into(), case_id.into()],
        ))
        .await?;
    Ok(rows.iter().filter_map(row_of).collect())
}

/// One frozen state with its content, as fingerprinted.
pub async fn snapshot(
    db: &DatabaseConnection,
    org_id: i64,
    case_id: Uuid,
    seq: i32,
) -> Result<CaseSnapshotFull> {
    if seq < 1 {
        return Err(invalid("seq", FIELD_INVALID));
    }
    case_of(db, org_id, case_id).await?;
    let r = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!(
                "SELECT {COLUMNS}, s.content FROM case_snapshots s LEFT JOIN users u ON u.id = s.taken_by \
                 WHERE s.org_id = $1 AND s.case_id = $2 AND s.seq = $3"
            ),
            [org_id.into(), case_id.into(), seq.into()],
        ))
        .await?
        .ok_or(OntologyError::NotFound)?;
    Ok(CaseSnapshotFull {
        snapshot: row_of(&r).ok_or(OntologyError::NotFound)?,
        content: r.try_get("", "content")?,
    })
}

/// Drops the frozen states that mention `object_id` (a purged person).
pub(super) async fn forget<C: ConnectionTrait>(db: &C, org_id: i64, object_id: Uuid) -> Result<()> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "DELETE FROM case_snapshots WHERE org_id = $1 AND strpos(content, $2) > 0",
        [org_id.into(), object_id.to_string().into()],
    ))
    .await?;
    Ok(())
}
