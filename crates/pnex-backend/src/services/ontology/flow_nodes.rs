//! Ontology flow nodes on the server side (partner-watch.md D312). The
//! flow runtime runs user code and is not trusted: each call names its
//! deployed node, whose config is read from the *deployed* version of the
//! flow (never from the request); the writes go through the services the
//! HTTP controller uses (same validations, same 409), as the flow
//! (`flow:<id>@<version>`, member role: an admin-only type stays out of
//! reach), never on a native system type, within a per-org write budget.

use std::collections::HashMap;
use std::sync::Mutex;

use pnex_core::flow::{FlowGraph, FlowNodeKind};
use pnex_core::ontology::api::{
    LinkInput, ObjectInput, OntologyQuery, PropertyFilter, QueryResult,
};
use pnex_core::ontology::flow::{
    FlowLinkRequest, FlowNodeRef, FlowQueryRequest, FlowUpsertAck, FlowUpsertRequest,
    WRITES_PER_MINUTE,
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use serde_json::Value;
use uuid::Uuid;

use super::{invalid, links, objects, query, schema, Actor, OntologyError, Result};
use crate::models::_entities::sea_orm_active_enums::OrgMemberRole;
use crate::models::_entities::{flow_versions, flows, resource_edges};

/// Why a call is refused before any ontology rule (runtime diagnostics).
#[derive(Debug)]
pub enum NodeError {
    /// No deployed node of that kind at that version: stale or forged.
    NotDeployed,
    /// The org spent its write budget of the minute.
    Budget,
    Onto(OntologyError),
}

impl From<OntologyError> for NodeError {
    fn from(e: OntologyError) -> Self {
        Self::Onto(e)
    }
}

impl From<sea_orm::DbErr> for NodeError {
    fn from(e: sea_orm::DbErr) -> Self {
        Self::Onto(e.into())
    }
}

/// The deployed node named by `r`.
pub(crate) async fn deployed(
    db: &DatabaseConnection,
    r: &FlowNodeRef,
) -> std::result::Result<FlowNodeKind, NodeError> {
    let flow = flows::Entity::find_by_id(r.flow_id)
        .filter(flows::Column::OrgId.eq(r.org_id))
        .filter(flows::Column::Status.eq(pnex_core::FLOW_STATUS_DEPLOYED))
        .one(db)
        .await?
        .ok_or(NodeError::NotDeployed)?;
    let version =
        flow_versions::Entity::find_by_id(flow.deployed_version_id.ok_or(NodeError::NotDeployed)?)
            .one(db)
            .await?
            .filter(|v| v.version_number == r.version && v.flow_id == flow.id)
            .ok_or(NodeError::NotDeployed)?;
    let graph: FlowGraph =
        serde_json::from_value(version.graph).map_err(|_| NodeError::NotDeployed)?;
    graph
        .nodes
        .into_iter()
        .find(|n| n.id == r.node_id)
        .map(|n| n.kind)
        .ok_or(NodeError::NotDeployed)
}

fn actor(r: &FlowNodeRef) -> Actor {
    Actor {
        user_id: None,
        role: OrgMemberRole::Member,
        source_ref: format!("flow:{}@{}", r.flow_id, r.version),
    }
}

/// Per-org writes of the current minute (this process).
// shortcut: counted per pod, not cluster-wide; move to Valkey (rate_limit)
// if flows of one org ever run on several pods at once.
static BUDGET: Mutex<Option<HashMap<i64, (i64, u32)>>> = Mutex::new(None);

fn spend(org_id: i64) -> std::result::Result<(), NodeError> {
    let minute = chrono::Utc::now().timestamp() / 60;
    let mut guard = BUDGET.lock().unwrap_or_else(|p| p.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    let slot = map.entry(org_id).or_insert((minute, 0));
    if slot.0 != minute {
        *slot = (minute, 0);
    }
    if slot.1 >= WRITES_PER_MINUTE {
        return Err(NodeError::Budget);
    }
    slot.1 += 1;
    Ok(())
}

/// `ontology_query`: the node's filters with the values the runtime
/// resolved (`$today`, `$msg.…`), positionally.
pub async fn query(
    db: &DatabaseConnection,
    req: &FlowQueryRequest,
) -> std::result::Result<QueryResult, NodeError> {
    let FlowNodeKind::OntologyQuery { config } = deployed(db, &req.node).await? else {
        return Err(NodeError::NotDeployed);
    };
    let filters: Vec<PropertyFilter> = config
        .filters
        .iter()
        .enumerate()
        .map(|(i, f)| PropertyFilter {
            value: req
                .values
                .get(i)
                .cloned()
                .unwrap_or_else(|| f.value.clone()),
            ..f.clone()
        })
        .collect();
    let q = OntologyQuery {
        type_key: Some(config.object_type.clone()),
        filters,
        order_by: config.order_by.clone(),
        descending: config.descending,
        limit: Some(config.limit.unwrap_or(100)),
        ..Default::default()
    };
    Ok(query::run(db, None, req.node.org_id, &q).await?)
}

/// `object_upsert`: on the natural key (or the id), created when absent.
pub async fn upsert(
    db: &DatabaseConnection,
    req: &FlowUpsertRequest,
) -> std::result::Result<FlowUpsertAck, NodeError> {
    let FlowNodeKind::ObjectUpsert { config } = deployed(db, &req.node).await? else {
        return Err(NodeError::NotDeployed);
    };
    let org = req.node.org_id;
    let s = schema(db, org).await?;
    let def = s
        .object_type(&config.object_type)
        .ok_or_else(|| invalid("object_type", pnex_core::err_codes::FIELD_INVALID))?;
    // No write on a native system type (device, flow, media…).
    if def.native() {
        return Err(OntologyError::SystemReadOnly.into());
    }
    spend(org)?;
    let existing = if config.key == "id" {
        let id = req
            .object_id
            .as_deref()
            .and_then(|i| Uuid::parse_str(i).ok())
            .ok_or_else(|| invalid("object_id", pnex_core::err_codes::FIELD_INVALID_UUID))?;
        let o = objects::find(db, org, id).await?;
        if o.type_key != config.object_type || o.valid_to.is_some() {
            return Err(OntologyError::NotFound.into());
        }
        Some(o)
    } else {
        let v = req
            .properties
            .get(&config.key)
            .ok_or_else(|| invalid(config.key.clone(), pnex_core::err_codes::FIELD_REQUIRED))?;
        objects::find_by_key(db, org, &config.object_type, &config.key, v).await?
    };
    let actor = actor(&req.node);
    match existing {
        // Left as is (a reviewed finding is never reset by a rule).
        Some(o) if config.skip_existing => Ok(FlowUpsertAck {
            created: false,
            object: objects::view(&o, &s),
        }),
        Some(o) => {
            let mut props = o.properties.as_object().cloned().unwrap_or_default();
            props.extend(req.properties.clone());
            let object = objects::update(
                db,
                org,
                &actor,
                o.id,
                &ObjectInput {
                    type_key: o.type_key.clone(),
                    title: req.title.clone().unwrap_or(o.title.clone()),
                    properties: props,
                    expected_version: Some(req.expected_version.unwrap_or(o.version)),
                },
            )
            .await?;
            Ok(FlowUpsertAck {
                created: false,
                object,
            })
        }
        None => {
            let title =
                req.title
                    .clone()
                    .unwrap_or_else(|| match req.properties.get(&config.key) {
                        Some(Value::String(s)) => s.clone(),
                        Some(v) => v.to_string(),
                        None => config.object_type.clone(),
                    });
            let object = objects::create(
                db,
                org,
                &actor,
                &ObjectInput {
                    type_key: config.object_type.clone(),
                    title,
                    properties: req.properties.clone(),
                    expected_version: None,
                },
            )
            .await?;
            Ok(FlowUpsertAck {
                created: true,
                object,
            })
        }
    }
}

/// `link_upsert`: open (or update the attributes of the open link), or
/// close it after merging the message's attributes (a reason, who
/// decided). With `skip_closed`, a pair closed before answers its last
/// closed link instead of opening a new one (web-presence.md D362).
pub async fn link(
    db: &DatabaseConnection,
    req: &FlowLinkRequest,
) -> std::result::Result<Value, NodeError> {
    let FlowNodeKind::LinkUpsert { config } = deployed(db, &req.node).await? else {
        return Err(NodeError::NotDeployed);
    };
    let org = req.node.org_id;
    spend(org)?;
    let actor = actor(&req.node);
    let parse = |f: &str, raw: &str| {
        Uuid::parse_str(raw.trim())
            .map_err(|_| invalid(f, pnex_core::err_codes::FIELD_INVALID_UUID))
    };
    let (src, tgt) = (
        parse("source_id", &req.source_id)?,
        parse("target_id", &req.target_id)?,
    );
    let open = resource_edges::Entity::find()
        .filter(resource_edges::Column::OrgId.eq(org))
        .filter(resource_edges::Column::Relation.eq(&config.link_type))
        .filter(resource_edges::Column::SourceObjectId.eq(src))
        .filter(resource_edges::Column::TargetObjectId.eq(tgt))
        .filter(resource_edges::Column::ValidTo.is_null())
        .one(db)
        .await?;
    if open.is_none() && config.skip_closed && !config.close {
        let closed = resource_edges::Entity::find()
            .filter(resource_edges::Column::OrgId.eq(org))
            .filter(resource_edges::Column::Relation.eq(&config.link_type))
            .filter(resource_edges::Column::SourceObjectId.eq(src))
            .filter(resource_edges::Column::TargetObjectId.eq(tgt))
            .order_by_desc(resource_edges::Column::ValidTo)
            .one(db)
            .await?;
        if let Some(e) = closed {
            return Ok(view(links::views(db, org, &[e]).await?.remove(0)));
        }
    }
    // D375: a new link is proposed, never written by the flow itself.
    if open.is_none() && config.propose && !config.close {
        let input = pnex_core::ontology::proposal::ProposalInput {
            kind: Some(pnex_core::ontology::proposal::ProposalKind::LinkCreate),
            type_key: config.link_type.clone(),
            source_id: Some(src.to_string()),
            link_target_id: Some(tgt.to_string()),
            values: req.attributes.clone(),
            origin: actor.source_ref.clone(),
            confidence: req.confidence.map(|c| c.clamp(0.0, 100.0)),
            evidence: req.evidence.clone(),
            ..Default::default()
        };
        let id = super::proposals::propose(db, org, &input).await?;
        return Ok(serde_json::json!({ "proposed": true, "proposal_id": id }));
    }
    Ok(view(match (open, config.close) {
        (Some(e), true) => {
            // shortcut: two writes, not one transaction (each journals on its
            // own); a failure between them leaves the link open with its new
            // attributes, and the next message closes it.
            if !req.attributes.is_empty() {
                let mut attrs = e.attributes.as_object().cloned().unwrap_or_default();
                attrs.extend(req.attributes.clone());
                links::update(db, org, &actor, e.id, &attrs).await?;
            }
            links::close(db, org, &actor, e.id, req.valid_from.as_deref()).await?
        }
        (None, true) => return Err(OntologyError::NotFound.into()),
        (Some(e), false) => {
            let mut attrs = e.attributes.as_object().cloned().unwrap_or_default();
            attrs.extend(req.attributes.clone());
            links::update(db, org, &actor, e.id, &attrs).await?
        }
        (None, false) => {
            links::create(
                db,
                org,
                &actor,
                &LinkInput {
                    link_type: config.link_type.clone(),
                    source_id: src.to_string(),
                    target_id: tgt.to_string(),
                    attributes: req.attributes.clone(),
                    valid_from: req.valid_from.clone(),
                    valid_to: None,
                },
            )
            .await?
        }
    }))
}

fn view(l: pnex_core::ontology::api::LinkView) -> Value {
    serde_json::to_value(l).unwrap_or(Value::Null)
}

/// What a node needs in the org schema.
enum Need {
    Type(String),
    Writable { object_type: String, key: String },
    Link(String),
}

impl Need {
    fn met(&self, s: &super::OrgSchema) -> bool {
        match self {
            Self::Type(t) => s.knows(t),
            Self::Writable { object_type, key } => s.object_type(object_type).is_some_and(|t| {
                !t.native()
                    && (key == "id" || t.properties.iter().any(|p| &p.key == key && p.unique))
            }),
            Self::Link(l) => s.link_type(l).is_some(),
        }
    }
}

/// Deploy gate: the ontology nodes name a type (with its unique key) or a
/// link type of the org schema.
pub async fn deploy_violations(
    db: &DatabaseConnection,
    org_id: i64,
    g: &FlowGraph,
) -> Result<Vec<pnex_core::FlowViolation>> {
    let mut out = Vec::new();
    let mut needs = Vec::new();
    for n in &g.nodes {
        let (check, need) = match &n.kind {
            FlowNodeKind::OntologyQuery { config } => (
                config.check_deployable(),
                Need::Type(config.object_type.clone()),
            ),
            FlowNodeKind::ObjectUpsert { config } => (
                config.check_deployable(),
                Need::Writable {
                    object_type: config.object_type.clone(),
                    key: config.key.clone(),
                },
            ),
            FlowNodeKind::LinkUpsert { config } => (
                config.check_deployable(),
                Need::Link(config.link_type.clone()),
            ),
            _ => continue,
        };
        match check {
            Some((code, message)) => out.push(pnex_core::FlowViolation::new(
                Some(n.id.as_str()),
                code,
                message,
            )),
            None => needs.push((n.id.clone(), need)),
        }
    }
    if needs.is_empty() {
        return Ok(out);
    }
    let s = schema(db, org_id).await?;
    for (id, need) in needs {
        if !need.met(&s) {
            out.push(pnex_core::FlowViolation::new(
                Some(id.as_str()),
                pnex_core::err_codes::ONTOLOGY_NODE_UNKNOWN,
                "the node names a type, key or link type absent from the organization ontology",
            ));
        }
    }
    Ok(out)
}
