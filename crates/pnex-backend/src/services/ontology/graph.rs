//! Graph store of the ontology (annex A3): a subgraph around an object is
//! loaded by one recursive CTE (links valid now, both directions, bounded
//! depth, row cap, statement timeout), then handed to the pure algorithms
//! of `pnex_core::ontology::graph` (petgraph). The public API stays bounded
//! (D185); deeper walks are for internal callers.

use std::collections::BTreeSet;

use pnex_core::ontology::api::Neighborhood;
use pnex_core::ontology::graph::{Edge, Subgraph};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Statement, TransactionTrait,
};
use uuid::Uuid;

use super::{links, OntologyError, Result};
use crate::models::_entities::resource_edges;

/// Hard caps of a load: edges and wall time.
const EDGES_MAX: i64 = 5_000;
const TIMEOUT_MS: u32 = 3_000;

/// Edge ids reachable from `start` within `depth` hops (both directions).
async fn reachable_edges(
    db: &DatabaseConnection,
    org_id: i64,
    start: Uuid,
    depth: u32,
) -> Result<Vec<i64>> {
    let txn = db.begin().await?;
    // Scoped to this transaction: a runaway walk cannot hold the pool.
    txn.execute_unprepared(&format!("SET LOCAL statement_timeout = {TIMEOUT_MS}"))
        .await?;
    let rows = txn
        .query_all_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "WITH RECURSIVE walk(node, depth) AS ( \
               SELECT $2::uuid, 0 \
               UNION \
               SELECT CASE WHEN e.source_object_id = w.node THEN e.target_object_id \
                           ELSE e.source_object_id END, w.depth + 1 \
               FROM walk w JOIN resource_edges e \
                 ON (e.source_object_id = w.node OR e.target_object_id = w.node) \
               WHERE e.org_id = $1 AND e.valid_to IS NULL AND w.depth < $3 \
                 AND e.source_object_id IS NOT NULL AND e.target_object_id IS NOT NULL) \
             SELECT DISTINCT e.id FROM resource_edges e \
             JOIN walk w ON e.source_object_id = w.node OR e.target_object_id = w.node \
             WHERE e.org_id = $1 AND e.valid_to IS NULL AND w.depth < $3 \
             LIMIT $4",
            [
                org_id.into(),
                start.into(),
                (depth as i32).into(),
                EDGES_MAX.into(),
            ],
        ))
        .await?;
    txn.commit().await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.try_get::<i64>("", "id").ok())
        .collect())
}

async fn load(
    db: &DatabaseConnection,
    org_id: i64,
    start: Uuid,
    depth: u32,
) -> Result<Vec<resource_edges::Model>> {
    use sea_orm::ColumnTrait;
    super::objects::find(db, org_id, start).await?;
    let ids = reachable_edges(db, org_id, start, depth).await?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(resource_edges::Entity::find()
        .filter(resource_edges::Column::Id.is_in(ids))
        .all(db)
        .await?)
}

fn subgraph(edges: &[resource_edges::Model]) -> Subgraph {
    let e: Vec<Edge> = edges
        .iter()
        .filter_map(|e| {
            Some(Edge {
                source: e.source_object_id?.to_string(),
                target: e.target_object_id?.to_string(),
                link_type: e.relation.clone(),
            })
        })
        .collect();
    Subgraph::new(&e)
}

/// The object, its neighbours within `depth` (1..=2 for the UI) and the
/// links among them, as of now.
pub async fn neighborhood(
    db: &DatabaseConnection,
    org_id: i64,
    start: Uuid,
    depth: u32,
) -> Result<Neighborhood> {
    let edges = load(db, org_id, start, depth.clamp(1, 2)).await?;
    let links = links::views(db, org_id, &edges).await?;
    let mut ids: BTreeSet<Uuid> = edges
        .iter()
        .flat_map(|e| [e.source_object_id, e.target_object_id])
        .flatten()
        .collect();
    ids.insert(start);
    let by_id = links::refs(db, org_id, ids).await?;
    let mut nodes: Vec<_> = by_id.values().map(links::object_ref).collect();
    nodes.sort_by(|a, b| a.title.cmp(&b.title));
    Ok(Neighborhood { nodes, links })
}

/// Shortest path between two objects (links in either direction), within
/// `depth` hops of `from`: the objects along it.
pub async fn path(
    db: &DatabaseConnection,
    org_id: i64,
    from: Uuid,
    to: Uuid,
    depth: u32,
) -> Result<Vec<pnex_core::ontology::api::ObjectRef>> {
    super::objects::find(db, org_id, to).await?;
    let edges = load(db, org_id, from, depth).await?;
    let ids = subgraph(&edges)
        .shortest_path(&from.to_string(), &to.to_string())
        .ok_or(OntologyError::NotFound)?;
    let uuids: Vec<Uuid> = ids.iter().filter_map(|i| Uuid::parse_str(i).ok()).collect();
    let by_id = links::refs(db, org_id, uuids.clone()).await?;
    Ok(uuids
        .iter()
        .filter_map(|i| by_id.get(i))
        .map(links::object_ref)
        .collect())
}

/// Cascading impact: objects downstream (or upstream) of `start`.
pub async fn impact(
    db: &DatabaseConnection,
    org_id: i64,
    start: Uuid,
    upstream: bool,
    depth: u32,
) -> Result<Vec<pnex_core::ontology::api::ObjectRef>> {
    let edges = load(db, org_id, start, depth).await?;
    let ids: Vec<Uuid> = subgraph(&edges)
        .impact(&start.to_string(), upstream)
        .iter()
        .filter_map(|i| Uuid::parse_str(i).ok())
        .collect();
    let by_id = links::refs(db, org_id, ids).await?;
    let mut out: Vec<_> = by_id.values().map(links::object_ref).collect();
    out.sort_by(|a, b| a.title.cmp(&b.title));
    Ok(out)
}
