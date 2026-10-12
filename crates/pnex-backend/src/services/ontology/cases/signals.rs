//! Raw signals of a case (cases-workspace.md K30): around the members, over
//! a window, the openings and closings of their links (D179), their object
//! changes (D184, O2), the ranges scoped on them (D182), the notifications
//! of member flows (D86) and the approximate dates of members (K5).
//! Computed on read, nothing is copied; retaining a signal makes an event.
//! The workspace's own operations (events, notes, attachments) are not
//! signals and are left out.
//! shortcut: camera events, transcripts and series are not merged yet; they
//! join as further sources.

use std::collections::{HashMap, HashSet};

use pnex_core::ontology::api::{CaseSignals, SignalItem, SignalsQuery};
use pnex_core::ontology::PropertyKind;
use pnex_core::ontology::{EVENT_OCCURRED, KIND_EVENT, KIND_NOTE, REL_INVOLVES, REL_IN_CASE};
use pnex_core::resources::KIND_FLOW;
use pnex_core::time_range::ScopeKind;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::sea_query::Expr;
use sea_orm::DatabaseConnection;
use sea_orm::{ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde_json::Value;
use uuid::Uuid;

use super::super::links::{self, views};
use super::super::{changes, invalid, parse_instant, schema, Result};
use super::membership;
use crate::models::_entities::{resource_edges, time_ranges};
use crate::services::notify_journal;
use crate::services::openobserve::Client;

/// Cap per source; beyond it the answer says `truncated`.
const CAP: u64 = 2000;
const DEFAULT_SPAN_DAYS: i64 = 30;

/// Span of the case events (`occurred` bounds), when any is dated, padded
/// by a tenth of its width and at least an hour on each side: what led to
/// the first event and followed the last one matters.
pub(super) fn events_span(
    events: &[crate::models::_entities::objects::Model],
) -> Option<(DateTimeWithTimeZone, DateTimeWithTimeZone)> {
    let bound = |o: &crate::models::_entities::objects::Model, k: &str| {
        o.properties
            .get(EVENT_OCCURRED)?
            .get(k)?
            .as_str()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
    };
    let from = events.iter().filter_map(|o| bound(o, "from")).min()?;
    let to = events.iter().filter_map(|o| bound(o, "to")).max()?;
    let pad = ((to - from) / 10).max(chrono::Duration::hours(1));
    Some((from - pad, to + pad))
}

pub async fn signals(
    db: &DatabaseConnection,
    o2: Option<&Client>,
    org_id: i64,
    case_id: Uuid,
    q: &SignalsQuery,
) -> Result<CaseSignals> {
    let m = membership(db, org_id, case_id).await?;
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let events: Vec<_> = m.events.iter().map(|(_, o)| o.clone()).collect();
    let span = events_span(&events);
    let to = parse_instant("to", q.to.as_deref())?
        .or(span.map(|(_, t)| t))
        .unwrap_or(now);
    let from = parse_instant("from", q.from.as_deref())?
        .or(span.map(|(f, _)| f))
        .unwrap_or(to - chrono::Duration::days(DEFAULT_SPAN_DAYS));
    if from >= to {
        return Err(invalid("from", pnex_core::err_codes::FIELD_INVALID));
    }
    let workspace: HashSet<Uuid> = events.iter().map(|o| o.id).collect();
    let mut ids = m.member_ids();
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
        .filter(resource_edges::Column::Relation.is_not_in([REL_IN_CASE, REL_INVOLVES]))
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
            items.push(SignalItem {
                at: e.valid_from.to_rfc3339(),
                kind: "link_open".into(),
                lane: lane.clone(),
                link: Some(v.clone()),
                ..Default::default()
            });
        }
        if let Some(t) = e.valid_to.filter(|t| *t >= from && *t <= to) {
            items.push(SignalItem {
                at: t.to_rfc3339(),
                kind: "link_close".into(),
                lane,
                link: Some(v),
                ..Default::default()
            });
        }
    }

    // Object changes (links are already above, from the database).
    let by_id = links::refs(db, org_id, ids.iter().copied()).await?;
    let signal_ids: Vec<String> = ids
        .iter()
        .filter(|id| **id != case_id && !workspace.contains(id))
        .filter(|id| by_id.get(id).is_none_or(|o| o.type_key != KIND_NOTE))
        .map(Uuid::to_string)
        .collect();
    let object_ids: Vec<String> = ids.iter().map(Uuid::to_string).collect();
    let changes_available = match changes::history_of(
        db,
        o2,
        org_id,
        &signal_ids,
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
                    .map(|(lane, c)| SignalItem {
                        at: c.at.clone(),
                        kind: "change".into(),
                        // The object's title, for a readable label.
                        title: Uuid::parse_str(&lane)
                            .ok()
                            .and_then(|id| by_id.get(&id))
                            .map(|o| o.title.clone()),
                        lane,
                        change: Some(c),
                        ..Default::default()
                    }),
            );
            true
        }
        Ok(None) => false,
        Err(e) => {
            tracing::warn!("case signals: change journal read failed: {e}");
            false
        }
    };
    // Ranges scoped on a member or on the case, overlapping the window.
    let ranges = time_ranges::Entity::find()
        .filter(time_ranges::Column::OrgId.eq(org_id))
        .filter(time_ranges::Column::ScopeKind.eq(ScopeKind::Object.wire()))
        .filter(time_ranges::Column::ScopeId.is_in(object_ids.clone()))
        .filter(Expr::cust_with_values(
            "coalesce(actual_start, planned_start) <= $1",
            [to],
        ))
        .filter(Expr::cust_with_values(
            "coalesce(actual_end, planned_end, actual_start, planned_start) >= $1",
            [from],
        ))
        .limit(CAP + 1)
        .all(db)
        .await?;
    truncated |= ranges.len() as u64 > CAP;
    for r in ranges.iter().take(CAP as usize) {
        let Some(start) = r.actual_start.or(r.planned_start) else {
            continue;
        };
        items.push(SignalItem {
            at: start.to_rfc3339(),
            kind: "range".into(),
            lane: r.scope_id.clone(),
            range: Some(crate::services::time_ranges::view(r)),
            ..Default::default()
        });
    }

    // Approximate dates of the members (`fuzzy_time` properties, K5),
    // placed at their lower bound when they overlap the window.
    let s = schema(db, org_id).await?;
    for o in by_id.values().filter(|o| o.type_key != KIND_EVENT) {
        let Some(def) = s.object_type(&o.type_key) else {
            continue;
        };
        for p in def
            .properties
            .iter()
            .filter(|p| matches!(p.kind, PropertyKind::FuzzyTime))
        {
            let Some(v) = o.properties.get(&p.key) else {
                continue;
            };
            let bound = |k: &str| {
                v.get(k)
                    .and_then(Value::as_str)
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            };
            let (Some(a), Some(b)) = (bound("from"), bound("to")) else {
                continue;
            };
            if a > to || b < from {
                continue;
            }
            items.push(SignalItem {
                at: a.to_rfc3339(),
                kind: "occurred".into(),
                lane: o.id.to_string(),
                fuzzy: Some(v.clone()),
                title: Some(o.title.clone()),
                ..Default::default()
            });
        }
    }

    // Notifications raised by member flows (D86 journal).
    let flows: HashMap<i64, String> = by_id
        .values()
        .filter(|o| o.type_key == KIND_FLOW)
        .filter_map(|o| Some((o.native_id.parse().ok()?, o.id.to_string())))
        .collect();
    let flow_ids: Vec<i64> = flows.keys().copied().collect();
    match notify_journal::for_flows(
        db,
        o2,
        org_id,
        &flow_ids,
        from.timestamp_micros(),
        to.timestamp_micros(),
        CAP as i64,
    )
    .await
    {
        Ok(Some(rows)) => {
            truncated |= rows.len() as u64 >= CAP;
            items.extend(rows.into_iter().filter_map(|n| {
                let lane = flows.get(&n.flow_id?)?.clone();
                Some(SignalItem {
                    at: n.created_at.clone(),
                    kind: "notification".into(),
                    lane,
                    notification: Some(n),
                    ..Default::default()
                })
            }));
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("case signals: notification journal read failed: {e}"),
    }

    items.sort_by_key(|i| chrono::DateTime::parse_from_rfc3339(&i.at).ok());
    Ok(CaseSignals {
        from: from.to_rfc3339(),
        to: to.to_rfc3339(),
        items,
        changes_available,
        truncated,
    })
}
