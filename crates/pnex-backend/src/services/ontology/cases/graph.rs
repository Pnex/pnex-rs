//! The case graph (cases-workspace.md K31): the ontology relations among the
//! members, with their history (the client fades them by the cursor without
//! asking again), and the two-step expansion of a node: its neighbours
//! counted by link type and object type over the case window (open links
//! included), then the ones picked. Case structure links (`in_case`, `involves`) are drawn from the
//! workspace, never here.

use std::collections::{BTreeMap, HashSet};

use pnex_core::ontology::api::{ExpandFacet, ExpandInput, ExpandResult, LinkView};
use pnex_core::ontology::{REL_INVOLVES, REL_IN_CASE};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use uuid::Uuid;

use super::super::{links, OntologyError, Result};
use super::membership;
use super::signals::events_span;
use crate::models::_entities::resource_edges;

/// Relations returned at most among the members.
const RELATIONS_CAP: u64 = 2_000;
/// Neighbours added at most by one expansion.
const EXPAND_CAP: usize = 100;

fn structural(relation: &str) -> bool {
    relation == REL_IN_CASE || relation == REL_INVOLVES
}

/// Every relation ever opened between two members, closed ones included.
pub async fn relations(
    db: &DatabaseConnection,
    org_id: i64,
    case_id: Uuid,
) -> Result<Vec<LinkView>> {
    let ids = membership(db, org_id, case_id).await?.member_ids();
    let rows = resource_edges::Entity::find()
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .filter(resource_edges::Column::SourceObjectId.is_in(ids.clone()))
        .filter(resource_edges::Column::TargetObjectId.is_in(ids))
        .filter(resource_edges::Column::Relation.is_not_in([REL_IN_CASE, REL_INVOLVES]))
        // Stable under the cap: pages of the same case never overlap or skip.
        .order_by_asc(resource_edges::Column::Id)
        .limit(RELATIONS_CAP)
        .all(db)
        .await?;
    Ok(links::views(db, org_id, &rows).await?)
}

/// Neighbours of a node over the case window (now when nothing is dated).
pub async fn expand(
    db: &DatabaseConnection,
    org_id: i64,
    case_id: Uuid,
    input: &ExpandInput,
) -> Result<ExpandResult> {
    let m = membership(db, org_id, case_id).await?;
    let events: Vec<_> = m.events.iter().map(|(_, o)| o.clone()).collect();
    let now = chrono::Utc::now();
    let (from, to) = events_span(&events)
        .map(|(f, t)| (f.with_timezone(&chrono::Utc), t.with_timezone(&chrono::Utc)))
        .unwrap_or((now, now));
    let id = Uuid::parse_str(&input.object_id).map_err(|_| OntologyError::NotFound)?;
    let shown: HashSet<&str> = input.exclude.iter().map(String::as_str).collect();
    let mut found: Vec<(LinkView, pnex_core::ontology::api::ObjectRef)> = Vec::new();
    // Another org's object: not found (R1).
    for l in links::for_object(db, org_id, id, None, true).await? {
        if structural(&l.link_type) {
            continue;
        }
        // Valid at some point of the window, or still open: a relation is
        // often recorded after the facts it describes.
        let opened = chrono::DateTime::parse_from_rfc3339(&l.valid_from).ok();
        let closed = l
            .valid_to
            .as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
        let in_window = !opened.is_some_and(|o| o > to) && !closed.is_some_and(|c| c <= from);
        if !in_window && closed.is_some() {
            continue;
        }
        let other = if l.source.id == input.object_id {
            l.target.clone()
        } else {
            l.source.clone()
        };
        if other.id.is_empty() || shown.contains(other.id.as_str()) {
            continue;
        }
        found.push((l, other));
    }
    // Live objects only (an archived neighbour is history, not a lead).
    let ids: Vec<Uuid> = found
        .iter()
        .filter_map(|(_, o)| Uuid::parse_str(&o.id).ok())
        .collect();
    let live: HashSet<String> = links::refs(db, org_id, ids)
        .await?
        .into_values()
        .filter(|o| o.valid_to.is_none())
        .map(|o| o.id.to_string())
        .collect();
    found.retain(|(_, o)| live.contains(&o.id));

    let Some(pick) = &input.pick else {
        let mut counts: BTreeMap<(String, String), HashSet<String>> = BTreeMap::new();
        for (l, o) in &found {
            counts
                .entry((l.link_type.clone(), o.type_key.clone()))
                .or_default()
                .insert(o.id.clone());
        }
        return Ok(ExpandResult {
            facets: counts
                .into_iter()
                .map(|((link_type, type_key), ids)| ExpandFacet {
                    link_type,
                    type_key,
                    count: ids.len(),
                })
                .collect(),
            ..Default::default()
        });
    };
    let wanted = |l: &LinkView, o: &pnex_core::ontology::api::ObjectRef| {
        pick.iter()
            .any(|f| f.link_type == l.link_type && f.type_key == o.type_key)
    };
    let mut out = ExpandResult::default();
    for (l, o) in found.into_iter().filter(|(l, o)| wanted(l, o)) {
        if !out.nodes.iter().any(|n| n.id == o.id) {
            if out.nodes.len() == EXPAND_CAP {
                out.truncated = true;
                continue;
            }
            out.nodes.push(o);
        }
        out.links.push(l);
    }
    Ok(out)
}
