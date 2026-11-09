//! Shared inputs of the causal chain detector (cases-workspace.md D905,
//! markets-research.md H.7): the links that propagate a failure, as they
//! were known at an instant, and the pairs in force for the org.

use chrono::{DateTime, Utc};
use pnex_core::rca::{Edge, Topology};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, Statement};

/// Link types of the org flagged `propagates_failure`.
pub async fn propagating_relations(
    db: &DatabaseConnection,
    org_id: i64,
) -> Result<Vec<String>, DbErr> {
    let s = crate::services::ontology::schema(db, org_id).await?;
    Ok(s.links
        .iter()
        .filter(|(l, _, _)| l.propagates_failure)
        .map(|(l, _, _)| l.key.clone())
        .collect())
}

/// The propagating links open at some point of `[from, as_of]`, as known at
/// `as_of`: a link opened later is absent, a later closing is ignored.
pub async fn topology(
    db: &DatabaseConnection,
    org_id: i64,
    from: DateTime<Utc>,
    as_of: DateTime<Utc>,
) -> Result<Topology, DbErr> {
    let relations = propagating_relations(db, org_id).await?;
    if relations.is_empty() {
        return Ok(Topology::new(Vec::new()));
    }
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT source_object_id, target_object_id, valid_from, valid_to FROM resource_edges \
             WHERE org_id = $1 AND relation = ANY($2) \
               AND source_object_id IS NOT NULL AND target_object_id IS NOT NULL \
               AND valid_from <= $4 AND (valid_to IS NULL OR valid_to > $3)",
            vec![org_id.into(), relations.into(), from.into(), as_of.into()],
        ))
        .await?;
    let mut edges = Vec::with_capacity(rows.len());
    for r in rows {
        let source: uuid::Uuid = r.try_get("", "source_object_id")?;
        let target: uuid::Uuid = r.try_get("", "target_object_id")?;
        let valid_from: DateTime<chrono::FixedOffset> = r.try_get("", "valid_from")?;
        let valid_to: Option<DateTime<chrono::FixedOffset>> = r.try_get("", "valid_to")?;
        edges.push(Edge {
            source: source.to_string(),
            target: target.to_string(),
            from: valid_from.timestamp(),
            to: valid_to.filter(|t| *t <= as_of).map(|t| t.timestamp()),
        });
    }
    Ok(Topology::new(edges))
}
