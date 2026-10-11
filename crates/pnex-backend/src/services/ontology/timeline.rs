//! Case timeline (cases.md K6, lot C1): the members of a case and, over a
//! window, the openings and closings of their links (D179) merged with
//! their object changes (D184, O2). Computed on read, writes nothing.
//!
//! shortcut: links and changes only; alerts (D86), camera events,
//! transcripts, ranges and series join as further sources (K6 table).

use std::collections::HashSet;

use loco_rs::prelude::AppContext;
use pnex_core::ontology::api::{CaseTimeline, TimelineItem, TimelineQuery};
use pnex_core::ontology::{CASE_WINDOW_FROM, CASE_WINDOW_TO, KIND_CASE, REL_IN_CASE};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde_json::Value;
use uuid::Uuid;

use super::links::views;
use super::objects::find;
use super::{changes, parse_instant, OntologyError, Result};
use crate::models::_entities::resource_edges;

/// Cap per source; beyond it the answer says `truncated`.
const CAP: u64 = 2000;
const DEFAULT_SPAN_DAYS: i64 = 30;

pub async fn for_case(
    ctx: &AppContext,
    org_id: i64,
    case_id: Uuid,
    q: &TimelineQuery,
) -> Result<CaseTimeline> {
    let db = &ctx.db;
    let case = find(db, org_id, case_id).await?;
    if case.type_key != KIND_CASE {
        return Err(OntologyError::NotFound);
    }
    let prop = |k: &str| case.properties.get(k).and_then(Value::as_str);
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let to = parse_instant("to", q.to.as_deref().or(prop(CASE_WINDOW_TO)))?.unwrap_or(now);
    let from = parse_instant("from", q.from.as_deref().or(prop(CASE_WINDOW_FROM)))?
        .unwrap_or(to - chrono::Duration::days(DEFAULT_SPAN_DAYS));
    if from >= to {
        return Err(super::invalid("from", pnex_core::err_codes::FIELD_INVALID));
    }

    // Members: open `in_case` links. Former members removed inside the
    // window keep their lane: their removal is part of the story.
    let member_rows = resource_edges::Entity::find()
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .filter(resource_edges::Column::Relation.eq(REL_IN_CASE))
        .filter(resource_edges::Column::TargetObjectId.eq(case_id))
        .filter(
            Condition::any()
                .add(resource_edges::Column::ValidTo.is_null())
                .add(resource_edges::Column::ValidTo.gte(from)),
        )
        .order_by_asc(resource_edges::Column::ValidFrom)
        .all(db)
        .await?;
    let open: Vec<_> = member_rows
        .iter()
        .filter(|e| e.valid_to.is_none())
        .cloned()
        .collect();
    let members = views(db, org_id, &open).await?;
    let mut ids: Vec<Uuid> = member_rows
        .iter()
        .filter_map(|e| e.source_object_id)
        .collect();
    ids.sort();
    ids.dedup();
    ids.push(case_id);
    let id_set: HashSet<Uuid> = ids.iter().copied().collect();

    // Links of the members opened or closed inside the window.
    let ends = Condition::any()
        .add(resource_edges::Column::SourceObjectId.is_in(ids.clone()))
        .add(resource_edges::Column::TargetObjectId.is_in(ids.clone()));
    let moved = Condition::any()
        .add(
            Condition::all()
                .add(resource_edges::Column::ValidFrom.gte(from))
                .add(resource_edges::Column::ValidFrom.lte(to)),
        )
        .add(
            Condition::all()
                .add(resource_edges::Column::ValidTo.gte(from))
                .add(resource_edges::Column::ValidTo.lte(to)),
        );
    let edges = resource_edges::Entity::find()
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .filter(ends)
        .filter(moved)
        .order_by_asc(resource_edges::Column::ValidFrom)
        .limit(CAP + 1)
        .all(db)
        .await?;
    let mut truncated = edges.len() as u64 > CAP;
    let edges = &edges[..edges.len().min(CAP as usize)];
    let link_views = views(db, org_id, edges).await?;
    let mut items = Vec::new();
    for (e, v) in edges.iter().zip(link_views) {
        // Lane: the member end (the source when both ends are members).
        let lane = [e.source_object_id, e.target_object_id]
            .into_iter()
            .flatten()
            .find(|id| id_set.contains(id) && *id != case_id)
            .unwrap_or(case_id)
            .to_string();
        if e.valid_from >= from && e.valid_from <= to {
            items.push(TimelineItem {
                at: e.valid_from.to_rfc3339(),
                kind: "link_open".into(),
                lane: lane.clone(),
                link: Some(v.clone()),
                change: None,
            });
        }
        if let Some(t) = e.valid_to.filter(|t| *t >= from && *t <= to) {
            items.push(TimelineItem {
                at: t.to_rfc3339(),
                kind: "link_close".into(),
                lane,
                link: Some(v),
                change: None,
            });
        }
    }

    // Object changes (links are already above, from the database).
    let object_ids: Vec<String> = ids.iter().map(Uuid::to_string).collect();
    let changes_available = match changes::history_of(
        ctx,
        org_id,
        &object_ids,
        from.timestamp_micros(),
        to.timestamp_micros(),
        CAP as i64,
    )
    .await
    {
        Ok(Some(rows)) => {
            truncated |= rows.len() as u64 >= CAP;
            items.extend(
                rows.into_iter()
                    .filter(|(_, c)| c.action.starts_with("object."))
                    .map(|(lane, c)| TimelineItem {
                        at: c.at.clone(),
                        kind: "change".into(),
                        lane,
                        link: None,
                        change: Some(c),
                    }),
            );
            true
        }
        Ok(None) => false,
        Err(e) => {
            tracing::warn!("case timeline: change journal read failed: {e}");
            false
        }
    };
    items.sort_by_key(|i| chrono::DateTime::parse_from_rfc3339(&i.at).ok());
    Ok(CaseTimeline {
        from: from.to_rfc3339(),
        to: to.to_rfc3339(),
        members,
        items,
        changes_available,
        truncated,
    })
}
