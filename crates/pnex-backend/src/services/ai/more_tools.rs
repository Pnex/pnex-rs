//! Assistant tools beyond flows and dashboards (extension rule §9.3):
//! notification templates, JS/Starlark functions, and read-only access to
//! annotations, tours and map POIs.
//!
//! Writes go through the services of the UI's HTTP controllers
//! (`services::notify_templates`, `services::functions`), re-check
//! `can_write`, never delete, never send a notification and never touch a
//! channel (channels hold credentials; their test sends a real message).
//! A template or function change never reaches a running flow: deployed
//! flows keep the snapshot / pinned version taken at deploy.

use std::collections::{BTreeMap, HashMap};

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde_json::{json, Value};
use uuid::Uuid;

use super::tools::{ToolDeps, ToolError, ToolOutcome};
use crate::models::_entities::{
    annotation_layers, device_placements, function_versions, functions, map_pins, notify_templates,
    tours,
};
use crate::services::notify_templates::{self as tpl, TemplateInput, TemplateWriteError};

const LIST_CAP: u64 = 100;

fn outcome(value: Value) -> Result<ToolOutcome, ToolError> {
    Ok(ToolOutcome {
        value,
        flow_id: None,
    })
}

fn require_write(deps: &ToolDeps<'_>) -> Result<(), ToolError> {
    if deps.can_write {
        Ok(())
    } else {
        Err(
            "owner, admin or member role required — the assistant cannot change this for you"
                .to_string()
                .into(),
        )
    }
}

fn uuid_arg(args: &Value, key: &str) -> Result<Uuid, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| format!("argument '{key}' missing or not a UUID").into())
}

fn i64_arg(args: &Value, key: &str) -> Result<i64, ToolError> {
    args.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("argument '{key}' missing (integer)").into())
}

fn str_arg(args: &Value, key: &str) -> Result<String, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("argument '{key}' missing (string)").into())
}

// ─────────────────────────── Notification templates ───────────────────────────

async fn find_template(
    deps: &ToolDeps<'_>,
    id: Uuid,
) -> Result<notify_templates::Model, ToolError> {
    notify_templates::Entity::find_by_id(id)
        .filter(notify_templates::Column::OrgId.eq(deps.org_id))
        .one(deps.db)
        .await
        .map_err(|e| format!("reading template: {e}"))?
        .ok_or_else(|| format!("template {id} not found in this organization").into())
}

fn template_json(m: &notify_templates::Model) -> Value {
    json!({
        "id": m.id,
        "name": m.name,
        "subject": m.subject,
        "body": m.body,
        "vars": m.vars,
        "updated_at": m.updated_at.to_rfc3339(),
    })
}

fn template_input(args: &Value) -> Result<TemplateInput, ToolError> {
    serde_json::from_value(json!({
        "name": args.get("name").cloned().unwrap_or(Value::Null),
        "subject": args.get("subject").cloned().unwrap_or(Value::Null),
        "body": args.get("body").cloned().unwrap_or(Value::Null),
        "vars": args.get("vars").cloned().unwrap_or(json!([])),
    }))
    .map_err(|e| format!("invalid template arguments (name, body required): {e}").into())
}

fn template_error(e: TemplateWriteError) -> ToolError {
    match e {
        TemplateWriteError::Field(field, token) => format!("field {field}: {token}"),
        TemplateWriteError::Render(e) => format!("the template does not render: {e}"),
        TemplateWriteError::NameTaken => {
            "another template of the organization has this name".into()
        }
        TemplateWriteError::Db => "database error".into(),
    }
    .into()
}

pub async fn get_notification_template(
    deps: &ToolDeps<'_>,
    args: &Value,
) -> Result<ToolOutcome, ToolError> {
    let m = find_template(deps, uuid_arg(args, "template_id")?).await?;
    outcome(json!({ "template": template_json(&m) }))
}

/// Renders a template with example values — never sends anything.
pub async fn preview_notification_template(
    deps: &ToolDeps<'_>,
    args: &Value,
) -> Result<ToolOutcome, ToolError> {
    let m = find_template(deps, uuid_arg(args, "template_id")?).await?;
    let vars: BTreeMap<String, String> = args
        .get("vars")
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.as_str().map_or_else(|| v.to_string(), str::to_string),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let payload = args.get("payload").cloned().unwrap_or(json!({}));
    let rendered = tpl::preview(&m, vars, payload)
        .map_err(|e| format!("the template does not render: {e}"))?;
    outcome(json!({ "subject": rendered.subject, "body": rendered.body, "sent": false }))
}

pub async fn create_notification_template(
    deps: &ToolDeps<'_>,
    args: &Value,
) -> Result<ToolOutcome, ToolError> {
    require_write(deps)?;
    let input = template_input(args)?;
    let m = tpl::create(deps.db, deps.org_id, &input)
        .await
        .map_err(template_error)?;
    outcome(json!({ "template_id": m.id, "name": m.name, "updated_at": m.updated_at.to_rfc3339() }))
}

/// In-place update guarded by `expected_updated_at` (read with
/// get_notification_template): an edit made in between is not overwritten.
pub async fn update_notification_template(
    deps: &ToolDeps<'_>,
    args: &Value,
) -> Result<ToolOutcome, ToolError> {
    require_write(deps)?;
    let m = find_template(deps, uuid_arg(args, "template_id")?).await?;
    let expected = str_arg(args, "expected_updated_at")?;
    let current = m.updated_at.to_rfc3339();
    let same = chrono::DateTime::parse_from_rfc3339(&expected).is_ok_and(|t| t == m.updated_at);
    if !same {
        return Err(format!(
            "conflict: the template changed since you read it (now updated_at {current}) — reload it with get_notification_template and redo the change"
        )
        .into());
    }
    let input = template_input(args)?;
    let m = tpl::update(deps.db, m, &input)
        .await
        .map_err(template_error)?;
    outcome(json!({ "template_id": m.id, "name": m.name, "updated_at": m.updated_at.to_rfc3339() }))
}

// ─────────────────────────── Functions (JS / Starlark) ───────────────────────────

async fn find_function(deps: &ToolDeps<'_>, id: i64) -> Result<functions::Model, ToolError> {
    functions::Entity::find_by_id(id)
        .filter(functions::Column::OrgId.eq(deps.org_id))
        .one(deps.db)
        .await
        .map_err(|e| format!("reading function: {e}"))?
        .ok_or_else(|| format!("function #{id} not found in this organization").into())
}

fn language_arg(args: &Value) -> Result<pnex_core::FunctionLanguage, ToolError> {
    let raw = str_arg(args, "language")?;
    crate::services::functions::parse_language(&raw)
        .ok_or_else(|| format!("language must be \"js\" or \"starlark\", got {raw}").into())
}

fn function_error(e: crate::services::functions::FunctionWriteError) -> ToolError {
    use crate::services::functions::FunctionWriteError as E;
    match e {
        E::Conflict { current } => format!(
            "version conflict: the function is now at version {current} — reload it with get_function and redo the change"
        ),
        E::Db(_) => "database error".to_string(),
        other => other.to_string(),
    }
    .into()
}

fn runtime_settings(deps: &ToolDeps<'_>) -> Result<crate::services::flow::FlowSettings, ToolError> {
    let config = deps
        .config
        .ok_or_else(|| "the flow runtime is not available here".to_string())?;
    let settings = crate::services::flow::FlowSettings::from_config(config);
    if !settings.enabled {
        return Err(
            "the flow runtime is not enabled on this server: functions cannot be checked or tested"
                .to_string()
                .into(),
        );
    }
    Ok(settings)
}

pub async fn list_functions(deps: &ToolDeps<'_>) -> Result<ToolOutcome, ToolError> {
    let rows = functions::Entity::find()
        .filter(functions::Column::OrgId.eq(deps.org_id))
        .order_by_asc(functions::Column::Name)
        .limit(LIST_CAP)
        .all(deps.db)
        .await
        .map_err(|e| format!("reading functions: {e}"))?;
    let list: Vec<Value> = rows
        .iter()
        .map(|f| json!({ "id": f.id, "name": f.name, "language": f.language, "description": f.description }))
        .collect();
    outcome(json!({ "functions": list }))
}

/// Latest code and declared interface of a function, plus the deployed
/// flows of the org that use it (and the version they pin).
pub async fn get_function(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let f = find_function(deps, i64_arg(args, "function_id")?).await?;
    let latest = crate::services::functions::latest_version(deps.db, f.id)
        .await
        .map_err(|e| format!("reading version: {e}"))?;
    let used_by: Vec<Value> = crate::services::functions::referencing_deployed_flows(deps.db, f.id)
        .await
        .map_err(|e| format!("reading flows: {e}"))?
        .into_iter()
        .filter(|(flow, _)| flow.org_id == deps.org_id)
        .map(|(flow, v)| json!({ "flow_id": flow.id, "name": flow.name, "deployed_flow_version": v }))
        .collect();
    let versions = {
        use sea_orm::PaginatorTrait;
        function_versions::Entity::find()
            .filter(function_versions::Column::FunctionId.eq(f.id))
            .count(deps.db)
            .await
            .unwrap_or(0)
    };
    outcome(json!({
        "function": { "id": f.id, "name": f.name, "language": f.language, "description": f.description },
        "latest_version": latest.as_ref().map(|v| v.version_number),
        "versions": versions,
        "code": latest.as_ref().map(|v| v.code.clone()),
        "inputs": latest.as_ref().map(|v| v.inputs.clone()),
        "outputs": latest.as_ref().map(|v| v.outputs.clone()),
        "deployed_flows_using_it": used_by,
    }))
}

/// Compile-only check of a code (no execution).
pub async fn validate_function(
    deps: &ToolDeps<'_>,
    args: &Value,
) -> Result<ToolOutcome, ToolError> {
    let settings = runtime_settings(deps)?;
    let req = pnex_core::FunctionValidateRequest {
        language: language_arg(args)?,
        code: str_arg(args, "code")?,
    };
    let _permit = crate::services::compute_limits::runtime_check()
        .try_acquire()
        .map_err(|_| "the server is busy, retry shortly".to_string())?;
    let resp = crate::services::functions::spawn_function_check(&settings, &req).await?;
    outcome(serde_json::to_value(resp).unwrap_or_default())
}

/// Runs a function in the sandbox of the UI's Test button (no network,
/// no device, no side effect): the saved latest version, or an ad-hoc code.
pub async fn test_function(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let settings = runtime_settings(deps)?;
    let f = find_function(deps, i64_arg(args, "function_id")?).await?;
    let ad_hoc = match args.get("code").and_then(Value::as_str) {
        Some(code) => Some(pnex_core::FunctionTestAdHoc {
            language: crate::services::functions::parse_language(&f.language).unwrap_or_default(),
            code: code.to_string(),
        }),
        None => None,
    };
    let inputs: BTreeMap<String, Value> = args
        .get("inputs")
        .and_then(Value::as_object)
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    let req = pnex_core::FunctionTestRequest {
        version_number: None,
        ad_hoc,
        inputs,
        msg: args.get("msg").cloned(),
    };
    let _permit = crate::services::compute_limits::runtime_check()
        .acquire()
        .await
        .map_err(|_| "the server is busy, retry shortly".to_string())?;
    let resp = crate::services::functions::spawn_function_test(
        &settings,
        deps.db,
        deps.org_id,
        f.id,
        &req,
    )
    .await?;
    outcome(serde_json::to_value(resp).unwrap_or_default())
}

pub async fn create_function(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    require_write(deps)?;
    let input = pnex_core::CreateFunction {
        name: str_arg(args, "name")?,
        language: language_arg(args)?,
        description: args
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        code: str_arg(args, "code")?,
        note: Some("created by the assistant".into()),
    };
    let (f, v) = crate::services::functions::create_function(deps.db, deps.org_id, &input)
        .await
        .map_err(function_error)?;
    outcome(json!({ "function_id": f.id, "name": f.name, "version": v.version_number }))
}

/// New version (append-only) of a function's code and/or metadata, on top
/// of the version read with get_function. Flows keep their pinned version
/// until the user picks the new one and redeploys.
pub async fn update_function(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    require_write(deps)?;
    let f = find_function(deps, i64_arg(args, "function_id")?).await?;
    let input = pnex_core::SaveFunctionVersion {
        expected_version_number: i64_arg(args, "expected_version")?,
        code: args.get("code").and_then(Value::as_str).map(str::to_string),
        name: args.get("name").and_then(Value::as_str).map(str::to_string),
        description: args
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        note: Some(
            args.get("note")
                .and_then(Value::as_str)
                .unwrap_or("modified by the assistant")
                .to_string(),
        ),
    };
    let (f, v) = crate::services::functions::save_new_version(deps.db, f, &input)
        .await
        .map_err(function_error)?;
    outcome(json!({
        "function_id": f.id,
        "new_version": v.map(|v| v.version_number),
        "note": "deployed flows keep their pinned version: the user selects the new version in the flow node and redeploys",
    }))
}

// ─────────────────────────── Read-only surfaces ───────────────────────────

pub async fn list_annotation_layers(deps: &ToolDeps<'_>) -> Result<ToolOutcome, ToolError> {
    let rows = annotation_layers::Entity::find()
        .filter(annotation_layers::Column::OrgId.eq(deps.org_id))
        .order_by_asc(annotation_layers::Column::Name)
        .limit(LIST_CAP)
        .all(deps.db)
        .await
        .map_err(|e| format!("reading annotation sets: {e}"))?;
    let list: Vec<Value> = rows
        .iter()
        .map(|l| {
            json!({
                "id": l.id,
                "name": l.name,
                "description": l.description,
                "published": l.published_version_id.is_some(),
                "on": if l.tour_id.is_some() { "tour" } else if l.media_asset_id.is_some() { "media" } else { "none" },
            })
        })
        .collect();
    outcome(json!({ "annotation_sets": list }))
}

pub async fn list_tours(deps: &ToolDeps<'_>) -> Result<ToolOutcome, ToolError> {
    let rows = tours::Entity::find()
        .filter(tours::Column::OrgId.eq(deps.org_id))
        .order_by_asc(tours::Column::Name)
        .limit(LIST_CAP)
        .all(deps.db)
        .await
        .map_err(|e| format!("reading tours: {e}"))?;
    // The share token is a credential: only whether a public link exists.
    let list: Vec<Value> = rows
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "name": t.name,
                "description": t.description,
                "mode": t.mode,
                "published": t.published_version_id.is_some(),
                "public_link": t.share_token.is_some(),
            })
        })
        .collect();
    outcome(json!({ "tours": list }))
}

pub async fn list_pois(deps: &ToolDeps<'_>) -> Result<ToolOutcome, ToolError> {
    let pins = map_pins::Entity::find()
        .filter(map_pins::Column::OrgId.eq(deps.org_id))
        .order_by_asc(map_pins::Column::Label)
        .limit(LIST_CAP)
        .all(deps.db)
        .await
        .map_err(|e| format!("reading POIs: {e}"))?;
    let mut devices: HashMap<Uuid, Vec<String>> = HashMap::new();
    for p in device_placements::Entity::find()
        .filter(device_placements::Column::OrgId.eq(deps.org_id))
        .all(deps.db)
        .await
        .map_err(|e| format!("reading placements: {e}"))?
    {
        devices.entry(p.pin_id).or_default().push(p.device_id);
    }
    let list: Vec<Value> = pins
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "label": p.label,
                "mode": p.mode,
                "location": p.location_detail,
                "latitude": p.latitude.map(|d| d.to_string()),
                "longitude": p.longitude.map(|d| d.to_string()),
                "devices": devices.get(&p.id).cloned().unwrap_or_default(),
            })
        })
        .collect();
    outcome(json!({ "pois": list }))
}

// ─────────────────────────── Controls and shared memory (read) ───────────────────────────

/// Controls of the org with their last commanded value and the deployed
/// flows listening to them. Read only: the assistant never writes a
/// control (D143) — a surface or the user does.
pub async fn list_controls(deps: &ToolDeps<'_>) -> Result<ToolOutcome, ToolError> {
    use crate::models::_entities::controls;
    let rows = controls::Entity::find()
        .filter(controls::Column::OrgId.eq(deps.org_id))
        .order_by_asc(controls::Column::Label)
        .limit(LIST_CAP)
        .all(deps.db)
        .await
        .map_err(|e| format!("reading controls: {e}"))?;
    let conn = match deps.config {
        Some(c) => crate::services::shared_valkey::conn(c).await,
        None => None,
    };
    let ids: Vec<Uuid> = rows.iter().map(|c| c.id).collect();
    let values = crate::services::controls::read_values(conn, deps.org_id, &ids).await;
    let listeners = super::dashboard_tools::coupled_controls(deps.db, deps.org_id).await?;
    let list: Vec<Value> = rows
        .iter()
        .map(|c| {
            json!({
                "id": c.id,
                "key": c.key,
                "label": c.label,
                "kind": c.kind,
                "spec": c.spec,
                "declared_by": c.origin,
                "value": values.get(&c.id).cloned().flatten(),
                "deployed_flows_listening": listeners
                    .get(&c.id)
                    .map(|fs| fs.iter().map(|(id, n)| json!({"id": id, "name": n})).collect::<Vec<_>>())
                    .unwrap_or_default(),
            })
        })
        .collect();
    outcome(json!({ "controls": list }))
}

/// Shared memory of the org (memory_write nodes): without `keys`, the
/// keys with their numeric fields and age; with `keys`, their values.
pub async fn read_memory(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let config = deps
        .config
        .ok_or_else(|| "the shared memory is not available here".to_string())?;
    let keys: Vec<String> = args
        .get("keys")
        .and_then(Value::as_array)
        .map(|ks| {
            ks.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if keys.is_empty() {
        let list = crate::services::memory::list_keys(config, deps.org_id).await;
        return outcome(json!({ "keys": list }));
    }
    let refs: Vec<pnex_core::memory::MemoryRef> = keys
        .iter()
        .take(pnex_core::memory::MEMORY_VALUES_CAP)
        .map(|k| {
            let (key, field) = k.split_once('#').unwrap_or((k.as_str(), ""));
            pnex_core::memory::MemoryRef {
                key: key.to_string(),
                field: field.to_string(),
            }
        })
        .collect();
    let resp = crate::services::memory::values(config, deps.org_id, &refs).await;
    let values: Vec<Value> = keys
        .iter()
        .zip(resp.results.iter())
        .map(|(k, v)| json!({ "key": k, "available": v.available, "value": v.value, "ts_ms": v.ts_ms }))
        .collect();
    outcome(json!({ "available": resp.available, "values": values }))
}
