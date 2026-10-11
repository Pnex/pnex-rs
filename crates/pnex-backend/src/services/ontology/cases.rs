//! Opening a case (cases.md K1, K3): the `case` object, then, when opened
//! from a dated event, its window around the event and its first members
//! (the flow that raised it as subject, the objects linked to that flow at
//! the event's instant as context). Every write goes through the generic
//! object and link services: same validations, same role lock (D188).
//! Then the reads that conclude a case: the hypotheses board (K8) and the
//! export as of an instant (K12).

use std::collections::{HashMap, HashSet};

use pnex_core::err_codes::{FIELD_INVALID, FIELD_REQUIRED};
use pnex_core::ontology::api::{
    BoardClaim, BoardHypothesis, CaseBoard, CaseExport, CaseExportQuery, CaseOpenInput, CasePlace,
    CaseSnapshot, ChangeView, ExportMember, LinkInput, LinkView, Neighborhood, ObjectInput,
    ObjectRef, ObjectView, PieceEntry, TimelineQuery,
};
use pnex_core::ontology::{
    PropertyKind, CASE_FROZEN_AT, CASE_STATUS, CASE_WINDOW_FROM, CASE_WINDOW_TO, IN_CASE_ROLE,
    KIND_CASE, REL_IN_CASE, REL_PLACED_AT,
};
use pnex_core::resources::{KIND_FLOW, KIND_MAP_PIN};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::{
    changes, invalid, links, objects, parse_instant, schema, timeline, ts, Actor, OntologyError,
    OrgSchema, Result,
};
use crate::models::_entities::map_pins;
use crate::services::openobserve::Client;
use crate::services::pois::d2f;

const DEFAULT_MARGIN_MIN: i64 = 120;
const MAX_MARGIN_MIN: i64 = 7 * 24 * 60;
const SOURCE_KINDS: [&str; 2] = ["notification", "event"];

pub(super) fn member(case_id: &str, object_id: &str, role: &str) -> LinkInput {
    let mut attributes = Map::new();
    attributes.insert(IN_CASE_ROLE.into(), Value::String(role.into()));
    LinkInput {
        link_type: REL_IN_CASE.into(),
        source_id: object_id.into(),
        target_id: case_id.into(),
        attributes,
        valid_from: None,
    }
}

pub async fn open(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    input: &CaseOpenInput,
) -> Result<ObjectView> {
    let mut props = Map::new();
    props.insert(CASE_STATUS.into(), Value::String("open".into()));
    let mut at = None;
    if let Some(from) = &input.from {
        if !SOURCE_KINDS.contains(&from.kind.as_str()) {
            return Err(invalid("from.kind", FIELD_INVALID));
        }
        let t = parse_instant("from.at", Some(&from.at))?
            .ok_or_else(|| invalid("from.at", FIELD_REQUIRED))?;
        let margin = from.margin_minutes.unwrap_or(DEFAULT_MARGIN_MIN);
        if !(1..=MAX_MARGIN_MIN).contains(&margin) {
            return Err(invalid("from.margin_minutes", FIELD_INVALID));
        }
        let m = chrono::Duration::minutes(margin);
        props.insert(CASE_WINDOW_FROM.into(), Value::String((t - m).to_rfc3339()));
        props.insert(CASE_WINDOW_TO.into(), Value::String((t + m).to_rfc3339()));
        at = Some(t);
    }
    let case = objects::create(
        db,
        org_id,
        actor,
        &ObjectInput {
            type_key: KIND_CASE.into(),
            title: input.title.clone(),
            properties: props,
            expected_version: None,
        },
    )
    .await?;
    let (Some(from), Some(t)) = (&input.from, at) else {
        return Ok(case);
    };
    let Some(flow_id) = from.flow_id else {
        return Ok(case);
    };
    // A flow of another org, or deleted since, simply adds nothing (R1).
    let flow = match objects::find_by_native(db, org_id, KIND_FLOW, &flow_id.to_string()).await {
        Ok(f) if f.valid_to.is_none() => f,
        Ok(_) | Err(OntologyError::NotFound) => return Ok(case),
        Err(e) => return Err(e),
    };
    let flow_id = flow.id.to_string();
    links::create(db, org_id, actor, &member(&case.id, &flow_id, "subject")).await?;
    // The flow's neighbours at the event's instant: devices it drives,
    // objects it feeds, the POI it serves.
    let around = links::for_object(db, org_id, flow.id, Some(t), false).await?;
    let mut added = vec![flow.id];
    for l in around {
        let other = if l.source.id == flow_id {
            &l.target
        } else {
            &l.source
        };
        let Ok(id) = Uuid::parse_str(&other.id) else {
            continue;
        };
        if added.contains(&id) || other.type_key == KIND_CASE {
            continue;
        }
        added.push(id);
        // An archived neighbour cannot join: skip it, keep the others.
        match links::create(db, org_id, actor, &member(&case.id, &other.id, "context")).await {
            Ok(_) | Err(OntologyError::NotFound) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(case)
}

/// Type and link keys of the investigation pack (packs/investigation.yaml)
/// the board reads; an org without the pack simply gets an empty board.
const T_HYPOTHESIS: &str = "hypothesis";
const T_CLAIM: &str = "claim";
const T_PIECE: &str = "piece";
const T_PERSON: &str = "person";
const L_SUPPORTS: &str = "supports";
const L_CONTRADICTS: &str = "contradicts";
const L_SOURCED_BY: &str = "sourced_by";
/// Journal rows an export reads to replay states (`states_at`).
const REPLAY_CAP: i64 = 5000;
/// Members an export reads at most (one query per member for its links).
const EXPORT_CAP: usize = 1000;
/// Links that put their source somewhere: a device installed at a POI
/// (D191), where an event happened, where anything was (investigation).
const LOCATION_LINKS: [&str; 3] = [REL_PLACED_AT, "occurred_at", "located_at"];

/// Open `in_case` links of the case at `at` (now when None).
async fn members_at(
    db: &DatabaseConnection,
    org_id: i64,
    case_id: Uuid,
    at: Option<DateTimeWithTimeZone>,
) -> Result<Vec<LinkView>> {
    let case = objects::find(db, org_id, case_id).await?;
    if case.type_key != KIND_CASE {
        return Err(OntologyError::NotFound);
    }
    let id = case_id.to_string();
    Ok(links::for_object(db, org_id, case_id, at, false)
        .await?
        .into_iter()
        .filter(|l| l.link_type == REL_IN_CASE && l.target.id == id)
        .collect())
}

fn status_of(o: &ObjectView) -> Option<String> {
    o.properties
        .get("status")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A claim with its status and open sources, read once per board.
async fn claim_row(
    db: &DatabaseConnection,
    org_id: i64,
    s: &OrgSchema,
    claim: &ObjectRef,
    at: Option<DateTimeWithTimeZone>,
    seen: &mut HashMap<String, BoardClaim>,
) -> Result<BoardClaim> {
    if let Some(c) = seen.get(&claim.id) {
        return Ok(c.clone());
    }
    let id = Uuid::parse_str(&claim.id).map_err(|_| OntologyError::NotFound)?;
    let o = objects::view(&objects::find(db, org_id, id).await?, s);
    let sources = links::for_object(db, org_id, id, at, false)
        .await?
        .iter()
        .filter(|l| l.link_type == L_SOURCED_BY && l.source.id == claim.id)
        .count();
    let row = BoardClaim {
        claim: claim.clone(),
        // Today's status; the export replays it at its instant (`states_at`).
        status: status_of(&o),
        sources,
        justification: objects::claim_note(&o.properties),
    };
    seen.insert(claim.id.clone(), row.clone());
    Ok(row)
}

async fn board_of(
    db: &DatabaseConnection,
    org_id: i64,
    members: &[LinkView],
    at: Option<DateTimeWithTimeZone>,
) -> Result<CaseBoard> {
    let s = schema(db, org_id).await?;
    let mut seen = HashMap::new();
    let mut board = CaseBoard::default();
    for m in members {
        let r = &m.source;
        let Ok(id) = Uuid::parse_str(&r.id) else {
            continue;
        };
        if r.type_key == T_HYPOTHESIS {
            let o = objects::view(&objects::find(db, org_id, id).await?, &s);
            let mut h = BoardHypothesis {
                hypothesis: r.clone(),
                status: status_of(&o),
                supports: Vec::new(),
                contradicts: Vec::new(),
            };
            for l in links::for_object(db, org_id, id, at, false).await? {
                if l.target.id != r.id || l.source.type_key != T_CLAIM {
                    continue;
                }
                let side = match l.link_type.as_str() {
                    L_SUPPORTS => &mut h.supports,
                    L_CONTRADICTS => &mut h.contradicts,
                    _ => continue,
                };
                side.push(claim_row(db, org_id, &s, &l.source, at, &mut seen).await?);
            }
            board.hypotheses.push(h);
        } else if r.type_key == T_CLAIM {
            let row = claim_row(db, org_id, &s, r, at, &mut seen).await?;
            if row.sources == 0 {
                board.unsourced.push(row.clone());
            }
            let contradicted = links::for_object(db, org_id, id, at, false)
                .await?
                .iter()
                .any(|l| l.link_type == L_CONTRADICTS && l.target.id == r.id);
            if contradicted {
                board.contradicted.push(row);
            }
        }
    }
    Ok(board)
}

/// The hypotheses board of a case, now (cases.md K8).
pub async fn board(db: &DatabaseConnection, org_id: i64, case_id: Uuid) -> Result<CaseBoard> {
    let members = members_at(db, org_id, case_id, None).await?;
    board_of(db, org_id, &members, None).await
}

/// The case as it stood at `as_of` (default its freeze, else now), in one
/// document (cases.md K12). Read only.
/// Links valid at `at` of each member, in member order. A member whose
/// id is not a UUID cannot exist (in_case sources are objects), skipped.
async fn member_links(
    db: &DatabaseConnection,
    org_id: i64,
    members: &[LinkView],
    at: DateTimeWithTimeZone,
) -> Result<Vec<(Uuid, Vec<LinkView>)>> {
    let mut out = Vec::new();
    for m in members {
        let Ok(id) = Uuid::parse_str(&m.source.id) else {
            continue;
        };
        out.push((
            id,
            links::for_object(db, org_id, id, Some(at), false).await?,
        ));
    }
    Ok(out)
}

/// Links among members (not their membership), deduplicated, by id.
fn between(members: &[LinkView], per: &[(Uuid, Vec<LinkView>)]) -> Vec<LinkView> {
    let ids: HashSet<&str> = members.iter().map(|l| l.source.id.as_str()).collect();
    let mut by_id: HashMap<i64, LinkView> = HashMap::new();
    for e in per.iter().flat_map(|(_, ls)| ls) {
        if e.link_type != REL_IN_CASE
            && ids.contains(e.source.id.as_str())
            && ids.contains(e.target.id.as_str())
        {
            by_id.insert(e.id, e.clone());
        }
    }
    let mut out: Vec<LinkView> = by_id.into_values().collect();
    out.sort_by_key(|l| l.id);
    out
}

/// The case at an instant (K14): members then, links among them, places.
/// shortcut: `geo_point` values are today's even for a past `as_of`
/// (replaying them needs the O2 change journal), links are replayed.
pub async fn snapshot(
    db: &DatabaseConnection,
    org_id: i64,
    case_id: Uuid,
    q: &CaseExportQuery,
) -> Result<CaseSnapshot> {
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let at = parse_instant("as_of", q.as_of.as_deref())?.unwrap_or(now);
    let mut members = members_at(db, org_id, case_id, Some(at)).await?;
    members.truncate(EXPORT_CAP);
    let per = member_links(db, org_id, &members, at).await?;
    let located = |id: &Uuid, l: &&LinkView| {
        LOCATION_LINKS.contains(&l.link_type.as_str()) && l.source.id == id.to_string()
    };
    let mut wanted: HashSet<Uuid> = per.iter().map(|(id, _)| *id).collect();
    for (id, ls) in &per {
        wanted.extend(
            ls.iter()
                .filter(|l| located(id, l))
                .filter_map(|l| Uuid::parse_str(&l.target.id).ok()),
        );
    }
    let objs = links::refs(db, org_id, wanted).await?;
    let pin_ids: Vec<Uuid> = objs
        .values()
        .filter(|o| o.type_key == KIND_MAP_PIN)
        .filter_map(|o| Uuid::parse_str(&o.native_id).ok())
        .collect();
    let pins: HashMap<String, (f64, f64)> = map_pins::Entity::find()
        .filter(map_pins::Column::OrgId.eq(org_id))
        .filter(map_pins::Column::Id.is_in(pin_ids))
        .all(db)
        .await?
        .into_iter()
        .filter_map(|p| Some((p.id.to_string(), (d2f(p.latitude)?, d2f(p.longitude)?))))
        .collect();
    let s = schema(db, org_id).await?;
    let coords = |id: &str| -> Option<(f64, f64)> {
        let o = objs.get(&Uuid::parse_str(id).ok()?)?;
        if o.type_key == KIND_MAP_PIN {
            return pins.get(&o.native_id).copied();
        }
        s.object_type(&o.type_key)?
            .properties
            .iter()
            .filter(|p| matches!(p.kind, PropertyKind::GeoPoint))
            .find_map(|p| {
                let v = o.properties.get(&p.key)?;
                Some((v.get("lat")?.as_f64()?, v.get("lon")?.as_f64()?))
            })
    };
    let mut places = Vec::new();
    for (m, (id, ls)) in members.iter().zip(&per) {
        if let Some((lat, lon)) = coords(&m.source.id) {
            places.push(CasePlace {
                object: m.source.clone(),
                lat,
                lon,
                via: None,
                link_type: String::new(),
            });
        }
        for l in ls.iter().filter(|l| located(id, l)) {
            if let Some((lat, lon)) = coords(&l.target.id) {
                places.push(CasePlace {
                    object: m.source.clone(),
                    lat,
                    lon,
                    via: Some(l.target.clone()),
                    link_type: l.link_type.clone(),
                });
            }
        }
    }
    Ok(CaseSnapshot {
        as_of: ts(&at),
        graph: Neighborhood {
            nodes: members.iter().map(|l| l.source.clone()).collect(),
            links: between(&members, &per),
        },
        places,
    })
}

pub async fn export(
    db: &DatabaseConnection,
    o2: Option<&Client>,
    org_id: i64,
    case_id: Uuid,
    q: &CaseExportQuery,
) -> Result<CaseExport> {
    let s = schema(db, org_id).await?;
    let case = objects::view(&objects::find(db, org_id, case_id).await?, &s);
    let frozen = case.properties.get(CASE_FROZEN_AT).and_then(Value::as_str);
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let at = parse_instant("as_of", q.as_of.as_deref().or(frozen))?.unwrap_or(now);
    let mut members_links = members_at(db, org_id, case_id, Some(at)).await?;
    members_links.truncate(EXPORT_CAP);
    let per = member_links(db, org_id, &members_links, at).await?;
    let links = between(&members_links, &per);
    let mut members = Vec::new();
    for (l, (id, _)) in members_links.iter().zip(&per) {
        members.push(ExportMember {
            link: l.clone(),
            object: objects::view(&objects::find(db, org_id, *id).await?, &s),
        });
    }
    let text = |o: &ObjectView, k: &str| {
        o.properties
            .get(k)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let pieces = members
        .iter()
        .filter(|m| m.object.type_key == T_PIECE)
        .map(|m| PieceEntry {
            id: m.object.id.clone(),
            title: m.object.title.clone(),
            source_url: text(&m.object, "source_url"),
            sha256: text(&m.object, "sha256"),
            captured_at: text(&m.object, "captured_at"),
        })
        .collect();
    let persons = members
        .iter()
        .filter(|m| m.object.type_key == T_PERSON)
        .count();
    let timeline = timeline::for_case(
        db,
        o2,
        org_id,
        case_id,
        &TimelineQuery {
            from: None,
            to: Some(ts(&at)),
        },
    )
    .await?;
    let mut board = board_of(db, org_id, &members_links, Some(at)).await?;
    // Objects whose state is replayed: the members and the board entries.
    let mut ids: Vec<String> = members.iter().map(|m| m.object.id.clone()).collect();
    for h in &board.hypotheses {
        ids.push(h.hypothesis.id.clone());
        ids.extend(
            h.supports
                .iter()
                .chain(&h.contradicts)
                .map(|c| c.claim.id.clone()),
        );
    }
    let states = states_at(db, o2, org_id, &ids, at, now).await;
    if let Some(states) = &states {
        for m in &mut members {
            if let Some(o) = states.get(&m.object.id) {
                m.object = o.clone();
            }
        }
        replay_board(&mut board, states);
    }
    Ok(CaseExport {
        exported_at: ts(&now),
        as_of: ts(&at),
        case,
        members,
        links,
        timeline,
        board,
        pieces,
        persons,
        states_replayed: states.is_some(),
    })
}

/// Object states at `at` (K12): the `before` of each object's first change
/// after `at`; an object unchanged since keeps its current state. `None`
/// when the change journal is unavailable.
/// shortcut: at most `REPLAY_CAP` changes after `at` are read; an object
/// changed more often than that since may replay a later state.
async fn states_at(
    db: &DatabaseConnection,
    o2: Option<&Client>,
    org_id: i64,
    ids: &[String],
    at: DateTimeWithTimeZone,
    now: DateTimeWithTimeZone,
) -> Option<HashMap<String, ObjectView>> {
    let rows = changes::history_of(
        db,
        o2,
        org_id,
        ids,
        at.timestamp_micros() + 1,
        now.timestamp_micros(),
        REPLAY_CAP,
    )
    .await
    .ok()??;
    Some(fold_states(rows))
}

/// Journal rows newest first → state before each object's first change.
fn fold_states(rows: Vec<(String, ChangeView)>) -> HashMap<String, ObjectView> {
    let mut out = HashMap::new();
    // Newest first: the last insert per object is its first change after `at`.
    for (id, c) in rows {
        if !c.action.starts_with("object.") {
            continue;
        }
        match serde_json::from_value::<ObjectView>(c.before) {
            Ok(o) => out.insert(id, o),
            Err(_) => out.remove(&id),
        };
    }
    out
}

fn replay_board(board: &mut CaseBoard, states: &HashMap<String, ObjectView>) {
    let claim = |c: &mut BoardClaim| {
        if let Some(o) = states.get(&c.claim.id) {
            c.status = status_of(o);
            c.justification = objects::claim_note(&o.properties);
        }
    };
    for h in &mut board.hypotheses {
        if let Some(o) = states.get(&h.hypothesis.id) {
            h.status = status_of(o);
        }
        h.supports
            .iter_mut()
            .chain(&mut h.contradicts)
            .for_each(claim);
    }
    board
        .unsourced
        .iter_mut()
        .chain(&mut board.contradicted)
        .for_each(claim);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(action: &str, status: Option<&str>) -> (String, ChangeView) {
        let before = status.map_or(Value::Null, |st| {
            serde_json::json!({"id": "c1", "type_key": "claim", "title": "C", "native_id": "c1",
                "system": false, "properties": {"status": st}, "version": 1,
                "valid_from": "", "created_at": "", "updated_at": ""})
        });
        let c = ChangeView {
            at: String::new(),
            action: action.into(),
            source_ref: String::new(),
            before,
            after: Value::Null,
        };
        ("c1".into(), c)
    }

    #[test]
    fn state_at_is_before_the_first_later_change() {
        // Newest first: unverified → contested → corroborated after `at`.
        let rows = vec![
            change("object.update", Some("contested")),
            change("link.open", None),
            change("object.update", Some("unverified")),
        ];
        let s = fold_states(rows);
        assert_eq!(status_of(&s["c1"]).as_deref(), Some("unverified"));
        // Created after `at`: no state then.
        assert!(fold_states(vec![change("object.create", None)]).is_empty());
    }
}
