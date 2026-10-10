//! Ontology tools of the assistant (ontology.md D189, extension rule
//! ai-assistant.md §9.3): reads through the query API, writes through the
//! services the UI controller uses (same validations, same 409 on a stale
//! `expected_version`). The schema the model reasons on is generated from
//! the org's registry, never written by hand.
//!
//! Guard rails: org from the principal (R1); `can_write` plus the type's
//! write role for objects and links, org admin for the schema (R2, D188);
//! no delete — an object is never archived by the assistant and a link is
//! only closed, which keeps it as history; no physical action (binding a
//! sensor to a property moves no actuator).

use pnex_core::ontology::api::{LinkInput, ObjectInput, ObjectTypeInput, OntologyQuery};
use pnex_core::ontology::{ObjectTypeDef, WriteRole};
use serde_json::{json, Value};
use uuid::Uuid;

use super::tools::{internal, require_write, ToolDeps, ToolError, ToolOutcome};
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
    require_write(deps)?;
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
        OntologyError::TypeInUse => coded(c::ONTOLOGY_TYPE_IN_USE, "the type is still in use".into(), None),
        OntologyError::WriteForbidden => coded(c::ONTOLOGY_WRITE_FORBIDDEN, "the user's role cannot write objects of this type".into(), None),
        OntologyError::SystemReadOnly => coded(
            c::ONTOLOGY_SYSTEM_READ_ONLY,
            "system objects (devices, media, flows…) are created and deleted from their own pages".into(),
            None,
        ),
        OntologyError::LinkNotAllowed => coded(c::ONTOLOGY_LINK_NOT_ALLOWED, "this link type does not connect objects of these types — check describe_ontology".into(), None),
        OntologyError::LinkCardinality => coded(
            c::ONTOLOGY_LINK_CARDINALITY,
            "an open link already takes this place: close it first (close_link), then open the new one".into(),
            None,
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

/// D185 query, capped for the model.
pub async fn query_ontology(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let mut q: OntologyQuery =
        serde_json::from_value(args.get("query").cloned().unwrap_or(json!({})))
            .map_err(|e| ToolError::from(format!("invalid query: {e}")))?;
    q.limit = Some(q.limit.unwrap_or(20).min(QUERY_CAP));
    let r = query::run(deps.db, deps.o2, deps.org_id, &q)
        .await
        .map_err(onto_error)?;
    outcome(json!({ "count": r.count, "rows": r.rows }))
}

/// One object with its links valid now.
pub async fn get_object(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let id = uuid_arg(args, "object_id")?;
    let o = objects::get(deps.db, deps.org_id, id)
        .await
        .map_err(onto_error)?;
    let l = links::for_object(deps.db, deps.org_id, id, None, false)
        .await
        .map_err(onto_error)?;
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
