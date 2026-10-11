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
    BoardClaim, BoardHypothesis, CaseBoard, CaseExport, CaseExportQuery, CaseOpenInput,
    ExportMember, LinkInput, LinkView, ObjectInput, ObjectRef, ObjectView, PieceEntry,
    TimelineQuery,
};
use pnex_core::ontology::{
    CASE_FROZEN_AT, CASE_STATUS, CASE_WINDOW_FROM, CASE_WINDOW_TO, IN_CASE_ROLE, KIND_CASE,
    REL_IN_CASE,
};
use pnex_core::resources::KIND_FLOW;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::DatabaseConnection;
use serde_json::{Map, Value};
use uuid::Uuid;

use super::{
    invalid, links, objects, parse_instant, schema, timeline, ts, Actor, OntologyError, OrgSchema,
    Result,
};
use crate::services::openobserve::Client;

const DEFAULT_MARGIN_MIN: i64 = 120;
const MAX_MARGIN_MIN: i64 = 7 * 24 * 60;
const SOURCE_KINDS: [&str; 2] = ["notification", "event"];

fn member(case_id: &str, object_id: &str, role: &str) -> LinkInput {
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
/// Members an export reads at most (one query per member for its links).
const EXPORT_CAP: usize = 1000;

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
        // shortcut: the status is today's even for a past `at`; replaying it
        // needs the O2 change journal (K12 freeze of statuses).
        status: status_of(&o),
        sources,
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
    let ids: HashSet<String> = members_links.iter().map(|l| l.source.id.clone()).collect();
    let mut members = Vec::new();
    let mut between: HashMap<i64, LinkView> = HashMap::new();
    for l in &members_links {
        let Ok(id) = Uuid::parse_str(&l.source.id) else {
            continue;
        };
        members.push(ExportMember {
            link: l.clone(),
            object: objects::view(&objects::find(db, org_id, id).await?, &s),
        });
        for e in links::for_object(db, org_id, id, Some(at), false).await? {
            if e.link_type != REL_IN_CASE
                && ids.contains(&e.source.id)
                && ids.contains(&e.target.id)
            {
                between.insert(e.id, e);
            }
        }
    }
    let mut links: Vec<LinkView> = between.into_values().collect();
    links.sort_by_key(|l| l.id);
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
    let board = board_of(db, org_id, &members_links, Some(at)).await?;
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
    })
}
