//! Spans of a media attributed to persons (media-review.md D272, D273):
//! "from 12:30 to 14:10, this is X", whatever found it (a person, a voice
//! profile, the transcript, the grid). Small rows, edited by people.
//!
//! - The org comes from the caller (R1); the person must be a `person`
//!   object of the same org, not archived.
//! - A new confirmed span of a person merges with the confirmed spans of
//!   the same person it overlaps: one person, one continuous span.
//! - Edits carry `expected_version` (optimistic concurrency, 409).

use pnex_core::media_review::{
    merge_spans, AttributionInput, AttributionOrigin, AttributionPatch, AttributionStatus,
    MediaSpan, SpeechAttribution, ATTRIBUTION_SPANS_MAX, MEDIA_CLOCK_MAX_MS,
};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbErr, FromQueryResult, Statement, TransactionTrait,
};
use uuid::Uuid;

use crate::services::ontology::{self, Actor};

#[derive(Debug, thiserror::Error)]
pub enum AttributionError {
    #[error("unknown media asset or attribution")]
    NotFound,
    #[error("not a person of the organization")]
    NotPerson,
    #[error("invalid spans")]
    Spans,
    #[error("the attribution changed meanwhile")]
    Conflict,
    #[error(transparent)]
    Db(#[from] DbErr),
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(sea_orm::DbBackend::Postgres, sql, values)
}

#[derive(Debug, FromQueryResult)]
struct Row {
    id: Uuid,
    object_id: Uuid,
    person: String,
    start_ms: i64,
    end_ms: i64,
    origin: String,
    confidence: Option<f32>,
    status: String,
    version: i32,
    updated_at: chrono::DateTime<chrono::FixedOffset>,
}

impl Row {
    fn view(self) -> Option<SpeechAttribution> {
        Some(SpeechAttribution {
            id: self.id,
            object_id: self.object_id,
            person: self.person,
            start_ms: self.start_ms,
            end_ms: self.end_ms,
            origin: AttributionOrigin::from_wire(&self.origin)?,
            confidence: self.confidence,
            status: AttributionStatus::from_wire(&self.status)?,
            version: self.version,
            updated_at: self.updated_at.to_rfc3339(),
        })
    }
}

const SELECT: &str = "SELECT a.id, a.object_id, o.title AS person, a.start_ms, a.end_ms,
    a.origin, a.confidence, a.status, a.version, a.updated_at
    FROM speech_attributions a JOIN objects o ON o.id = a.object_id";

/// Attributions of an asset of the org, in media order.
pub async fn list<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    asset_id: Uuid,
) -> Result<Vec<SpeechAttribution>, DbErr> {
    let rows = Row::find_by_statement(stmt(
        &format!(
            "{SELECT} WHERE a.org_id = $1 AND a.media_asset_id = $2 ORDER BY a.start_ms, a.id"
        ),
        vec![org_id.into(), asset_id.into()],
    ))
    .all(db)
    .await?;
    Ok(rows.into_iter().filter_map(Row::view).collect())
}

async fn find<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    asset_id: Uuid,
    id: Uuid,
) -> Result<SpeechAttribution, AttributionError> {
    Row::find_by_statement(stmt(
        &format!("{SELECT} WHERE a.org_id = $1 AND a.media_asset_id = $2 AND a.id = $3"),
        vec![org_id.into(), asset_id.into(), id.into()],
    ))
    .one(db)
    .await?
    .and_then(Row::view)
    .ok_or(AttributionError::NotFound)
}

async fn check_asset<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    asset_id: Uuid,
) -> Result<(), AttributionError> {
    db.query_one_raw(stmt(
        "SELECT 1 AS one FROM media_assets WHERE id = $1 AND org_id = $2",
        vec![asset_id.into(), org_id.into()],
    ))
    .await?
    .map(|_| ())
    .ok_or(AttributionError::NotFound)
}

/// Whether `object_id` is a live `person` of the org.
async fn is_person<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    object_id: Uuid,
) -> Result<bool, DbErr> {
    Ok(db
        .query_one_raw(stmt(
            "SELECT 1 AS one FROM objects
             WHERE id = $1 AND org_id = $2 AND type_key = 'person' AND valid_to IS NULL",
            vec![object_id.into(), org_id.into()],
        ))
        .await?
        .is_some())
}

fn span_ok(s: &MediaSpan) -> bool {
    s.start_ms >= 0 && s.end_ms > s.start_ms && s.end_ms <= MEDIA_CLOCK_MAX_MS
}

/// Attributes spans of an asset to a person, confirmed (D273): merged
/// with the confirmed spans of that person they overlap. Returns the
/// asset's attributions after the change.
pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    asset_id: Uuid,
    input: &AttributionInput,
) -> Result<Vec<SpeechAttribution>, AttributionError> {
    if input.spans.is_empty()
        || input.spans.len() > ATTRIBUTION_SPANS_MAX
        || !input.spans.iter().all(span_ok)
    {
        return Err(AttributionError::Spans);
    }
    let txn = db.begin().await?;
    check_asset(&txn, org_id, asset_id).await?;
    if !is_person(&txn, org_id, input.object_id).await? {
        return Err(AttributionError::NotPerson);
    }
    let mut spans = merge_spans(input.spans.clone());
    // Confirmed spans of the same person touching the new ones join them.
    let mine: Vec<(Uuid, i64, i64)> = Row::find_by_statement(stmt(
        &format!(
            "{SELECT} WHERE a.org_id = $1 AND a.media_asset_id = $2 AND a.object_id = $3
               AND a.status = 'confirmed'"
        ),
        vec![org_id.into(), asset_id.into(), input.object_id.into()],
    ))
    .all(&txn)
    .await?
    .into_iter()
    .map(|r| (r.id, r.start_ms, r.end_ms))
    .collect();
    let touched: Vec<(Uuid, i64, i64)> = mine
        .into_iter()
        .filter(|(_, s, e)| spans.iter().any(|n| n.start_ms <= *e && *s <= n.end_ms))
        .collect();
    spans.extend(touched.iter().map(|(_, s, e)| MediaSpan {
        start_ms: *s,
        end_ms: *e,
    }));
    let spans = merge_spans(spans);
    for (id, _, _) in &touched {
        txn.execute_raw(stmt(
            "DELETE FROM speech_attributions WHERE id = $1",
            vec![(*id).into()],
        ))
        .await?;
    }
    for s in &spans {
        txn.execute_raw(stmt(
            "INSERT INTO speech_attributions (id, org_id, media_asset_id, object_id, start_ms,
                end_ms, origin, status, created_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            vec![
                Uuid::new_v4().into(),
                org_id.into(),
                asset_id.into(),
                input.object_id.into(),
                s.start_ms.into(),
                s.end_ms.into(),
                AttributionOrigin::Manual.wire().into(),
                AttributionStatus::Confirmed.wire().into(),
                actor.user_id.into(),
            ],
        ))
        .await?;
    }
    sync_appearance(&txn, org_id, actor, asset_id, input.object_id).await?;
    let out = list(&txn, org_id, asset_id).await?;
    txn.commit().await?;
    Ok(out)
}

/// Confirms, rejects or moves an attribution written at `expected_version`.
pub async fn patch(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    asset_id: Uuid,
    id: Uuid,
    p: &AttributionPatch,
) -> Result<SpeechAttribution, AttributionError> {
    let cur = find(db, org_id, asset_id, id).await?;
    let span = MediaSpan {
        start_ms: p.start_ms.unwrap_or(cur.start_ms),
        end_ms: p.end_ms.unwrap_or(cur.end_ms),
    };
    if !span_ok(&span) {
        return Err(AttributionError::Spans);
    }
    let status = p.status.unwrap_or(cur.status);
    let done = db
        .execute_raw(stmt(
            "UPDATE speech_attributions SET start_ms = $5, end_ms = $6, status = $7,
                version = version + 1, updated_at = now()
             WHERE id = $1 AND org_id = $2 AND media_asset_id = $3 AND version = $4",
            vec![
                id.into(),
                org_id.into(),
                asset_id.into(),
                p.expected_version.into(),
                span.start_ms.into(),
                span.end_ms.into(),
                status.wire().into(),
            ],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(AttributionError::Conflict);
    }
    sync_appearance(db, org_id, actor, asset_id, cur.object_id).await?;
    find(db, org_id, asset_id, id).await
}

/// Removes an attribution of an asset of the org.
pub async fn delete(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    asset_id: Uuid,
    id: Uuid,
) -> Result<(), AttributionError> {
    let cur = find(db, org_id, asset_id, id).await?;
    let done = db
        .execute_raw(stmt(
            "DELETE FROM speech_attributions WHERE id = $1 AND org_id = $2 AND media_asset_id = $3",
            vec![id.into(), org_id.into(), asset_id.into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(AttributionError::NotFound);
    }
    sync_appearance(db, org_id, actor, asset_id, cur.object_id).await
}

/// Keeps the ontology in step (D278, cases.md K17): a person with a
/// confirmed span of the media has one open `appears_in` link to it
/// (`kind = speaking`), closed when no confirmed span is left. Without the
/// link type (investigation pack absent or older) nothing is written.
async fn sync_appearance<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    actor: &Actor,
    asset_id: Uuid,
    person: Uuid,
) -> Result<(), AttributionError> {
    let confirmed = db
        .query_one_raw(stmt(
            "SELECT 1 AS one FROM speech_attributions
             WHERE org_id = $1 AND media_asset_id = $2 AND object_id = $3 AND status = 'confirmed'
             LIMIT 1",
            vec![org_id.into(), asset_id.into(), person.into()],
        ))
        .await?
        .is_some();
    let Some(media) = db
        .query_one_raw(stmt(
            "SELECT object_id FROM media_assets WHERE id = $1 AND org_id = $2",
            vec![asset_id.into(), org_id.into()],
        ))
        .await?
        .and_then(|r| r.try_get::<Option<Uuid>>("", "object_id").ok().flatten())
    else {
        return Ok(());
    };
    let open: Option<i64> = db
        .query_one_raw(stmt(
            "SELECT id FROM resource_edges
             WHERE org_id = $1 AND relation = $2 AND source_object_id = $3
               AND target_object_id = $4 AND valid_to IS NULL",
            vec![
                org_id.into(),
                APPEARS_IN.into(),
                person.into(),
                media.into(),
            ],
        ))
        .await?
        .and_then(|r| r.try_get("", "id").ok());
    let outcome = match (confirmed, open) {
        (true, None) => {
            let mut attributes = serde_json::Map::new();
            attributes.insert("kind".into(), "speaking".into());
            attributes.insert("origin".into(), AttributionOrigin::Manual.wire().into());
            let input = pnex_core::ontology::api::LinkInput {
                link_type: APPEARS_IN.into(),
                source_id: person.to_string(),
                target_id: media.to_string(),
                attributes,
                valid_from: None,
                valid_to: None,
            };
            ontology::links::create(db, org_id, actor, &input)
                .await
                .map(|_| ())
        }
        (false, Some(id)) => ontology::links::close(db, org_id, actor, id, None)
            .await
            .map(|_| ()),
        _ => Ok(()),
    };
    match outcome {
        Ok(()) => Ok(()),
        // No such link type in the org (pack absent or older): the spans stay.
        Err(ontology::OntologyError::Invalid { .. }) => Ok(()),
        Err(ontology::OntologyError::Db(e)) => Err(AttributionError::Db(e)),
        Err(e) => {
            tracing::warn!(error = %e, "appearance not written");
            Ok(())
        }
    }
}

/// Link type of a person appearing in a media file (investigation pack).
const APPEARS_IN: &str = "appears_in";

/// Replaces the voice proposals of an asset (D275): one proposed span per
/// merged turn of each matched speaker, except where a person already
/// confirmed someone, or rejected this very person. Returns how many.
pub async fn propose_voices(
    db: &DatabaseConnection,
    org_id: i64,
    asset_id: Uuid,
    segments: &[pnex_core::media_review::ReviewSegment],
    matches: &[crate::services::voice_id::media::SpeakerMatch],
) -> Result<usize, AttributionError> {
    let txn = db.begin().await?;
    txn.execute_raw(stmt(
        "DELETE FROM speech_attributions
         WHERE org_id = $1 AND media_asset_id = $2 AND origin = 'voice' AND status = 'proposed'",
        vec![org_id.into(), asset_id.into()],
    ))
    .await?;
    let existing = list(&txn, org_id, asset_id).await?;
    let mut count = 0;
    for m in matches {
        for span in pnex_core::media_review::label_spans(segments, &m.label) {
            let overlaps =
                |a: &SpeechAttribution| a.start_ms < span.end_ms && span.start_ms < a.end_ms;
            let decided = existing.iter().any(|a| {
                overlaps(a)
                    && (a.status == AttributionStatus::Confirmed
                        || (a.status == AttributionStatus::Rejected && a.object_id == m.object_id))
            });
            if decided || !span_ok(&span) {
                continue;
            }
            txn.execute_raw(stmt(
                "INSERT INTO speech_attributions (id, org_id, media_asset_id, object_id, start_ms,
                    end_ms, origin, confidence, status)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                vec![
                    Uuid::new_v4().into(),
                    org_id.into(),
                    asset_id.into(),
                    m.object_id.into(),
                    span.start_ms.into(),
                    span.end_ms.into(),
                    AttributionOrigin::Voice.wire().into(),
                    m.score.into(),
                    AttributionStatus::Proposed.wire().into(),
                ],
            ))
            .await?;
            count += 1;
        }
    }
    txn.commit().await?;
    Ok(count)
}

/// Persons expected in a media (D274): those appearing in it, its
/// publisher, and those already attributed (confirmed).
pub async fn expected_persons<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    asset_id: Uuid,
) -> Result<Vec<Uuid>, DbErr> {
    #[derive(FromQueryResult)]
    struct P {
        id: Uuid,
    }
    let rows = P::find_by_statement(stmt(
        "SELECT e.source_object_id AS id FROM resource_edges e JOIN media_assets m
            ON m.object_id = e.target_object_id
         WHERE m.id = $2 AND e.org_id = $1 AND e.relation = 'appears_in' AND e.valid_to IS NULL
           AND e.source_object_id IS NOT NULL
         UNION
         SELECT e.target_object_id FROM resource_edges e JOIN media_assets m
            ON m.object_id = e.source_object_id
         WHERE m.id = $2 AND e.org_id = $1 AND e.relation = 'published_by' AND e.valid_to IS NULL
           AND e.target_object_id IS NOT NULL
         UNION
         SELECT object_id FROM speech_attributions
         WHERE org_id = $1 AND media_asset_id = $2 AND status = 'confirmed'",
        vec![org_id.into(), asset_id.into()],
    ))
    .all(db)
    .await?;
    Ok(rows.into_iter().map(|p| p.id).collect())
}
