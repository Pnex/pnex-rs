//! Graph store of the ontology (annex A3, D589): open links are read level by
//! level (visited set, row cap, statement timeout), then shortest paths are
//! computed by the pure code of `pnex_core::ontology::graph`. A cut is always
//! reported (`truncated`), never silent. The public API stays bounded (D185).

use std::collections::{BTreeSet, HashSet};

use pnex_core::ontology::api::{Neighborhood, ObjectImpact, ObjectPath};
use pnex_core::ontology::graph::{Edge, Subgraph};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Statement, TransactionTrait,
};
use uuid::Uuid;

use super::{links, OntologyError, Result};
use crate::models::_entities::resource_edges;

/// Links read for a neighbourhood or an impact: what a screen can use.
const EDGES_MAX: usize = 5_000;
/// Links read for a path: a search from both ends stays cheap at this size
/// (D589: p95 under 300 ms on 30M links), and only needs ids in memory.
const PATH_EDGES_MAX: usize = 200_000;
/// Wall time of one level.
const TIMEOUT_MS: u32 = 3_000;
/// Starts considered by [`path_from_any`] at most.
const STARTS_MAX: usize = 500;

/// Which ends of a link a level follows.
#[derive(Clone, Copy)]
enum Way {
    Both,
    Downstream,
    Upstream,
}

#[derive(Default)]
struct Loaded {
    edges: Vec<(i64, Uuid, Uuid)>,
    ids: HashSet<i64>,
    truncated: bool,
}

impl Loaded {
    /// Reads one level from `frontier` and returns the objects it reaches.
    /// A level past its row cap or its time cap ends the walk as truncated.
    async fn expand(
        &mut self,
        db: &DatabaseConnection,
        org_id: i64,
        frontier: &[Uuid],
        way: Way,
        relations: &[String],
        cap: usize,
    ) -> Result<Vec<Uuid>> {
        let left = cap.saturating_sub(self.edges.len());
        let Some(mut rows) = level(db, org_id, frontier, way, relations, left + 1).await? else {
            self.truncated = true;
            return Ok(Vec::new());
        };
        // Flagged on the row count: a link seen from both ends takes two rows.
        if rows.len() > left {
            self.truncated = true;
            rows.truncate(left);
        }
        let mut reached = Vec::with_capacity(rows.len());
        for (id, source, target) in rows {
            if self.ids.insert(id) {
                self.edges.push((id, source, target));
            }
            match way {
                Way::Both => reached.extend([source, target]),
                Way::Downstream => reached.push(target),
                Way::Upstream => reached.push(source),
            }
        }
        Ok(reached)
    }
}

/// Open links at the given ends of `frontier`; `None` past the time cap.
async fn level(
    db: &DatabaseConnection,
    org_id: i64,
    frontier: &[Uuid],
    way: Way,
    relations: &[String],
    limit: usize,
) -> Result<Option<Vec<(i64, Uuid, Uuid)>>> {
    let branch = |column: &str, other: &str| {
        format!(
            "SELECT id, source_object_id, target_object_id FROM resource_edges \
             WHERE {column} = ANY($2::uuid[]) AND {other} IS NOT NULL AND org_id = $1 \
               AND valid_to IS NULL AND (cardinality($3::text[]) = 0 OR relation = ANY($3))"
        )
    };
    let (down, up) = (
        branch("source_object_id", "target_object_id"),
        branch("target_object_id", "source_object_id"),
    );
    let sql = match way {
        Way::Both => format!("{down} UNION ALL {up} LIMIT $4"),
        Way::Downstream => format!("{down} LIMIT $4"),
        Way::Upstream => format!("{up} LIMIT $4"),
    };
    let txn = db.begin().await?;
    // Scoped to this transaction: a runaway level cannot hold the pool. JIT
    // costs more than these index lookups (D589: 17 ms against 2.5 ms).
    txn.execute_unprepared(&format!(
        "SET LOCAL statement_timeout = {TIMEOUT_MS}; SET LOCAL jit = off"
    ))
    .await?;
    let found = txn
        .query_all_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            sql,
            [
                org_id.into(),
                frontier.to_vec().into(),
                relations.to_vec().into(),
                (limit as i64).into(),
            ],
        ))
        .await;
    let rows = match found {
        Ok(rows) => rows,
        Err(e) if e.to_string().contains("statement timeout") => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    txn.commit().await?;
    Ok(Some(
        rows.iter()
            .filter_map(|r| {
                Some((
                    r.try_get::<i64>("", "id").ok()?,
                    r.try_get::<Uuid>("", "source_object_id").ok()?,
                    r.try_get::<Uuid>("", "target_object_id").ok()?,
                ))
            })
            .collect(),
    ))
}

/// The links around `start`, one level per hop, visiting each object once.
async fn walk(
    db: &DatabaseConnection,
    org_id: i64,
    start: Uuid,
    depth: u32,
    way: Way,
) -> Result<Loaded> {
    let mut loaded = Loaded::default();
    let mut seen = HashSet::from([start]);
    let mut frontier = vec![start];
    for _ in 0..depth {
        if frontier.is_empty() || loaded.truncated {
            break;
        }
        let reached = loaded
            .expand(db, org_id, &frontier, way, &[], EDGES_MAX)
            .await?;
        frontier = reached.into_iter().filter(|n| seen.insert(*n)).collect();
    }
    Ok(loaded)
}

/// The object, its neighbours within `depth` (1..=2 for the UI) and the
/// links among them, as of now.
pub async fn neighborhood(
    db: &DatabaseConnection,
    org_id: i64,
    start: Uuid,
    depth: u32,
) -> Result<Neighborhood> {
    use sea_orm::ColumnTrait;
    super::objects::find(db, org_id, start).await?;
    let loaded = walk(db, org_id, start, depth.clamp(1, 2), Way::Both).await?;
    let edges = if loaded.edges.is_empty() {
        Vec::new()
    } else {
        let ids = loaded.edges.iter().map(|e| e.0);
        resource_edges::Entity::find()
            .filter(resource_edges::Column::Id.is_in(ids))
            .all(db)
            .await?
    };
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
    let s = super::schema(db, org_id).await?;
    let colors = by_id
        .iter()
        .filter_map(|(id, o)| Some((id.to_string(), super::objects::color_of(&s, o)?)))
        .collect();
    Ok(Neighborhood {
        nodes,
        links,
        colors,
        truncated: loaded.truncated,
    })
}

/// Shortest path from the nearest of `starts` to `to`, searched from both
/// ends, each level expanding the smaller side: two half-depth balls instead
/// of one full-depth ball. Every link of both balls is loaded, so the first
/// meeting gives the shortest path unless the search was cut.
async fn between(
    db: &DatabaseConnection,
    org_id: i64,
    starts: Vec<Uuid>,
    to: Uuid,
    depth: u32,
    relations: &[String],
    starts_cut: bool,
) -> Result<ObjectPath> {
    let mut loaded = Loaded::default();
    let mut seen_from: HashSet<Uuid> = starts.iter().copied().collect();
    let mut seen_to = HashSet::from([to]);
    let (mut from_side, mut to_side) = (starts.clone(), vec![to]);
    let mut hops = 0;
    while hops < depth && !from_side.is_empty() && !to_side.is_empty() {
        let forward = from_side.len() <= to_side.len();
        let side = if forward { &from_side } else { &to_side };
        let reached = loaded
            .expand(db, org_id, side, Way::Both, relations, PATH_EDGES_MAX)
            .await?;
        let (mine, other) = if forward {
            (&mut seen_from, &seen_to)
        } else {
            (&mut seen_to, &seen_from)
        };
        let next: Vec<Uuid> = reached.into_iter().filter(|n| mine.insert(*n)).collect();
        let met = next.iter().any(|n| other.contains(n));
        if forward {
            from_side = next;
        } else {
            to_side = next;
        }
        hops += 1;
        if met || loaded.truncated {
            break;
        }
    }
    let edges: Vec<Edge> = loaded
        .edges
        .iter()
        .map(|(_, s, t)| Edge {
            source: s.to_string(),
            target: t.to_string(),
            link_type: String::new(),
        })
        .collect();
    let starts: Vec<String> = starts.iter().map(Uuid::to_string).collect();
    let found = Subgraph::new(&edges).nearest_path(&starts, &to.to_string());
    // Cut starts flag the answer but must not stop the walk above.
    let truncated = loaded.truncated || starts_cut;
    let Some(ids) = found else {
        return Err(if truncated {
            OntologyError::PathIncomplete
        } else {
            OntologyError::NotFound
        });
    };
    let uuids: Vec<Uuid> = ids.iter().filter_map(|i| Uuid::parse_str(i).ok()).collect();
    let by_id = links::refs(db, org_id, uuids.clone()).await?;
    Ok(ObjectPath {
        path: uuids
            .iter()
            .filter_map(|i| by_id.get(i))
            .map(links::object_ref)
            .collect(),
        truncated,
    })
}

/// Shortest path between two objects (links in either direction), within
/// `depth` hops, along `relations` only when given: the objects along it.
pub async fn path(
    db: &DatabaseConnection,
    org_id: i64,
    from: Uuid,
    to: Uuid,
    depth: u32,
    relations: &[String],
) -> Result<ObjectPath> {
    super::objects::find(db, org_id, to).await?;
    let origin = super::objects::find(db, org_id, from).await?;
    if from == to {
        return Ok(ObjectPath {
            path: vec![links::object_ref(&origin)],
            truncated: false,
        });
    }
    between(db, org_id, vec![from], to, depth, relations, false).await
}

/// Shortest path to `to` from the nearest object at the source of an open
/// link of `start_links` (the team following accounts, D338), within
/// `depth` hops, along `relations` when given.
pub async fn path_from_any(
    db: &DatabaseConnection,
    org_id: i64,
    start_links: &[String],
    to: Uuid,
    depth: u32,
    relations: &[String],
) -> Result<ObjectPath> {
    let mut starts: Vec<Uuid> = db
        .query_all_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT DISTINCT source_object_id FROM resource_edges \
             WHERE org_id = $1 AND relation = ANY($2) AND valid_to IS NULL \
               AND source_object_id IS NOT NULL LIMIT $3",
            [
                org_id.into(),
                start_links.to_vec().into(),
                (STARTS_MAX as i64 + 1).into(),
            ],
        ))
        .await?
        .iter()
        .filter_map(|r| r.try_get::<Uuid>("", "source_object_id").ok())
        .collect();
    let cut = starts.len() > STARTS_MAX;
    starts.truncate(STARTS_MAX);
    starts.retain(|s| *s != to);
    super::objects::find(db, org_id, to).await?;
    if starts.is_empty() {
        return Err(OntologyError::NotFound);
    }
    between(db, org_id, starts, to, depth, relations, cut).await
}

/// Cascading impact: objects downstream (or upstream) of `start`.
pub async fn impact(
    db: &DatabaseConnection,
    org_id: i64,
    start: Uuid,
    upstream: bool,
    depth: u32,
) -> Result<ObjectImpact> {
    super::objects::find(db, org_id, start).await?;
    let way = if upstream {
        Way::Upstream
    } else {
        Way::Downstream
    };
    let loaded = walk(db, org_id, start, depth, way).await?;
    let ids: BTreeSet<Uuid> = loaded
        .edges
        .iter()
        .map(|(_, s, t)| if upstream { *s } else { *t })
        .filter(|n| *n != start)
        .collect();
    let by_id = links::refs(db, org_id, ids).await?;
    let mut objects: Vec<_> = by_id.values().map(links::object_ref).collect();
    objects.sort_by(|a, b| a.title.cmp(&b.title));
    Ok(ObjectImpact {
        objects,
        truncated: loaded.truncated,
    })
}
