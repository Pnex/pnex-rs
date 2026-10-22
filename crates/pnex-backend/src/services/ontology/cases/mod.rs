//! Investigation cases (cases-workspace.md). A case holds dated events and
//! undated pieces through `in_case` links; objects are attached to events by
//! `involves` with a role (evidence, actor, context, hypothesis, source).
//! Members are derived: the events, what they involve, the undated pieces.
//!
//! Every write goes through the generic object and link services (same
//! validations, same role lock D188), inside one transaction per gesture.

mod board;
mod detect;
mod export;
mod graph;
mod imports;
mod layout;
pub mod pages;
mod purge;
mod reads;
mod signals;
mod snapshots;
mod tracks;

use std::collections::HashSet;

pub use board::board;
pub use detect::{detect, proposals, reject, retain};
pub use export::{export, import as import_case};
pub use graph::{expand, relations};
pub use imports::{delete_import, import, imports, MAX_BYTES as IMPORT_MAX_BYTES};
pub use layout::{layout, save_layout};
pub use pages::{add_page, on_concluded, templates};
pub use purge::{candidates, purge};
pub use reads::{reads, spawn_writer as spawn_reads_writer, Reader};
pub use signals::signals;
pub use snapshots::{freeze, snapshot, snapshots};
pub use tracks::tracks;

use pnex_core::err_codes::{FIELD_INVALID, FIELD_INVALID_UUID, FIELD_REQUIRED};
use pnex_core::geo_area::GeoArea;
use pnex_core::ontology::api::{
    CaseAttachInput, CaseAttachment, CaseChild, CaseDetachInput, CaseEvent, CaseEventInput,
    CaseOpenInput, CasePlace, CaseWorkspace, CaseZone, LinkInput, ObjectInput, ObjectRef,
    ObjectView,
};
use pnex_core::ontology::{
    CASE_LEGAL_BASIS, CASE_STATUS, EVENT_OCCURRED, EVENT_ORIGIN, INVOLVES_DETAIL, INVOLVES_ROLE,
    KIND_CASE, KIND_EVENT, KIND_NOTE, REL_INVOLVES, REL_IN_CASE,
};
use pnex_core::resources::{KIND_FLOW, KIND_MAP_PIN};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter,
    TransactionTrait,
};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::{invalid, links, objects, parse_instant, schema, Actor, OntologyError, Result};
use crate::models::_entities::{map_pins, objects as object_rows, resource_edges};
use crate::services::resources::containment;

/// Pack types the case logic knows by key (packs/investigation.yaml); an org
/// without the pack simply never meets them.
pub(crate) const T_PERSON: &str = "person";
const T_ORGANIZATION: &str = "organization";
pub(crate) const T_PLACE: &str = "place";
const T_HYPOTHESIS: &str = "hypothesis";

/// Role proposed when an object is attached (K21), changeable afterwards.
pub fn default_role(type_key: &str) -> &'static str {
    match type_key {
        T_PERSON | T_ORGANIZATION => "actor",
        KIND_MAP_PIN | T_PLACE => "context",
        KIND_NOTE | T_HYPOTHESIS => "hypothesis",
        KIND_FLOW => "source",
        _ => "evidence",
    }
}

/// Open links of a case, as read inside the current transaction.
pub(crate) struct Membership {
    pub case: object_rows::Model,
    /// Open `in_case` links of the events, with the event.
    pub events: Vec<(resource_edges::Model, object_rows::Model)>,
    /// Open `involves` links from those events.
    pub involves: Vec<resource_edges::Model>,
    /// Open `in_case` links of the undated pieces, with the piece.
    pub undated: Vec<(resource_edges::Model, object_rows::Model)>,
}

impl Membership {
    fn is_event(&self, id: Uuid) -> bool {
        self.events.iter().any(|(_, o)| o.id == id)
    }

    fn attached(&self, id: Uuid) -> bool {
        self.involves.iter().any(|e| e.target_object_id == Some(id))
    }

    fn undated_link(&self, id: Uuid) -> Option<&resource_edges::Model> {
        self.undated
            .iter()
            .find(|(_, o)| o.id == id)
            .map(|(e, _)| e)
    }

    /// Events, what they involve, undated pieces; each once, in that order.
    pub fn member_ids(&self) -> Vec<Uuid> {
        let mut seen = HashSet::new();
        self.events
            .iter()
            .map(|(_, o)| o.id)
            .chain(self.involves.iter().filter_map(|e| e.target_object_id))
            .chain(self.undated.iter().map(|(_, o)| o.id))
            .filter(|id| seen.insert(*id))
            .collect()
    }
}

async fn case_of<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    case_id: Uuid,
) -> Result<object_rows::Model> {
    let case = objects::find(db, org_id, case_id).await?;
    if case.type_key != KIND_CASE || case.valid_to.is_some() {
        return Err(OntologyError::NotFound);
    }
    Ok(case)
}

pub(crate) async fn membership<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    case_id: Uuid,
) -> Result<Membership> {
    let case = case_of(db, org_id, case_id).await?;
    let in_case = resource_edges::Entity::find()
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .filter(resource_edges::Column::Relation.eq(REL_IN_CASE))
        .filter(resource_edges::Column::TargetObjectId.eq(case_id))
        .filter(resource_edges::Column::ValidTo.is_null())
        .all(db)
        .await?;
    let objs = links::refs(
        db,
        org_id,
        in_case.iter().filter_map(|e| e.source_object_id),
    )
    .await?;
    let (mut events, mut undated) = (Vec::new(), Vec::new());
    for e in in_case {
        let Some(o) = e.source_object_id.and_then(|id| objs.get(&id)).cloned() else {
            continue;
        };
        if o.type_key == KIND_EVENT {
            events.push((e, o));
        } else {
            undated.push((e, o));
        }
    }
    let event_ids: Vec<Uuid> = events.iter().map(|(_, o)| o.id).collect();
    let involves = if event_ids.is_empty() {
        Vec::new()
    } else {
        resource_edges::Entity::find()
            .filter(resource_edges::Column::OrgId.eq(org_id))
            .filter(resource_edges::Column::Relation.eq(REL_INVOLVES))
            .filter(resource_edges::Column::SourceObjectId.is_in(event_ids))
            .filter(resource_edges::Column::ValidTo.is_null())
            .all(db)
            .await?
    };
    Ok(Membership {
        case,
        events,
        involves,
        undated,
    })
}

fn link(kind: &str, source: Uuid, target: Uuid, attributes: Map<String, Value>) -> LinkInput {
    LinkInput {
        link_type: kind.into(),
        source_id: source.to_string(),
        target_id: target.to_string(),
        attributes,
        valid_from: None,
        valid_to: None,
    }
}

/// `in_case` input: an event or an undated piece joins the case.
pub(crate) fn in_case(case_id: Uuid, object_id: Uuid) -> LinkInput {
    link(REL_IN_CASE, object_id, case_id, Map::new())
}

fn parse_id(field: &str, raw: &str) -> Result<Uuid> {
    Uuid::parse_str(raw.trim()).map_err(|_| invalid(field, FIELD_INVALID_UUID))
}

/// K38: a person joins a case only once its processing basis is declared.
fn guard_person(case: &object_rows::Model, obj: &object_rows::Model) -> Result<()> {
    let declared = case
        .properties
        .get(CASE_LEGAL_BASIS)
        .and_then(Value::as_str)
        .is_some_and(|b| !b.trim().is_empty());
    if obj.type_key == T_PERSON && !declared {
        return Err(OntologyError::CaseLegalBasisRequired);
    }
    Ok(())
}

/// Instant of an event's `occurred` lower bound, for ordering.
fn occurred_from(o: &ObjectView) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    o.properties
        .get(EVENT_OCCURRED)?
        .get("from")?
        .as_str()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
}

/// The case as the workspace shows it: events oldest first (undated events
/// last), their attachments, the undated pieces.
pub async fn workspace(
    db: &DatabaseConnection,
    org_id: i64,
    case_id: Uuid,
) -> Result<CaseWorkspace> {
    let m = membership(db, org_id, case_id).await?;
    let s = schema(db, org_id).await?;
    // Attachment order: the order they were made in.
    let mut views = links::views(db, org_id, &m.involves).await?;
    views.sort_by_key(|v| v.id);
    let mut events: Vec<CaseEvent> = m
        .events
        .iter()
        .map(|(_, o)| {
            let id = o.id.to_string();
            CaseEvent {
                event: objects::view(o, &s),
                attachments: views
                    .iter()
                    .filter(|v| v.source.id == id)
                    .map(|v| CaseAttachment {
                        link_id: v.id,
                        object: v.target.clone(),
                        role: v
                            .attributes
                            .get(INVOLVES_ROLE)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                        detail: v
                            .attributes
                            .get(INVOLVES_DETAIL)
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                    .collect(),
            }
        })
        .collect();
    events.sort_by(|a, b| {
        let (ta, tb) = (occurred_from(&a.event), occurred_from(&b.event));
        tb.is_none()
            .cmp(&ta.is_none())
            .then(ta.cmp(&tb))
            .then(a.event.title.cmp(&b.event.title))
    });
    let members = links::refs(db, org_id, m.member_ids()).await?;
    let mut zones: Vec<CaseZone> = zones_of(&members)
        .into_iter()
        .map(|(o, _, area)| CaseZone {
            object: links::object_ref(o),
            area,
        })
        .collect();
    zones.sort_by(|a, b| a.object.title.cmp(&b.object.title));
    let places = places_of(db, org_id, &members).await?;
    let (parents, children) = family_of(db, org_id, case_id, &m).await?;
    Ok(CaseWorkspace {
        case: objects::view(&m.case, &s),
        events,
        undated: m
            .undated
            .iter()
            .map(|(_, o)| links::object_ref(o))
            .collect(),
        zones,
        places,
        parents,
        children,
    })
}

/// The enclosing cases, nearest first, and the sub-cases with what they
/// share with this one (GS16), through the containment tree. A folder ends
/// the chain of parents.
async fn family_of(
    db: &DatabaseConnection,
    org_id: i64,
    case_id: Uuid,
    m: &Membership,
) -> Result<(Vec<ObjectRef>, Vec<CaseChild>)> {
    let id = case_id.to_string();
    let mut parents = Vec::new();
    for (kind, pid) in containment::path_of(db, org_id, KIND_CASE, &id).await? {
        let Some(uid) = (kind == KIND_CASE)
            .then(|| Uuid::parse_str(&pid).ok())
            .flatten()
        else {
            break;
        };
        match case_of(db, org_id, uid).await {
            Ok(c) => parents.push(links::object_ref(&c)),
            Err(OntologyError::NotFound) => break,
            Err(e) => return Err(e),
        }
    }
    let mine: HashSet<Uuid> = m.member_ids().into_iter().collect();
    let mut children = Vec::new();
    for (kind, cid, _) in containment::children_of(db, org_id, KIND_CASE, &id).await? {
        let Some(uid) = (kind == KIND_CASE)
            .then(|| Uuid::parse_str(&cid).ok())
            .flatten()
        else {
            continue;
        };
        let sub = match membership(db, org_id, uid).await {
            Ok(sub) => sub,
            Err(OntologyError::NotFound) => continue,
            Err(e) => return Err(e),
        };
        let ids = sub.member_ids();
        children.push(CaseChild {
            case: links::object_ref(&sub.case),
            members: ids.len(),
            shared: ids
                .iter()
                .filter(|i| mine.contains(i))
                .map(Uuid::to_string)
                .collect(),
        });
    }
    children.sort_by(|a, b| a.case.title.cmp(&b.case.title));
    Ok((parents, children))
}

/// Where each member stands: its first `geo_point` value, else the centre of
/// its first area, else (a POI) the pin's coordinates. Sorted by title.
async fn places_of<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    objs: &std::collections::HashMap<Uuid, object_rows::Model>,
) -> Result<Vec<CasePlace>> {
    let point = |v: &Value| -> Option<(f64, f64)> {
        let o = v.as_object().filter(|o| o.len() == 2)?;
        Some((o.get("lon")?.as_f64()?, o.get("lat")?.as_f64()?))
    };
    let pin_ids: Vec<Uuid> = objs
        .values()
        .filter(|o| o.type_key == KIND_MAP_PIN)
        .filter_map(|o| Uuid::parse_str(&o.native_id).ok())
        .collect();
    let pins: std::collections::HashMap<String, (f64, f64)> = if pin_ids.is_empty() {
        Default::default()
    } else {
        map_pins::Entity::find()
            .filter(map_pins::Column::OrgId.eq(org_id))
            .filter(map_pins::Column::Id.is_in(pin_ids))
            .all(db)
            .await?
            .into_iter()
            .filter_map(|p| {
                let f = |d: Option<sea_orm::prelude::Decimal>| d?.to_string().parse::<f64>().ok();
                let (lon, lat) = (f(p.longitude)?, f(p.latitude)?);
                Some((p.id.to_string(), (lon, lat)))
            })
            .collect()
    };
    let mut out: Vec<CasePlace> = objs
        .values()
        .filter_map(|o| {
            let props = o.properties.as_object();
            let at = props
                .and_then(|p| p.values().find_map(point))
                .or_else(|| {
                    props.and_then(|p| p.values().find_map(|v| Some(GeoArea::parse(v)?.center())))
                })
                .or_else(|| pins.get(&o.native_id).copied())?;
            Some(CasePlace {
                object: links::object_ref(o),
                lon: at.0,
                lat: at.1,
            })
        })
        .collect();
    out.sort_by(|a, b| a.object.title.cmp(&b.object.title));
    Ok(out)
}

/// Areas held by objects: every property value that is a valid `geo_area`.
pub(crate) fn zones_of(
    objs: &std::collections::HashMap<Uuid, object_rows::Model>,
) -> Vec<(&object_rows::Model, GeoArea, Value)> {
    objs.values()
        .flat_map(|o| {
            o.properties
                .as_object()
                .into_iter()
                .flat_map(|p| p.values())
                .filter_map(move |v| Some((o, GeoArea::parse(v)?, v.clone())))
        })
        .collect()
}

/// Attaches inside a transaction; idempotent (an existing attachment or
/// membership is kept as is). Attaching again with another role or detail
/// changes it: the old attachment closes, a new one opens.
async fn attach_in<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    input: &CaseAttachInput,
) -> Result<()> {
    let m = membership(db, org_id, case_id).await?;
    let obj_id = parse_id("object_id", &input.object_id)?;
    let obj = objects::find(db, org_id, obj_id).await?;
    if obj.valid_to.is_some() || obj.id == case_id {
        return Err(OntologyError::NotFound);
    }
    guard_person(&m.case, &obj)?;
    let Some(raw) = &input.event_id else {
        // Undated piece, or an existing event joining the case.
        let member = m.is_event(obj_id) || m.attached(obj_id) || m.undated_link(obj_id).is_some();
        if !member {
            links::create(db, org_id, actor, &in_case(case_id, obj_id)).await?;
        }
        return Ok(());
    };
    let event_id = parse_id("event_id", raw)?;
    if !m.is_event(event_id) {
        return Err(invalid("event_id", FIELD_INVALID));
    }
    let open = m
        .involves
        .iter()
        .find(|e| e.source_object_id == Some(event_id) && e.target_object_id == Some(obj_id));
    // Another role or detail asked: the old attachment closes (history kept).
    let current = open.map(|e| {
        let attr = |k: &str| {
            e.attributes
                .get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        (attr(INVOLVES_ROLE), attr(INVOLVES_DETAIL))
    });
    let wanted_detail = input
        .detail
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string);
    let changed = current.as_ref().is_some_and(|(role, detail)| {
        input
            .role
            .as_ref()
            .is_some_and(|r| Some(r) != role.as_ref())
            || (input.detail.is_some() && &wanted_detail != detail)
    });
    if let (Some(e), true) = (open, changed) {
        links::close(db, org_id, actor, e.id, None).await?;
    }
    if open.is_none() || changed {
        let mut attrs = Map::new();
        let kept_role = current.as_ref().and_then(|(r, _)| r.clone());
        let role = input
            .role
            .clone()
            .or(kept_role)
            .unwrap_or_else(|| default_role(&obj.type_key).to_string());
        attrs.insert(INVOLVES_ROLE.into(), role.into());
        let detail = if input.detail.is_some() {
            wanted_detail
        } else {
            current.and_then(|(_, d)| d)
        };
        if let Some(d) = detail {
            attrs.insert(INVOLVES_DETAIL.into(), d.into());
        }
        links::create(
            db,
            org_id,
            actor,
            &link(REL_INVOLVES, event_id, obj_id, attrs),
        )
        .await?;
    }
    // Dated now: it leaves the undated pieces.
    if let Some(e) = m.undated_link(obj_id) {
        links::close(db, org_id, actor, e.id, None).await?;
    }
    Ok(())
}

pub async fn attach(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    input: &CaseAttachInput,
) -> Result<CaseWorkspace> {
    crate::services::ontology::changes::deferred(async {
        let txn = db.begin().await?;
        attach_in(&txn, org_id, actor, case_id, input).await?;
        txn.commit().await?;
        crate::services::ontology::changes::flush();
        workspace(db, org_id, case_id).await
    })
    .await
}

/// Puts `id` among the undated pieces unless something still holds it.
async fn keep_in_case<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    id: Uuid,
) -> Result<()> {
    let m = membership(db, org_id, case_id).await?;
    if !m.is_event(id) && !m.attached(id) && m.undated_link(id).is_none() {
        links::create(db, org_id, actor, &in_case(case_id, id)).await?;
    }
    Ok(())
}

pub async fn detach(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    input: &CaseDetachInput,
) -> Result<CaseWorkspace> {
    crate::services::ontology::changes::deferred(async {
        let obj_id = parse_id("object_id", &input.object_id)?;
        let txn = db.begin().await?;
        let m = membership(&txn, org_id, case_id).await?;
        if let Some(raw) = &input.event_id {
            let event_id = parse_id("event_id", raw)?;
            let edge = m
                .involves
                .iter()
                .find(|e| {
                    e.source_object_id == Some(event_id) && e.target_object_id == Some(obj_id)
                })
                .ok_or(OntologyError::NotFound)?;
            links::close(&txn, org_id, actor, edge.id, None).await?;
            keep_in_case(&txn, org_id, actor, case_id, obj_id).await?;
        } else if let Some((edge, _)) = m.events.iter().find(|(_, o)| o.id == obj_id) {
            // The event leaves the case; its pieces stay in it.
            links::close(&txn, org_id, actor, edge.id, None).await?;
            let pieces: Vec<Uuid> = m
                .involves
                .iter()
                .filter(|e| e.source_object_id == Some(obj_id))
                .filter_map(|e| e.target_object_id)
                .collect();
            for p in pieces {
                keep_in_case(&txn, org_id, actor, case_id, p).await?;
            }
        } else if let Some(edge) = m.undated_link(obj_id) {
            links::close(&txn, org_id, actor, edge.id, None).await?;
        } else if m.attached(obj_id) {
            // Out of the case entirely: every attachment to its events closes.
            for e in m
                .involves
                .iter()
                .filter(|e| e.target_object_id == Some(obj_id))
            {
                links::close(&txn, org_id, actor, e.id, None).await?;
            }
        } else {
            return Err(OntologyError::NotFound);
        }
        txn.commit().await?;
        crate::services::ontology::changes::flush();
        workspace(db, org_id, case_id).await
    })
    .await
}

async fn create_event_in<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    input: &CaseEventInput,
    mut props: Map<String, Value>,
) -> Result<Uuid> {
    case_of(db, org_id, case_id).await?;
    props.insert(EVENT_OCCURRED.into(), input.occurred.clone());
    props.insert(
        EVENT_ORIGIN.into(),
        input.origin.as_deref().unwrap_or("manual").into(),
    );
    let event = objects::create(
        db,
        org_id,
        actor,
        &ObjectInput {
            type_key: KIND_EVENT.into(),
            title: input.title.clone(),
            properties: props,
            expected_version: None,
        },
    )
    .await?;
    let event_id = Uuid::parse_str(&event.id).map_err(|_| OntologyError::NotFound)?;
    links::create(db, org_id, actor, &in_case(case_id, event_id)).await?;
    for a in &input.attach {
        let a = CaseAttachInput {
            event_id: Some(event.id.clone()),
            ..a.clone()
        };
        attach_in(db, org_id, actor, case_id, &a).await?;
    }
    Ok(event_id)
}

/// A new event and its first attachments, in one transaction.
pub async fn create_event(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    input: &CaseEventInput,
) -> Result<CaseWorkspace> {
    crate::services::ontology::changes::deferred(async {
        let txn = db.begin().await?;
        create_event_in(&txn, org_id, actor, case_id, input, Map::new()).await?;
        txn.commit().await?;
        crate::services::ontology::changes::flush();
        workspace(db, org_id, case_id).await
    })
    .await
}

/// `fuzzy_time` value of an exact instant (UTC, `Z` suffix).
fn instant(t: chrono::DateTime<chrono::FixedOffset>) -> Value {
    let z = t
        .with_timezone(&chrono::Utc)
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    serde_json::json!({ "from": z, "to": z, "precision": "instant" })
}

/// Opens a case. From an alert, its first event is the alert: the flow that
/// raised it attached as source, the flow's neighbours at that instant as
/// context (a flow of another org adds nothing, R1).
pub async fn open(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    input: &CaseOpenInput,
) -> Result<ObjectView> {
    crate::services::ontology::changes::deferred(async {
        const SOURCE_KINDS: [&str; 2] = ["notification", "event"];
        let at = match &input.from {
            Some(from) if !SOURCE_KINDS.contains(&from.kind.as_str()) => {
                return Err(invalid("from.kind", FIELD_INVALID))
            }
            Some(from) => Some(
                parse_instant("from.at", Some(&from.at))?
                    .ok_or_else(|| invalid("from.at", FIELD_REQUIRED))?,
            ),
            None => None,
        };
        let mut props = Map::new();
        props.insert(CASE_STATUS.into(), "open".into());
        // Sub-case (GS16): the parent is a live case of the org (R1); its legal
        // basis is copied, so persons can join at once (K38).
        let parent = match input.parent_id.as_deref() {
            Some(raw) => match case_of(db, org_id, parse_id("parent_id", raw)?).await {
                Ok(p) => Some(p),
                Err(OntologyError::NotFound) => return Err(invalid("parent_id", FIELD_INVALID)),
                Err(e) => return Err(e),
            },
            None => None,
        };
        if let Some(basis) = parent
            .as_ref()
            .and_then(|p| p.properties.get(CASE_LEGAL_BASIS))
        {
            props.insert(CASE_LEGAL_BASIS.into(), basis.clone());
        }
        let txn = db.begin().await?;
        let case = objects::create(
            &txn,
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
        if let (Some(from), Some(t)) = (&input.from, at) {
            let mut attach = Vec::new();
            let flow = match from.flow_id {
                Some(id) => {
                    match objects::find_by_native(&txn, org_id, KIND_FLOW, &id.to_string()).await {
                        Ok(f) if f.valid_to.is_none() => Some(f),
                        Ok(_) | Err(OntologyError::NotFound) => None,
                        Err(e) => return Err(e),
                    }
                }
                None => None,
            };
            if let Some(flow) = flow {
                attach.push(CaseAttachInput {
                    object_id: flow.id.to_string(),
                    ..Default::default()
                });
                // Devices it drives, objects it feeds, the POI it serves.
                for l in links::for_object(db, org_id, flow.id, Some(t), false).await? {
                    let other = if l.source.id == flow.id.to_string() {
                        &l.target
                    } else {
                        &l.source
                    };
                    let live = objects::find(&txn, org_id, parse_id("id", &other.id)?)
                        .await
                        .is_ok_and(|o| o.valid_to.is_none());
                    if live
                        && other.type_key != KIND_CASE
                        && attach.iter().all(|a| a.object_id != other.id)
                    {
                        attach.push(CaseAttachInput {
                            object_id: other.id.clone(),
                            role: Some("context".into()),
                            ..Default::default()
                        });
                    }
                }
            }
            let case_id = Uuid::parse_str(&case.id).map_err(|_| OntologyError::NotFound)?;
            let event = CaseEventInput {
                title: input.title.clone(),
                occurred: instant(t),
                origin: Some("signal".into()),
                attach,
            };
            create_event_in(&txn, org_id, actor, case_id, &event, Map::new()).await?;
        }
        txn.commit().await?;
        crate::services::ontology::changes::flush();
        if let Some(p) = parent {
            let parent_id = p.id.to_string();
            containment::set_parent(
                db,
                org_id,
                KIND_CASE,
                &case.id,
                Some((KIND_CASE, &parent_id)),
                None,
            )
            .await
            // A fresh case under a live case: only the database can fail here.
            .map_err(|e| OntologyError::Db(DbErr::Custom(format!("case containment: {e:?}"))))?;
        }
        Ok(case)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roles_follow_the_object_type() {
        assert_eq!(default_role(T_PERSON), "actor");
        assert_eq!(default_role(KIND_MAP_PIN), "context");
        assert_eq!(default_role(KIND_NOTE), "hypothesis");
        assert_eq!(default_role(KIND_FLOW), "source");
        assert_eq!(default_role("media_asset"), "evidence");
    }

    #[test]
    fn instant_is_a_zero_width_utc_interval() {
        let t = chrono::DateTime::parse_from_rfc3339("2026-10-10T16:32:00+02:00").unwrap();
        assert_eq!(
            instant(t),
            serde_json::json!({"from": "2026-10-10T14:32:00Z", "to": "2026-10-10T14:32:00Z", "precision": "instant"})
        );
    }
}
