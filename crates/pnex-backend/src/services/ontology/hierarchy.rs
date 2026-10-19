//! Hierarchies (partner-watch.md D305): trees computed at read time on the
//! links a pack declares, never containment (D180 keeps one parent). A
//! node reached from several parents appears under each one, marked with
//! the others; links are read valid at an instant (D179).

use std::collections::{HashMap, HashSet};

use pnex_core::ontology::api::{Hierarchy, HierarchyNode, HierarchyView};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ColumnTrait, Condition, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::{links, objects, packs, Result};
use crate::models::_entities::resource_edges;

pub const DEPTH_MAX: u32 = 6;
pub const NODES_MAX: usize = 500;

/// The tree of `key` (the first installed hierarchy by default) from `root`.
pub async fn tree(
    db: &DatabaseConnection,
    org_id: i64,
    root: Uuid,
    key: Option<&str>,
    at: Option<DateTimeWithTimeZone>,
) -> Result<HierarchyView> {
    let root_row = objects::find(db, org_id, root).await?;
    let hierarchies = packs::hierarchies(db, org_id).await?;
    let Some(h) = (match key {
        Some(k) => hierarchies.iter().find(|h| h.key == k),
        None => hierarchies.first(),
    })
    .cloned() else {
        return Ok(HierarchyView {
            hierarchies,
            ..Default::default()
        });
    };
    let at = at.unwrap_or_else(|| chrono::Utc::now().into());
    let (children, truncated) = expand(db, org_id, root, &h, at).await?;
    let ids: HashSet<Uuid> = children
        .values()
        .flatten()
        .map(|(c, _)| *c)
        .chain([root])
        .collect();
    let rows = links::refs(db, org_id, ids).await?;
    let title = |id: &Uuid| rows.get(id).map(|o| o.title.clone()).unwrap_or_default();
    let mut parents: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (p, cs) in &children {
        for (c, _) in cs {
            parents.entry(*c).or_default().push(*p);
        }
    }
    let mut nodes = vec![HierarchyNode {
        id: root.to_string(),
        type_key: root_row.type_key.clone(),
        title: root_row.title.clone(),
        depth: 0,
        link: String::new(),
        also_under: Vec::new(),
    }];
    // Depth first; a node already on the path is not expanded again (cycle).
    let mut stack: Vec<(Uuid, Uuid, String, u32, Vec<Uuid>)> = children
        .get(&root)
        .into_iter()
        .flatten()
        .rev()
        .map(|(c, l)| (*c, root, l.clone(), 1, vec![root]))
        .collect();
    let mut cut = truncated;
    while let Some((id, parent, link, depth, path)) = stack.pop() {
        if nodes.len() >= NODES_MAX {
            cut = true;
            break;
        }
        let Some(row) = rows.get(&id) else {
            continue;
        };
        nodes.push(HierarchyNode {
            id: id.to_string(),
            type_key: row.type_key.clone(),
            title: row.title.clone(),
            depth,
            link,
            also_under: parents
                .get(&id)
                .into_iter()
                .flatten()
                .filter(|p| **p != parent)
                .map(title)
                .collect(),
        });
        if path.contains(&id) || depth >= DEPTH_MAX {
            continue;
        }
        let mut next = path.clone();
        next.push(id);
        for (c, l) in children.get(&id).into_iter().flatten().rev() {
            if !next.contains(c) {
                stack.push((*c, id, l.clone(), depth + 1, next.clone()));
            }
        }
    }
    Ok(HierarchyView {
        hierarchies,
        key: h.key,
        nodes,
        truncated: cut,
    })
}

type Children = HashMap<Uuid, Vec<(Uuid, String)>>;

/// Children of every node within `DEPTH_MAX` levels, one query per level,
/// in the order of the steps. `true` when the node cap stopped the walk.
async fn expand(
    db: &DatabaseConnection,
    org_id: i64,
    root: Uuid,
    h: &Hierarchy,
    at: DateTimeWithTimeZone,
) -> Result<(Children, bool)> {
    let relations: Vec<String> = h.steps.iter().map(|s| s.link.clone()).collect();
    let mut children: Children = HashMap::new();
    let mut seen: HashSet<Uuid> = HashSet::from([root]);
    let mut frontier = vec![root];
    for _ in 0..DEPTH_MAX {
        if frontier.is_empty() {
            break;
        }
        let edges = resource_edges::Entity::find()
            .filter(resource_edges::Column::OrgId.eq(org_id))
            .filter(resource_edges::Column::Relation.is_in(relations.clone()))
            .filter(links::valid_at(at))
            .filter(
                Condition::any()
                    .add(resource_edges::Column::SourceObjectId.is_in(frontier.clone()))
                    .add(resource_edges::Column::TargetObjectId.is_in(frontier.clone())),
            )
            .all(db)
            .await?;
        let mut next = Vec::new();
        for node in &frontier {
            let mut out = Vec::new();
            for step in &h.steps {
                for e in edges.iter().filter(|e| e.relation == step.link) {
                    let (from, to) = match step.inverse {
                        false => (e.source_object_id, e.target_object_id),
                        true => (e.target_object_id, e.source_object_id),
                    };
                    if let (Some(f), Some(t)) = (from, to) {
                        if f == *node && !out.iter().any(|(c, _)| *c == t) {
                            out.push((t, step.link.clone()));
                        }
                    }
                }
            }
            for (c, _) in &out {
                if seen.insert(*c) {
                    next.push(*c);
                }
            }
            if !out.is_empty() {
                children.insert(*node, out);
            }
            if seen.len() > NODES_MAX {
                return Ok((children, true));
            }
        }
        frontier = next;
    }
    Ok((children, false))
}
