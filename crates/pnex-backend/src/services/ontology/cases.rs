//! Opening a case (cases.md K1, K3): the `case` object, then, when opened
//! from a dated event, its window around the event and its first members
//! (the flow that raised it as subject, the objects linked to that flow at
//! the event's instant as context). Every write goes through the generic
//! object and link services: same validations, same role lock (D188).

use pnex_core::err_codes::{FIELD_INVALID, FIELD_REQUIRED};
use pnex_core::ontology::api::{CaseOpenInput, LinkInput, ObjectInput, ObjectView};
use pnex_core::ontology::{
    CASE_STATUS, CASE_WINDOW_FROM, CASE_WINDOW_TO, IN_CASE_ROLE, KIND_CASE, REL_IN_CASE,
};
use pnex_core::resources::KIND_FLOW;
use sea_orm::DatabaseConnection;
use serde_json::{Map, Value};
use uuid::Uuid;

use super::{invalid, links, objects, parse_instant, Actor, OntologyError, Result};

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
