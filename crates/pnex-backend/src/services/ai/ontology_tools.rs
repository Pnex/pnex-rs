//! Ontology tools of the assistant (ontology.md D189, extension rule
//! ai-assistant.md §9.3): reads through the query API, writes through the
//! services the UI controller uses (same validations, same 409 on a stale
//! `expected_version`). The schema the model reasons on is generated from
//! the org's registry, never written by hand.
//!
//! Guard rails: org from the principal (R1); the tool's IAM permission plus the type's
//! write role for objects and links, org admin for the schema (R2, D188);
//! no delete — an object is never archived by the assistant and a link is
//! only closed, which keeps it as history; no physical action (binding a
//! sensor to a property moves no actuator).

use pnex_core::ontology::api::{
    LinkInput, ObjectInput, ObjectTypeInput, OntologyQuery, SignalsQuery,
};
use pnex_core::ontology::{ObjectTypeDef, WriteRole};
use serde_json::{json, Value};
use uuid::Uuid;

use super::tools::{internal, ToolDeps, ToolError, ToolOutcome};
use crate::services::ontology::{self as onto, links, objects, query, types, Actor, OntologyError};

/// Rows a query returns to the model at most.
const QUERY_CAP: u64 = 50;

fn outcome(value: Value) -> Result<ToolOutcome, ToolError> {
    Ok(ToolOutcome {
        value,
        flow_id: None,
    })
}

fn uuid_arg(args: &Value, key: &str) -> Result<Uuid, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| format!("argument '{key}' missing or not a UUID").into())
}

fn actor(deps: &ToolDeps<'_>) -> Result<Actor, ToolError> {
    deps.actor
        .clone()
        .ok_or_else(|| "this tool needs a signed-in member".into())
}

/// Service refusal → coded tool refusal (the UI renders `err-<code>`).
fn onto_error(e: OntologyError) -> ToolError {
    use pnex_core::err_codes as c;
    let coded = |code: &'static str, message: String, args: Option<Value>| ToolError {
        message,
        code: Some(code),
        args,
    };
    match e {
        OntologyError::Invalid { field, token } => {
            format!("invalid {field}: {token} — fix this field and retry").into()
        }
        OntologyError::NotFound => coded(c::ONTOLOGY_NOT_FOUND, "not found in this organization's ontology".into(), None),
        OntologyError::KeyTaken => coded(c::ONTOLOGY_ALREADY_EXISTS, "it already exists (same key, or the same link is already open)".into(), None),
        OntologyError::VersionConflict { current } => coded(
            c::ONTOLOGY_VERSION_CONFLICT,
            format!("version conflict: it is now at version {current} — read it again and redo the change"),
            Some(json!({ "current": current.to_string() })),
        ),
        OntologyError::AggregateHidden { property } => coded(
            c::ACCESS_DENIED,
            "this aggregate names what the user may not read".into(),
            Some(json!({ "property": property })),
        ),
        OntologyError::AggregateTimeout => coded(
            c::ONTOLOGY_AGGREGATE_TIMEOUT,
            "this aggregate took too long: narrow it".into(),
            None,
        ),
        OntologyError::PathIncomplete => coded(
            c::ONTOLOGY_PATH_INCOMPLETE,
            "the path search stopped before finding a path: restrict the link types".into(),
            None,
        ),
        OntologyError::TypeInUse => coded(c::ONTOLOGY_TYPE_IN_USE, "the type is still in use".into(), None),
        OntologyError::WriteForbidden => coded(c::ONTOLOGY_WRITE_FORBIDDEN, "the user's role cannot write objects of this type".into(), None),
        OntologyError::SystemReadOnly => coded(
            c::ONTOLOGY_SYSTEM_READ_ONLY,
            "system objects (devices, media, flows…) are created and deleted from their own pages".into(),
            None,
        ),
        OntologyError::LinkNotAllowed => coded(c::ONTOLOGY_LINK_NOT_ALLOWED, "this link type does not connect objects of these types — check describe_ontology".into(), None),
        OntologyError::Erased => coded(
            c::ONTOLOGY_ERASED,
            "this object was erased (personal data): never edit nor link it, tell the user".into(),
            None,
        ),
        OntologyError::Opposed => coded(
            c::PROSPECTING_OPPOSED,
            "this person objected to the processing of their data: never link them again, tell the user".into(),
            None,
        ),
        OntologyError::LinkCardinality => coded(
            c::ONTOLOGY_LINK_CARDINALITY,
            "an open link already takes this place: close it first (close_link), then open the new one".into(),
            None,
        ),
        OntologyError::ClaimUnsourced => coded(
            c::ONTOLOGY_CLAIM_UNSOURCED,
            "a claim becomes corroborated only with two pieces of distinct origins linked by sourced_by; never change a claim's status yourself, the user does".into(),
            None,
        ),
        OntologyError::CaseLegalBasisRequired => coded(
            c::CASE_LEGAL_BASIS_REQUIRED,
            "a person can be attached to this case only once the user has filled in its processing basis".into(),
            None,
        ),
        OntologyError::LinkMemberOnly => coded(
            c::ONTOLOGY_LINK_MEMBER_ONLY,
            "only the user writes this link, from a case (such as a market move attributed to a cause); you may only say an event is in the same window, never that it caused a move".into(),
            None,
        ),
        OntologyError::ProposalDecided => coded(
            c::PROPOSAL_DECIDED,
            "this proposal is already decided; only a person decides proposals".into(),
            None,
        ),
        OntologyError::CaseProposalStale => coded(
            c::CASE_PROPOSAL_STALE,
            "this proposal was already reviewed by the user; only the user retains or rejects proposals".into(),
            None,
        ),
        OntologyError::TrackImport(code) => coded(
            code,
            "track files are imported by the user from the case workspace".into(),
            None,
        ),
        // The assistant never invokes an action (D183); only kept exhaustive.
        OntologyError::ActionNoFlow => coded(c::ONTOLOGY_ACTION_NO_FLOW, "no deployed flow handles this action".into(), None),
        OntologyError::ActionUnavailable => coded(c::ONTOLOGY_ACTION_UNAVAILABLE, "the action bus is unavailable".into(), None),
        OntologyError::UniqueDuplicates { property, values } => coded(
            c::ONTOLOGY_UNIQUE_DUPLICATES,
            "live objects already share values of this property: the user fixes them before it can be unique".into(),
            Some(json!({ "property": property, "values": values.join(", ") })),
        ),
        OntologyError::Import { code, args } => coded(
            code,
            "the import was refused; imports are applied by the user from the Imports page".into(),
            args,
        ),
        OntologyError::Db(e) => internal("using the ontology")(e),
    }
}

/// The org schema: types (properties, containment, write role, object
/// count) and link types — the assistant's knowledge of the ontology.
pub async fn describe_ontology(deps: &ToolDeps<'_>) -> Result<ToolOutcome, ToolError> {
    let t = types::list(deps.db, deps.org_id)
        .await
        .map_err(onto_error)?;
    let l = onto::link_types::list(deps.db, deps.org_id)
        .await
        .map_err(onto_error)?;
    outcome(json!({
        "types": t.iter().map(|t| json!({
            "key": t.def.key,
            "name": t.def.name,
            "system": t.def.system,
            "version": t.version,
            "object_count": t.object_count,
            "write_role": t.def.write_role,
            "may_contain": t.def.may_contain,
            "may_be_contained_in": t.def.may_be_contained_in,
            "properties": t.def.properties,
        })).collect::<Vec<_>>(),
        "link_types": l.iter().map(|l| &l.def).collect::<Vec<_>>(),
    }))
}

/// What the member may not read (crm.md D443), with the schema it needs.
/// The member's mask: classification (D443) and marked objects (governance.md D801).
/// Without a known member every marked object stays hidden.
async fn redaction(deps: &ToolDeps<'_>) -> Result<onto::redact::Redaction, ToolError> {
    let user = deps.actor.as_ref().and_then(|a| a.user_id).unwrap_or(-1);
    onto::redact::Redaction::of(|p| deps.grants.can(p))
        .with_markings(deps.db, deps.org_id, user)
        .await
        .map_err(|e| ToolError::from(e.to_string()))
}

async fn masking(
    deps: &ToolDeps<'_>,
) -> Result<Option<(onto::redact::Redaction, onto::OrgSchema)>, ToolError> {
    let m = redaction(deps).await?;
    if m.is_none() {
        return Ok(None);
    }
    let s = onto::schema(deps.db, deps.org_id)
        .await
        .map_err(|e| ToolError::from(e.to_string()))?;
    Ok(Some((m, s)))
}

/// A case read masked for the member (D443) before it is reshaped for the
/// model: through its JSON, every object and link it names.
async fn masked_as<T: serde::Serialize + serde::de::DeserializeOwned>(
    deps: &ToolDeps<'_>,
    v: T,
) -> Result<T, ToolError> {
    let Some((m, s)) = masking(deps).await? else {
        return Ok(v);
    };
    let mut j = serde_json::to_value(&v).map_err(|e| ToolError::from(e.to_string()))?;
    m.json(&s, &mut j);
    serde_json::from_value(j).map_err(|e| ToolError::from(e.to_string()))
}

/// D185 query, capped for the model.
pub async fn query_ontology(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let mut q: OntologyQuery =
        serde_json::from_value(args.get("query").cloned().unwrap_or(json!({})))
            .map_err(|e| ToolError::from(format!("invalid query: {e}")))?;
    q.limit = Some(q.limit.unwrap_or(20).min(QUERY_CAP));
    let hide = redaction(deps).await?;
    let mut r = query::run(deps.db, deps.o2, deps.org_id, &q, hide)
        .await
        .map_err(onto_error)?;
    if let Some((m, s)) = masking(deps).await? {
        m.query(&s, &mut r);
    }
    outcome(json!({ "count": r.count, "rows": r.rows }))
}

/// Numbers over the objects of a query (object-aggregates.md D587): the
/// route's service, rights and refusals.
pub async fn aggregate_objects(
    deps: &ToolDeps<'_>,
    args: &Value,
) -> Result<ToolOutcome, ToolError> {
    let req: pnex_core::ontology::aggregate::AggregateQuery = serde_json::from_value(args.clone())
        .map_err(|e| ToolError::from(format!("invalid aggregate: {e}")))?;
    let hide = redaction(deps).await?;
    let r = onto::aggregate::run(deps.db, deps.org_id, &req, hide)
        .await
        .map_err(onto_error)?;
    outcome(serde_json::to_value(r).unwrap_or_default())
}

/// Signals of a case handed to the model (newest kept).
const SIGNALS_CAP: usize = 200;

/// A case as the workspace shows it (cases-workspace.md K22): its events
/// oldest first with what each involves and the role, and its undated
/// pieces. Read-only: the assistant never attaches nor detaches.
pub async fn case_workspace(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let id = uuid_arg(args, "case_id")?;
    let w = onto::cases::workspace(deps.db, deps.org_id, id)
        .await
        .map_err(onto_error)?;
    let w = masked_as(deps, w).await?;
    let events: Vec<Value> = w
        .events
        .iter()
        .map(|e| {
            json!({
                "event_id": e.event.id, "title": e.event.title,
                "occurred": e.event.properties.get("occurred"),
                "origin": e.event.properties.get("origin"),
                "note": e.event.properties.get("note"),
                "attachments": e.attachments.iter().map(|a| json!({
                    "object_id": a.object.id, "title": a.object.title, "type": a.object.type_key,
                    "role": a.role, "detail": a.detail,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let undated: Vec<Value> = w
        .undated
        .iter()
        .map(|o| json!({"object_id": o.id, "title": o.title, "type": o.type_key}))
        .collect();
    outcome(json!({
        "case": {"id": w.case.id, "title": w.case.title, "properties": w.case.properties},
        "events": events, "undated": undated,
    }))
}

/// Raw signals around the case members (K30), compacted for the model: one
/// line per item, the source objects by title and id. Read-only.
pub async fn case_signals(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let id = uuid_arg(args, "case_id")?;
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    let q = SignalsQuery {
        from: s("from"),
        to: s("to"),
    };
    let sig = onto::cases::signals(deps.db, deps.o2, deps.org_id, id, &q)
        .await
        .map_err(onto_error)?;
    let sig = masked_as(deps, sig).await?;
    let total = sig.items.len();
    let items: Vec<Value> = sig
        .items
        .iter()
        .skip(total.saturating_sub(SIGNALS_CAP))
        .map(|i| {
            let what = match (&i.link, &i.change, &i.range, &i.notification, &i.fuzzy) {
                (Some(l), ..) => json!({
                    "link": l.link_type, "source": l.source.title, "target": l.target.title,
                    "source_id": l.source.id, "target_id": l.target.id, "attributes": l.attributes,
                }),
                (_, Some(c), ..) => json!({"action": c.action, "by": c.source_ref}),
                (_, _, Some(r), ..) => json!({
                    "range": r.label,
                    "end": r.actual_end.clone().or_else(|| r.planned_end.clone()),
                }),
                (_, _, _, Some(n), _) => json!({
                    "notification": n.subject, "status": n.status, "channel": n.channel_kind,
                }),
                (.., Some(f)) => json!({"title": i.title, "occurred": f}),
                _ => Value::Null,
            };
            json!({"at": i.at, "kind": i.kind, "lane": i.lane, "what": what})
        })
        .collect();
    outcome(json!({
        "from": sig.from, "to": sig.to, "items": items, "items_total": total,
        "changes_available": sig.changes_available,
        "truncated": sig.truncated || total > SIGNALS_CAP,
    }))
}

/// The hypotheses board of a case (cases.md K8), as the UI shows it.
pub async fn case_board(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let id = uuid_arg(args, "case_id")?;
    let b = onto::cases::board(deps.db, deps.org_id, id)
        .await
        .map_err(onto_error)?;
    let b = masked_as(deps, b).await?;
    outcome(serde_json::to_value(b).map_err(|e| ToolError::from(e.to_string()))?)
}

/// The detections proposed to a case and how they were reviewed (K30).
pub async fn case_proposals(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let id = uuid_arg(args, "case_id")?;
    let p = onto::cases::proposals(deps.db, deps.org_id, id)
        .await
        .map_err(onto_error)?;
    let p = masked_as(deps, p).await?;
    outcome(json!({ "proposals": p }))
}

/// One object with its links valid now.
pub async fn get_object(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let id = uuid_arg(args, "object_id")?;
    let as_of = match args.get("as_of").and_then(Value::as_str) {
        Some(t) => Some(
            chrono::DateTime::parse_from_rfc3339(t)
                .map_err(|_| ToolError::from("as_of must be an RFC 3339 instant"))?,
        ),
        None => None,
    };
    let mut o = match as_of {
        Some(at) => objects::get_as_of(deps.db, deps.org_id, id, at).await,
        None => objects::get(deps.db, deps.org_id, id).await,
    }
    .map_err(onto_error)?;
    let mut l = links::for_object(deps.db, deps.org_id, id, as_of, None, false)
        .await
        .map_err(onto_error)?;
    if let Some((m, s)) = masking(deps).await? {
        m.object(&s, &mut o);
        l.iter_mut().for_each(|x| m.link(&s, x));
    }
    outcome(json!({ "object": o, "links": l }))
}

pub async fn create_object(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let a = actor(deps)?;
    let input: ObjectInput = serde_json::from_value(args.clone())
        .map_err(|e| ToolError::from(format!("invalid arguments: {e}")))?;
    let o = objects::create(deps.db, deps.org_id, &a, &input)
        .await
        .map_err(onto_error)?;
    outcome(
        json!({ "object_id": o.id, "title": o.title, "type_key": o.type_key, "version": o.version }),
    )
}

/// Checks an action invocation and hands it to the user (D183): never
/// dispatched here, the UI shows a button that opens the invocation dialog
/// pre-filled with these arguments (D123).
pub async fn propose_action(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let id = uuid_arg(args, "object_id")?;
    let key = args
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::from("action is required"))?;
    let params = args
        .get("params")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let c = onto::actions::check(deps.db, deps.org_id, id, key, &params)
        .await
        .map_err(onto_error)?;
    let flows = onto::actions::listeners(deps.db, deps.org_id, &c.object.type_key, key)
        .await
        .map_err(onto_error)?;
    outcome(json!({
        "object_id": id,
        "title": c.object.title,
        "action": key,
        "action_name": c.action.name,
        "params": c.params,
        "flows": flows,
        "executed": false,
        "next": if flows.is_empty() {
            "no deployed flow handles this action: the user must deploy a flow with an action_trigger node for it before running it"
        } else {
            "the user reviews the parameters and runs the action from the button under your answer"
        },
    }))
}

/// Writes the COMPLETE property document on top of `expected_version`.
pub async fn update_object(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let a = actor(deps)?;
    let id = uuid_arg(args, "object_id")?;
    let input: ObjectInput = serde_json::from_value(args.clone())
        .map_err(|e| ToolError::from(format!("invalid arguments: {e}")))?;
    let o = objects::update(deps.db, deps.org_id, &a, id, &input)
        .await
        .map_err(onto_error)?;
    outcome(
        json!({ "object_id": o.id, "title": o.title, "type_key": o.type_key, "version": o.version }),
    )
}

pub async fn open_link(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let a = actor(deps)?;
    let input: LinkInput = serde_json::from_value(args.clone())
        .map_err(|e| ToolError::from(format!("invalid arguments: {e}")))?;
    // D742: a member's judgement, never the assistant's.
    let s = crate::services::ontology::schema(deps.db, deps.org_id)
        .await
        .map_err(|e| onto_error(e.into()))?;
    if s.link_type(&input.link_type).is_some_and(|l| l.member_only) {
        return Err(onto_error(OntologyError::LinkMemberOnly));
    }
    let l = links::create(deps.db, deps.org_id, &a, &input)
        .await
        .map_err(onto_error)?;
    outcome(
        json!({ "link_id": l.id, "link_type": l.link_type, "source": l.source.title, "target": l.target.title }),
    )
}

/// Closes an open link (kept as history, never deleted).
pub async fn close_link(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let a = actor(deps)?;
    let id = args
        .get("link_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| ToolError::from("argument 'link_id' missing (integer)"))?;
    let valid_to = args.get("valid_to").and_then(Value::as_str);
    let l = links::close(deps.db, deps.org_id, &a, id, valid_to)
        .await
        .map_err(onto_error)?;
    outcome(
        json!({ "link_id": l.id, "link_type": l.link_type, "source": l.source.title, "target": l.target.title, "valid_to": l.valid_to }),
    )
}

/// Creates an org type (`expected_version` absent) or appends a version;
/// for a system type only the extra properties are taken. Org admin only.
pub async fn save_object_type(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let a = actor(deps)?;
    if !onto::role_allows(&a.role, WriteRole::Admin) {
        return Err(onto_error(OntologyError::WriteForbidden));
    }
    let def: ObjectTypeDef =
        serde_json::from_value(args.get("def").cloned().unwrap_or(Value::Null))
            .map_err(|e| ToolError::from(format!("invalid def: {e}")))?;
    let expected = args
        .get("expected_version")
        .and_then(Value::as_i64)
        .map(|v| v as i32);
    let user = a.user_id;
    let view = match expected {
        None => types::create(deps.db, deps.org_id, user, &def, None).await,
        Some(v) => {
            let key = def.key.clone();
            types::update(
                deps.db,
                deps.org_id,
                &key,
                user,
                &ObjectTypeInput {
                    def,
                    expected_version: Some(v),
                },
            )
            .await
        }
    }
    .map_err(onto_error)?;
    outcome(
        json!({ "type_key": view.def.key, "version": view.version, "property_count": view.def.properties.len() }),
    )
}

// ─────────────────────────── Registry entries ───────────────────────────

use super::provider::ToolSpec;
use super::tools::{count, shown, trace, ToolDef, TraceSummary};

/// The ontology tools: spec and trace summary of each, in one place.
pub(super) fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            spec: ToolSpec {
                name: "describe_ontology",
                description: "The organization's ontology schema: object types (key, name, typed properties, containment, write role, object count; system types are devices, media, dashboards, tours, POIs, flows, folders) and link types (key, from/to types, attributes, cardinality). Read it before any ontology query or write. Read-only.",
                input_schema: json!({"type": "object", "properties": {}}),
            },
            summary: |v| {
            let n = count(&v["types"]);
            trace(
                "ai-trace-ontology-schema",
                json!({"count": n}),
                format!("{n} object type(s)"),
            )
        },
        },
        ToolDef {
            spec: ToolSpec {
                name: "query_ontology",
                description: "Declarative ontology query: start set (type_key, ids, text in titles, property filters {key, op: eq|ne|lt|lte|gt|gte|contains|exists, value}, label 'name' or 'name:value', within = descendants of an object) then up to 4 traversals [{link_type, direction: out|in}], evaluated as_of an RFC 3339 instant (default now); latest = series properties whose last value is joined. At most 50 rows. Read-only.",
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"query":{"type":"object","properties":{"type_key":{"type":"string"},"ids":{"type":"array","items":{"type":"string"}},"text":{"type":"string"},"filters":{"type":"array","items":{"type":"object","properties":{"key":{"type":"string"},"op":{"type":"string"},"value":{}},"required":["key","op"]}},"label":{"type":"string"},"within":{"type":"string"},"traverse":{"type":"array","items":{"type":"object","properties":{"link_type":{"type":"string"},"direction":{"type":"string"}},"required":["link_type"]}},"as_of":{"type":"string"},"latest":{"type":"array","items":{"type":"string"}},"limit":{"type":"integer"}}}},"required":["query"]}"#).expect("static schema"),
            },
            summary: |v| {
            let n = shown(&v["count"]);
            trace(
                "ai-trace-ontology-query",
                json!({"count": n}),
                format!("{n} object(s)"),
            )
        },
        },
        ToolDef {
            spec: ToolSpec {
                name: "aggregate_objects",
                description: "Numbers over the objects of an ontology query, without reading them one by one. metrics: {key, op: count|count_distinct|sum|avg|min|max, property, weight (with sum: a number property as a percentage, an object without it is left out), where: {key, op, value}} or a ratio {key, ratio: [a, b], percent}. group_by, two at most: {property}, {property, bucket: day|week|month|quarter|year} or {hops: [{link_type, direction}]}, two hops at most. Filters accept $today-30 and $today+7. A money property answers by_currency: never add currencies. An object linked to two targets counts in both groups. Refused on what the user may not read.",
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"query":{"type":"object"},"group_by":{"type":"array"},"metrics":{"type":"array"},"limit":{"type":"integer"}},"required":["metrics"]}"#).expect("static schema"),
            },
            summary: |v| {
            let n = v["groups"].as_array().map_or(0, Vec::len).to_string();
            trace(
                "ai-trace-ontology-aggregate",
                json!({"count": n}),
                format!("{n} group(s)"),
            )
        },
        },
        ToolDef {
            spec: ToolSpec {
                name: "get_object",
                description: "One ontology object (properties, version) with its links valid now, or as known at as_of (RFC 3339). Read it before update_object.",
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"object_id":{"type":"string"},"as_of":{"type":"string"}},"required":["object_id"]}"#).expect("static schema"),
            },
            summary: |v| {
            let t = shown(&v["object"]["title"]);
            trace(
                "ai-trace-ontology-object",
                json!({"title": t}),
                format!("object \"{t}\""),
            )
        },
        },
        ToolDef {
            spec: ToolSpec {
                name: "case_workspace",
                description: "An investigation case (object of type case) as its workspace shows it: its events oldest first (title, occurred as fuzzy time, origin, analysis note) with the objects each involves and their role (evidence, actor, context, hypothesis, source), and its undated pieces. Read-only: never attach, detach nor create an event; propose it to the user.",
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"case_id":{"type":"string"}},"required":["case_id"]}"#).expect("static schema"),
            },
            summary: |v| {
            let (e, u) = (count(&v["events"]), count(&v["undated"]));
            trace(
                "ai-trace-case-workspace",
                json!({"events": e, "undated": u}),
                format!("case: {e} event(s), {u} undated piece(s)"),
            )
        },
        },
        ToolDef {
            spec: ToolSpec {
                name: "case_signals",
                description: "Raw signals around the members of an investigation case, oldest first: links opened and closed, object changes, ranges set on members, notifications of member flows, approximate dates — over the span of the case events or from/to (RFC 3339). At most 200 items (the newest). Each may be worth an event: propose it to the user, never create it.",
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"case_id":{"type":"string"},"from":{"type":"string"},"to":{"type":"string"}},"required":["case_id"]}"#).expect("static schema"),
            },
            summary: |v| {
            let n = shown(&v["items_total"]);
            trace(
                "ai-trace-case-signals",
                json!({"count": n}),
                format!("case signals: {n}"),
            )
        },
        },
        ToolDef {
            spec: ToolSpec {
                name: "case_board",
                description: "The hypotheses board of an investigation case: each hypothesis member with the claims that support or contradict it (status, number of sources), the member claims without any source, and those contradicted by another claim. No score: present the balance, the user concludes. Read-only: never change a hypothesis' or claim's status.",
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"case_id":{"type":"string"}},"required":["case_id"]}"#).expect("static schema"),
            },
            summary: |v| {
            let (h, u) = (count(&v["hypotheses"]), count(&v["unsourced"]));
            trace(
                "ai-trace-case-board",
                json!({"hypotheses": h, "unsourced": u}),
                format!("board: {h} hypothesis(es), {u} unsourced claim(s)"),
            )
        },
        },
        ToolDef {
            spec: ToolSpec {
                name: "case_proposals",
                description: "Detections proposed to an investigation case from the members' position tracks (co_presence of two members, stop, signal_loss), each with its span, members, place, thresholds and status (proposed, retained as an event, rejected with an optional reason). A detection is a lead, not a fact. Read-only: only the user retains or rejects a proposal.",
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"case_id":{"type":"string"}},"required":["case_id"]}"#).expect("static schema"),
            },
            summary: |v| {
            let n = count(&v["proposals"]);
            trace(
                "ai-trace-case-proposals",
                json!({"count": n}),
                format!("case proposals: {n}"),
            )
        },
        },
        ToolDef {
            spec: ToolSpec {
                name: "create_object",
                description: "Creates an object of an organization type (not a system type: devices, media… are created from their own pages). properties must follow the type's schema (describe_ontology); series and events properties hold no value. A recurrence value is {start: YYYY-MM-DD, every: 1-999, unit: day|week|month|year, until?: YYYY-MM-DD, floating?: true (count from the last completion), days?: length, lead_days?: days to prepare}: its occurrences appear in the calendar (list_calendar).",
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"type_key":{"type":"string"},"title":{"type":"string"},"properties":{"type":"object"}},"required":["type_key","title"]}"#).expect("static schema"),
            },
            summary: summary_create_object,
        },
        ToolDef {
            spec: ToolSpec {
                name: "update_object",
                description: "Saves the title and the COMPLETE property document of an object (properties left out are cleared) on top of the version read with get_object (expected_version). For a system object only its organization properties change.",
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"object_id":{"type":"string"},"expected_version":{"type":"integer"},"title":{"type":"string"},"properties":{"type":"object"}},"required":["object_id","expected_version","properties"]}"#).expect("static schema"),
            },
            summary: summary_create_object,
        },
        ToolDef {
            spec: ToolSpec {
                name: "open_link",
                description: "Opens a typed link between two objects (link types from describe_ontology), optionally valid_from a past RFC 3339 instant. To bind a sensor to an object's series property: link_type 'measures', source = the device object, target = the object, attributes {metric, property}.",
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"link_type":{"type":"string"},"source_id":{"type":"string"},"target_id":{"type":"string"},"attributes":{"type":"object"},"valid_from":{"type":"string"}},"required":["link_type","source_id","target_id"]}"#).expect("static schema"),
            },
            summary: |v| link_summary("ai-trace-ontology-link-opened", v),
        },
        ToolDef {
            spec: ToolSpec {
                name: "close_link",
                description: "Closes an open link at valid_to (default now). The link stays in the history; nothing is deleted. Replacing a sensor = close its 'measures' link, then open one from the new device.",
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"link_id":{"type":"integer"},"valid_to":{"type":"string"}},"required":["link_id"]}"#).expect("static schema"),
            },
            summary: |v| link_summary("ai-trace-ontology-link-closed", v),
        },
        ToolDef {
            spec: ToolSpec {
                name: "propose_action",
                description: "Proposes to the user an action declared on the object's type (actions in describe_ontology, e.g. start_maintenance on a pump), with its parameters pre-filled. Nothing is executed: the user reviews the proposal and runs it from the button shown under your answer; a deployed flow then acts. Tells which deployed flows would receive it (none = the user must first deploy a flow with an action_trigger node). Never claim the action was done.",
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"object_id":{"type":"string"},"action":{"type":"string"},"params":{"type":"object"}},"required":["object_id","action"]}"#).expect("static schema"),
            },
            summary: |v| {
            let (a, t) = (shown(&v["action_name"]), shown(&v["title"]));
            trace(
                "ai-trace-action-proposed",
                json!({"action": a, "title": t}),
                format!("proposed \"{a}\" on \"{t}\""),
            )
        },
        },
        ToolDef {
            spec: ToolSpec {
                name: "save_object_type",
                description: "Creates an organization object type (no expected_version) or saves a new version of one on top of expected_version (from describe_ontology). def = {key [a-z][a-z0-9_]*, name, icon, properties: [{key, name, kind: text|number|bool|date|date_time|enum|geo_point|url|ref|series|events|fuzzy_time|recurrence, required, indexed, unit, min, max, values, to_type, stream, max_len}], may_contain / may_be_contained_in: '*' or a list of type keys or null, write_role: member|admin|owner}. For a system type only properties are taken. Organization owners and admins only.",
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"def":{"type":"object"},"expected_version":{"type":"integer"}},"required":["def"]}"#).expect("static schema"),
            },
            summary: |v| {
            let (k, ver) = (shown(&v["type_key"]), shown(&v["version"]));
            trace(
                "ai-trace-ontology-type-saved",
                json!({"key": k, "version": ver}),
                format!("type {k} v{ver}"),
            )
        },
        },
    ]
}

/// Trace summary shared by create_object, update_object.
fn summary_create_object(v: &Value) -> TraceSummary {
    {
        let (t, ver) = (shown(&v["title"]), shown(&v["version"]));
        trace(
            "ai-trace-ontology-object-saved",
            json!({"title": t, "version": ver}),
            format!("object \"{t}\" v{ver}"),
        )
    }
}

/// Trace summary of open_link / close_link (`key` tells which).
fn link_summary(key: &'static str, v: &Value) -> TraceSummary {
    let (a, b, k) = (
        shown(&v["source"]),
        shown(&v["target"]),
        shown(&v["link_type"]),
    );
    trace(
        key,
        json!({"source": a, "target": b, "link": k}),
        format!("{a} —{k}→ {b}"),
    )
}
