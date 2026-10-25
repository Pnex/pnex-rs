//! Change feed of the ontology (external-apps.md D435): what changed in an
//! org since a cursor, for applications that keep a copy.
//!
//! Rows come from statement-level triggers on `objects` and
//! `resource_edges` (migration 000054). Order is (transaction id,
//! sequence), and a page only reads transactions older than every one
//! still running: a transaction that commits late has a larger id than
//! anything already served, so no row is ever skipped.
//!
//! The log is compacted: only the latest row of each object or link is
//! kept, and close or delete rows go after the retention. Cursor `0` is
//! therefore a full copy; a cursor behind a removed close is expired.

use std::collections::HashMap;
use std::time::Duration;

use pnex_core::ontology::api::{ChangeFeed, FeedAction, FeedEntry, FeedKind};
use sea_orm::{
    AccessMode, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait,
    FromQueryResult, IsolationLevel, QueryFilter, Statement, TransactionTrait,
};
use uuid::Uuid;

use crate::models::_entities::{objects, resource_edges};

/// Most entries per page.
pub const PAGE_MAX: u64 = 1_000;
/// Close and delete rows older than this are compacted away.
pub const RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);
const COMPACT_EVERY: Duration = Duration::from_secs(3600);

#[derive(Debug, thiserror::Error)]
pub enum FeedError {
    #[error("invalid cursor")]
    InvalidCursor,
    #[error("cursor expired")]
    Expired,
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// A feed position: (transaction id, sequence). `0` is the start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Cursor {
    pub txid: u64,
    pub seq: i64,
}

impl Cursor {
    pub fn parse(raw: &str) -> Result<Self, FeedError> {
        let raw = raw.trim();
        if raw.is_empty() || raw == "0" {
            return Ok(Self::default());
        }
        let (t, s) = raw.split_once('.').ok_or(FeedError::InvalidCursor)?;
        Ok(Self {
            txid: t.parse().map_err(|_| FeedError::InvalidCursor)?,
            seq: s.parse().map_err(|_| FeedError::InvalidCursor)?,
        })
    }
}

impl std::fmt::Display for Cursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if *self == Self::default() {
            f.write_str("0")
        } else {
            write!(f, "{}.{}", self.txid, self.seq)
        }
    }
}

/// What to read: `None` = every type, `Some` = only these (none if empty).
#[derive(Debug, Default)]
pub struct Filter {
    pub types: Option<Vec<String>>,
    pub link_types: Option<Vec<String>>,
}

#[derive(FromQueryResult)]
struct Row {
    seq: i64,
    txid: String,
    at: chrono::DateTime<chrono::FixedOffset>,
    kind: String,
    id: String,
    type_key: String,
    action: String,
}

impl Row {
    fn cursor(&self) -> Cursor {
        Cursor {
            txid: self.txid.parse().unwrap_or_default(),
            seq: self.seq,
        }
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, sql, values)
}

/// Rows a reader may see: from transactions older than every running one.
const SETTLED: &str = "txid < pg_snapshot_xmin(pg_current_snapshot())";

/// One page after `since`, at most `limit` entries.
pub async fn page(
    db: &DatabaseConnection,
    org_id: i64,
    since: Cursor,
    filter: &Filter,
    limit: u64,
) -> Result<ChangeFeed, FeedError> {
    let limit = limit.clamp(1, PAGE_MAX);
    // One snapshot for the rows, the head and the states joined to them.
    let txn = db
        .begin_with_config(
            Some(IsolationLevel::RepeatableRead),
            Some(AccessMode::ReadOnly),
        )
        .await?;
    if since != Cursor::default() {
        let mark = txn
            .query_one_raw(stmt(
                "SELECT 1 AS one FROM ontology_change_marks \
                 WHERE org_id = $1 AND (txid, seq) > ($2::text::xid8, $3)",
                vec![
                    org_id.into(),
                    since.txid.to_string().into(),
                    since.seq.into(),
                ],
            ))
            .await?;
        if mark.is_some() {
            return Err(FeedError::Expired);
        }
    }
    let rows = Row::find_by_statement(stmt(
        &format!(
            "SELECT seq, txid::text AS txid, at, kind, id, type_key, action \
             FROM ontology_changes \
             WHERE org_id = $1 AND (txid, seq) > ($2::text::xid8, $3) AND {SETTLED} \
               AND (kind <> 'object' OR $7 OR type_key = ANY($4)) \
               AND (kind <> 'link' OR $8 OR type_key = ANY($5)) \
             ORDER BY txid, seq LIMIT $6"
        ),
        vec![
            org_id.into(),
            since.txid.to_string().into(),
            since.seq.into(),
            filter.types.clone().unwrap_or_default().into(),
            filter.link_types.clone().unwrap_or_default().into(),
            (limit as i64 + 1).into(),
            filter.types.is_none().into(),
            filter.link_types.is_none().into(),
        ],
    ))
    .all(&txn)
    .await?;
    let more = rows.len() as u64 > limit;
    let rows = &rows[..rows.len().min(limit as usize)];
    let next = match rows.last() {
        Some(r) if more => r.cursor(),
        // A short page: skip past the rows the filter left out too.
        _ => head_in(&txn, org_id).await?.max(since),
    };
    let changes = entries(&txn, org_id, rows).await?;
    txn.commit().await?;
    Ok(ChangeFeed {
        changes,
        next: next.to_string(),
        more,
    })
}

/// The newest settled position of the org (`0` when the feed is empty).
pub async fn head(db: &DatabaseConnection, org_id: i64) -> Result<Cursor, FeedError> {
    head_in(db, org_id).await
}

async fn head_in<C: ConnectionTrait>(db: &C, org_id: i64) -> Result<Cursor, FeedError> {
    let row = Row::find_by_statement(stmt(
        &format!(
            "SELECT seq, txid::text AS txid, at, kind, id, type_key, action \
             FROM ontology_changes WHERE org_id = $1 AND {SETTLED} \
             ORDER BY txid DESC, seq DESC LIMIT 1"
        ),
        vec![org_id.into()],
    ))
    .one(db)
    .await?;
    Ok(row.map(|r| r.cursor()).unwrap_or_default())
}

/// The rows as entries: the last one of each object or link, with its
/// current state.
async fn entries<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    rows: &[Row],
) -> Result<Vec<FeedEntry>, FeedError> {
    let mut last: HashMap<(&str, &str), usize> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        last.insert((r.kind.as_str(), r.id.as_str()), i);
    }
    let kept: Vec<&Row> = rows
        .iter()
        .enumerate()
        .filter(|(i, r)| last.get(&(r.kind.as_str(), r.id.as_str())) == Some(i))
        .map(|(_, r)| r)
        .collect();
    let object_ids: Vec<Uuid> = kept
        .iter()
        .filter(|r| r.kind == "object")
        .filter_map(|r| Uuid::parse_str(&r.id).ok())
        .collect();
    let link_ids: Vec<i64> = kept
        .iter()
        .filter(|r| r.kind == "link")
        .filter_map(|r| r.id.parse().ok())
        .collect();
    let mut objects_by_id = HashMap::new();
    if !object_ids.is_empty() {
        let s = super::schema(db, org_id).await?;
        for o in objects::Entity::find()
            .filter(objects::Column::OrgId.eq(org_id))
            .filter(objects::Column::Id.is_in(object_ids))
            .all(db)
            .await?
        {
            objects_by_id.insert(o.id.to_string(), super::objects::view(&o, &s));
        }
    }
    let mut links_by_id = HashMap::new();
    if !link_ids.is_empty() {
        let edges = resource_edges::Entity::find()
            .filter(resource_edges::Column::OrgId.eq(org_id))
            .filter(resource_edges::Column::Id.is_in(link_ids))
            .all(db)
            .await?;
        for l in super::links::views(db, org_id, &edges).await? {
            links_by_id.insert(l.id.to_string(), l);
        }
    }
    Ok(kept
        .into_iter()
        .map(|r| {
            let kind = if r.kind == "link" {
                FeedKind::Link
            } else {
                FeedKind::Object
            };
            let action = match r.action.as_str() {
                "create" => FeedAction::Create,
                "close" => FeedAction::Close,
                "delete" => FeedAction::Delete,
                _ => FeedAction::Update,
            };
            FeedEntry {
                cursor: r.cursor().to_string(),
                at: r.at.to_rfc3339(),
                kind,
                action,
                object: (kind == FeedKind::Object)
                    .then(|| objects_by_id.remove(&r.id))
                    .flatten(),
                link: (kind == FeedKind::Link)
                    .then(|| links_by_id.remove(&r.id))
                    .flatten(),
                id: r.id.clone(),
                type_key: r.type_key.clone(),
            }
        })
        .collect())
}

/// Compaction: every row superseded by a later one of the same object or
/// link, then close and delete rows past the retention (their position
/// becomes the org's mark). Returns the rows removed.
pub async fn compact(db: &DatabaseConnection) -> Result<u64, DbErr> {
    let superseded = db
        .execute_raw(stmt(
            &format!(
                "DELETE FROM ontology_changes c USING ontology_changes d \
                 WHERE d.org_id = c.org_id AND d.kind = c.kind AND d.id = c.id \
                   AND (d.txid, d.seq) > (c.txid, c.seq) AND d.{SETTLED}"
            ),
            vec![],
        ))
        .await?
        .rows_affected();
    let txn = db.begin().await?;
    txn.execute_raw(stmt(
        "WITH gone AS (
             DELETE FROM ontology_changes
             WHERE action IN ('close', 'delete') AND at < now() - make_interval(secs => $1)
             RETURNING org_id, txid, seq
         ), top AS (
             SELECT DISTINCT ON (org_id) org_id, txid, seq FROM gone
             ORDER BY org_id, txid DESC, seq DESC
         )
         INSERT INTO ontology_change_marks (org_id, txid, seq)
         SELECT org_id, txid, seq FROM top
         ON CONFLICT (org_id) DO UPDATE SET txid = EXCLUDED.txid, seq = EXCLUDED.seq
         WHERE (EXCLUDED.txid, EXCLUDED.seq) > (ontology_change_marks.txid, ontology_change_marks.seq)",
        vec![(RETENTION.as_secs() as f64).into()],
    ))
    .await?;
    txn.commit().await?;
    Ok(superseded)
}

pub fn spawn_compactor(ctx: &loco_rs::app::AppContext) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(90)).await;
        loop {
            if crate::services::singleton::my_turn(
                &ctx.db,
                "ontology-changes-compact",
                COMPACT_EVERY,
            )
            .await
            {
                match compact(&ctx.db).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(rows = n, "ontology change feed compacted"),
                    Err(e) => tracing::warn!(error = %e, "ontology change feed compaction failed"),
                }
            }
            tokio::time::sleep(COMPACT_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_round_trip_and_order() {
        assert_eq!(Cursor::parse("0").unwrap(), Cursor::default());
        assert_eq!(Cursor::parse("").unwrap(), Cursor::default());
        let c = Cursor::parse("812.44").unwrap();
        assert_eq!((c.txid, c.seq), (812, 44));
        assert_eq!(c.to_string(), "812.44");
        assert_eq!(Cursor::default().to_string(), "0");
        assert!(Cursor::parse("812").is_err());
        assert!(Cursor::parse("a.1").is_err());
        assert!(Cursor { txid: 9, seq: 100 } < Cursor { txid: 10, seq: 1 });
    }
}
