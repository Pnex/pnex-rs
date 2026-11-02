//! Hierarchies (partner-watch.md D305): trees computed at read time on the
//! links a pack declares, never containment (D180 keeps one parent). A
//! node reached from several parents appears under each one, marked with
//! the others; links are read valid at an instant (D179).

use std::collections::{HashMap, HashSet};

use pnex_core::ontology::api::{
    Hierarchy, HierarchyNode, HierarchyView, MarkCount, NodeMark, PropertyTotal, TallyCount,
};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ColumnTrait, Condition, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::{links, objects, packs, Result};
use crate::models::_entities::resource_edges;

pub const DEPTH_MAX: u32 = 6;
pub const NODES_MAX: usize = 500;

/// The tree of `key` (the first installed hierarchy by default) from `root`.
/// `hide` masks the titles of the other parents (`also_under`) the caller
/// may not read (D443); the nodes are masked by the caller.
pub async fn tree(
    db: &DatabaseConnection,
    org_id: i64,
    root: Uuid,
    key: Option<&str>,
    at: Option<DateTimeWithTimeZone>,
    hide: super::redact::Redaction,
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
    let rows = links::refs(db, org_id, ids.clone()).await?;
    let s = super::schema(db, org_id).await?;
    let hidden = hide.title_hidden_types(&s);
    let title = |id: &Uuid| match rows.get(id) {
        Some(o) if hidden.contains(&o.type_key) => super::redact::masked(&id.to_string()),
        Some(o) => o.title.clone(),
        None => String::new(),
    };
    let marks = |id: &Uuid| -> Vec<NodeMark> {
        let Some(row) = rows.get(id) else {
            return Vec::new();
        };
        let Some(def) = s.object_type(&row.type_key) else {
            return Vec::new();
        };
        def.properties
            .iter()
            .filter_map(|p| {
                let v = row.properties.get(&p.key)?;
                let value = v.as_str()?.to_string();
                Some(NodeMark {
                    property: p.key.clone(),
                    color: p.color_of(v)?.to_string(),
                    labels: p.labels.get(&value).cloned().unwrap_or_default(),
                    value,
                })
            })
            .collect()
    };
    let covered = covered(db, org_id, &h, &ids, at).await?;
    let white = |id: &Uuid| {
        h.coverage.as_ref().is_some_and(|c| {
            *id != root
                && !covered.contains(id)
                && !rows.get(id).is_some_and(|r| {
                    c.values.iter().any(|(k, vs)| {
                        r.properties
                            .get(k)
                            .and_then(|v| v.as_str())
                            .is_some_and(|v| vs.iter().any(|x| x == v))
                    })
                })
        })
    };
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
        marks: marks(&root),
        white_space: false,
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
            marks: marks(&id),
            white_space: white(&id),
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
    // Coverage of the tree (D330): each node once, however many parents.
    let mut once: HashSet<&str> = HashSet::new();
    let distinct: Vec<&HierarchyNode> = nodes.iter().filter(|n| once.insert(&n.id)).collect();
    let mut counts: Vec<MarkCount> = Vec::new();
    for m in distinct.iter().flat_map(|n| &n.marks) {
        match counts
            .iter_mut()
            .find(|c| c.property == m.property && c.value == m.value)
        {
            Some(c) => c.count += 1,
            None => counts.push(MarkCount {
                property: m.property.clone(),
                value: m.value.clone(),
                color: m.color.clone(),
                labels: m.labels.clone(),
                count: 1,
            }),
        }
    }
    let totals = h
        .sums
        .iter()
        .map(|k| PropertyTotal {
            property: k.clone(),
            total: distinct
                .iter()
                .filter_map(|n| Uuid::parse_str(&n.id).ok())
                .filter_map(|id| rows.get(&id)?.properties.get(k)?.as_f64())
                .sum(),
        })
        .collect();
    let white_space = distinct.iter().filter(|n| n.white_space).count() as u32;
    let tallies = tallies(db, org_id, &h, &ids, at, &s, hide).await?;
    Ok(HierarchyView {
        hierarchies,
        has_coverage: h.coverage.is_some(),
        key: h.key,
        nodes,
        truncated: cut,
        counts,
        totals,
        tallies,
        white_space,
    })
}

/// Nodes with an open link of a covering type (D330), at `at`.
async fn covered(
    db: &DatabaseConnection,
    org_id: i64,
    h: &Hierarchy,
    ids: &HashSet<Uuid>,
    at: DateTimeWithTimeZone,
) -> Result<HashSet<Uuid>> {
    let Some(c) = h.coverage.as_ref().filter(|c| !c.links.is_empty()) else {
        return Ok(HashSet::new());
    };
    let ids: Vec<Uuid> = ids.iter().copied().collect();
    let edges = resource_edges::Entity::find()
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .filter(resource_edges::Column::Relation.is_in(c.links.clone()))
        .filter(links::valid_at(at))
        .filter(
            Condition::any()
                .add(resource_edges::Column::SourceObjectId.is_in(ids.clone()))
                .add(resource_edges::Column::TargetObjectId.is_in(ids)),
        )
        .all(db)
        .await?;
    Ok(edges
        .iter()
        .flat_map(|e| [e.source_object_id, e.target_object_id])
        .flatten()
        .collect())
}

/// What the nodes are linked to, counted (D330): each object at the other
/// end once, however many nodes it touches. A breakdown or filter on a
/// property the caller may not read would reveal it (D443): the breakdown
/// collapses into the whole count, the filtered count is left out.
async fn tallies(
    db: &DatabaseConnection,
    org_id: i64,
    h: &Hierarchy,
    ids: &HashSet<Uuid>,
    at: DateTimeWithTimeZone,
    s: &super::OrgSchema,
    hide: super::redact::Redaction,
) -> Result<Vec<TallyCount>> {
    let mut out = Vec::new();
    let nodes: Vec<Uuid> = ids.iter().copied().collect();
    for t in &h.tallies {
        let Some(lt) = s.link_type(&t.link) else {
            continue;
        };
        let edges = resource_edges::Entity::find()
            .filter(resource_edges::Column::OrgId.eq(org_id))
            .filter(resource_edges::Column::Relation.eq(t.link.clone()))
            .filter(links::valid_at(at))
            .filter(
                Condition::any()
                    .add(resource_edges::Column::SourceObjectId.is_in(nodes.clone()))
                    .add(resource_edges::Column::TargetObjectId.is_in(nodes.clone())),
            )
            .all(db)
            .await?;
        let (mut others, mut from_source) = (HashSet::new(), false);
        for e in &edges {
            match (e.source_object_id, e.target_object_id) {
                (Some(a), Some(b)) if ids.contains(&a) && !ids.contains(&b) => {
                    from_source = true;
                    others.insert(b);
                }
                (Some(a), Some(b)) if ids.contains(&b) && !ids.contains(&a) => {
                    others.insert(a);
                }
                _ => {}
            }
        }
        let rows = links::refs(db, org_id, others).await?;
        let def_of = |r: &crate::models::_entities::objects::Model, key: &str| {
            s.object_type(&r.type_key)
                .and_then(|d| d.properties.iter().find(|p| p.key == key).cloned())
        };
        let hidden = |r: &crate::models::_entities::objects::Model, key: &str| {
            def_of(r, key).is_some_and(|p| hide.hides(p.classification))
        };
        if rows.values().any(|r| t.only.keys().any(|k| hidden(r, k))) {
            continue;
        }
        let kept: Vec<_> = rows
            .values()
            .filter(|r| {
                t.only.iter().all(|(k, vs)| {
                    r.properties
                        .get(k)
                        .and_then(|v| v.as_str())
                        .is_some_and(|v| vs.iter().any(|x| x == v))
                })
            })
            .collect();
        let (name, names) = if from_source {
            (lt.name.clone(), lt.names.clone())
        } else {
            (lt.inverse_name.clone(), lt.inverse_names.clone())
        };
        let by =
            t.by.as_deref()
                .filter(|k| !kept.iter().any(|r| hidden(r, k)));
        let mut counts: Vec<TallyCount> = Vec::new();
        for r in &kept {
            let value = by
                .and_then(|k| r.properties.get(k))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            match counts.iter_mut().find(|c| c.value == value) {
                Some(c) => c.count += 1,
                None => counts.push(TallyCount {
                    link: t.link.clone(),
                    name: name.clone(),
                    names: names.clone(),
                    labels: value
                        .as_ref()
                        .zip(by)
                        .and_then(|(v, k)| def_of(r, k)?.labels.get(v).cloned())
                        .unwrap_or_default(),
                    value,
                    count: 1,
                }),
            }
        }
        // Stable order: a band that reshuffles at each read is unreadable.
        counts.sort_by(|a, b| a.value.cmp(&b.value));
        out.append(&mut counts);
    }
    Ok(out)
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
