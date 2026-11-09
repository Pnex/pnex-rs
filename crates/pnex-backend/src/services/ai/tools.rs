//! AI assistant tool registry — **the only execution surface of the
//! LLM outputs**.
//!
//! Structural safeguard: `execute` is a closed `match` over exactly the
//! tools declared in `tool_specs()` (each domain module declares its own
//! tools, spec and trace summary together, see [`ToolDef`]). There is no generic tool
//! (http/sql), no path to `flow_supervisor` (deploy), no deletion,
//! no device command. Any name outside this match → "unknown tool" error
//! returned to the model (which apologizes and corrects itself).
//!
//! Every tool is org-scoped via `ToolDeps.org_id` (never an org parameter
//! on the model side); every tool first needs its permission of the IAM
//! registry (`pnex_api_contract::iam::TOOLS`), checked by the engine.

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde_json::{json, Value};

use crate::models::_entities::{
    device_capability_instances, device_registries, flow_versions, flows, notify_channels,
    notify_templates, predefined_devices,
};
use crate::services::ai::provider::ToolSpec;
use crate::services::flow;

/// Dependencies injected by the controller (already authenticated).
pub struct ToolDeps<'a> {
    pub db: &'a DatabaseConnection,
    /// Org of the member talking to the assistant (systematic scoping).
    pub org_id: i64,
    /// What the member holds in the org: the engine checks each tool's
    /// registry permission against it before running the tool (iam.md D291).
    pub grants: crate::services::iam::Grants,
    /// Author recorded in `flow_versions.author`.
    pub author: Option<String>,
    /// Resolved O2 client (`None` = degraded telemetry).
    pub o2: Option<&'a crate::services::openobserve::client::Client>,
    /// App config (live caches, flow cluster); `None` in unit tests.
    pub config: Option<&'a loco_rs::config::Config>,
    /// Whole app context, for tools that start background work (a rule run
    /// on the server browser); `None` in unit tests.
    pub app: Option<&'a loco_rs::app::AppContext>,
    /// Who writes through the ontology tools (role for D188, provenance
    /// `assistant:<user>` for D184); `None` in unit tests.
    pub actor: Option<crate::services::ontology::Actor>,
    /// The conversation is frozen at this instant (D902): the harness sets
    /// every tool's instant, tools without one are withdrawn.
    pub frozen_at: Option<chrono::DateTime<chrono::FixedOffset>>,
}

/// Execution result: the JSON value seen by the model + the id of the touched
/// flow (for the "Open in the editor" deep-link in the UI).
#[derive(Debug)]
pub struct ToolOutcome {
    pub value: Value,
    /// Set by create_flow/update_flow.
    pub flow_id: Option<i64>,
}

/// Refusal of a tool. `message` is what the model reads; `code`/`args`
/// let the UI render the refusal in the user's language (`err-<code>`).
#[derive(Debug)]
pub struct ToolError {
    pub message: String,
    pub code: Option<&'static str>,
    pub args: Option<Value>,
}

impl From<String> for ToolError {
    fn from(message: String) -> Self {
        Self {
            message,
            code: None,
            args: None,
        }
    }
}

impl From<&str> for ToolError {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

/// Engine decision for a tool (iam.md D291): the permission the registry
/// gives it, against the member's role. A refusal is coded so the UI
/// renders it in the user's language.
pub(super) fn authorize_tool(deps: &ToolDeps<'_>, name: &str) -> Result<(), ToolError> {
    // A tool absent from the registry is refused, never run unchecked (SEC-79).
    let Some(entry) = pnex_api_contract::iam::tool(name) else {
        return Err(format!(
            "unknown tool: {name} — only the tools listed in this conversation are available"
        )
        .into());
    };
    let Some(permission) = entry else {
        return Ok(());
    };
    if deps.grants.can(permission) {
        return Ok(());
    }
    if let Some(user_id) = deps.actor.as_ref().and_then(|a| a.user_id) {
        crate::services::iam::audit::submit(crate::services::iam::audit::Entry {
            org_id: deps.org_id,
            user_id,
            service_account_id: None,
            operation: format!("tool:{name}"),
            permission: permission.to_string(),
            allowed: false,
            status: 0,
            origin: "assistant",
        });
    }
    Err(ToolError {
        message: format!(
            "the user's role does not grant the {permission} permission: the assistant cannot do this for them"
        ),
        code: Some(pnex_core::err_codes::ACCESS_DENIED),
        args: Some(json!({ "permission": permission })),
    })
}

/// Internal failure of a tool (database, store): the detail is logged, the
/// model and the UI only get a generic coded refusal — a database error may
/// carry SQL or identifiers (R4, R16).
pub(super) fn internal<E: std::fmt::Display>(context: &'static str) -> impl FnOnce(E) -> ToolError {
    move |e| {
        tracing::error!(error = %e, context, "assistant tool internal failure");
        ToolError {
            message: format!("internal error while {context}; tell the user to try again later"),
            code: Some(pnex_core::err_codes::AI_TOOL_INTERNAL),
            args: None,
        }
    }
}

/// Trace of a tool, returned to the front (collapsible ✓/✗ bubble).
#[derive(Clone, Debug)]
pub struct ToolTrace {
    pub name: String,
    pub arguments: Value,
    pub ok: bool,
    /// Short canonical English summary (verbatim fallback of the UI).
    pub summary: String,
    /// Fluent key of a successful call's summary, rendered by the UI with
    /// `args` (see [`TraceSummary`]).
    pub summary_key: Option<&'static str>,
    /// Touched flow — drives the "Open in the editor" button.
    pub flow_id: Option<i64>,
    /// Machine code of a coded refusal (`ai-flow-running`…).
    pub code: Option<&'static str>,
    /// Interpolation data of `code` (refusal) or of `summary_key` (success).
    pub args: Option<Value>,
}

/// One assistant tool: what the model is told (`spec`) and how the UI sums
/// up a successful call (`summary`), kept together so a tool lives in one
/// place, in the module of its domain.
pub struct ToolDef {
    pub spec: ToolSpec,
    pub summary: fn(&Value) -> TraceSummary,
}

/// Every registry tool, in the order the model sees them.
fn registry() -> &'static [ToolDef] {
    static TOOLS: std::sync::LazyLock<Vec<ToolDef>> = std::sync::LazyLock::new(|| {
        [
            flow_read_tools(),
            super::dashboard_tools::tools(),
            super::more_tools::tools(),
            super::ontology_tools::tools(),
            super::calendar_tools::tools(),
            super::market_tools::tools(),
            super::doc_tools::tools(),
            super::site_tools::tools(),
            super::mail_tools::tools(),
            flow_write_tools(),
        ]
        .into_iter()
        .flatten()
        .collect()
    });
    &TOOLS
}

/// The registry tools, as declared to the model.
pub fn tool_specs() -> Vec<ToolSpec> {
    registry().iter().map(|t| t.spec.clone()).collect()
}

/// The registry tools an internal agent may be given (D785), with their write flag.
pub fn tool_infos() -> Vec<pnex_core::agents::AgentToolInfo> {
    registry()
        .iter()
        .map(|t| pnex_core::agents::AgentToolInfo {
            name: t.spec.name.to_string(),
            summary: t.spec.description.to_string(),
            write: super::agent::is_write_tool(t.spec.name),
            never_delegated: crate::services::agent_gate::never_delegated(t.spec.name),
        })
        .collect()
}

fn flow_read_tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            spec: ToolSpec {
                name: "list_devices",
                description: "Lists the organization's devices (internal id, slug, model, active).",
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
            summary: |v| {
                let n = count(&v["devices"]);
                trace(
                    "ai-trace-devices",
                    json!({"count": n}),
                    format!("{n} device(s)"),
                )
            },
        },
        ToolDef {
            spec: ToolSpec {
                name: "get_device_pins",
                description: "Detail of a device's pins (gpio, label, mode) — id = internal id returned by list_devices.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "device_id": {"type": "integer", "description": "internal device id (list_devices)"}
                    },
                    "required": ["device_id"]
                }),
            },
            summary: |v| {
                let (d, n) = (shown(&v["device_id"]), count(&v["pins"]));
                trace(
                    "ai-trace-device-pins",
                    json!({"device": d, "count": n}),
                    format!("device #{d}: {n} pin(s)"),
                )
            },
        },
        ToolDef {
            spec: ToolSpec {
                name: "list_flows",
                description: "Lists the organization's flows (name, draft/deployed status, versions).",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "status": {"type": "string", "enum": ["draft", "deployed", "error"], "description": "optional filter"}
                    },
                    "required": []
                }),
            },
            summary: |v| {
                let n = count(&v["flows"]);
                trace(
                    "ai-trace-flows",
                    json!({"count": n}),
                    format!("{n} flow(s)"),
                )
            },
        },
        ToolDef {
            spec: ToolSpec {
                name: "get_flow",
                description: "Detail of a flow (full graph of the latest version) — id = internal id returned by list_flows.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "flow_id": {"type": "integer", "description": "internal flow id (list_flows)"}
                    },
                    "required": ["flow_id"]
                }),
            },
            summary: |v| {
                let (id, nm) = (shown(&v["flow"]["id"]), shown(&v["flow"]["name"]));
                trace(
                    "ai-trace-flow",
                    json!({"id": id, "name": nm}),
                    format!("flow #{id} \"{nm}\""),
                )
            },
        },
        ToolDef {
            spec: ToolSpec {
                name: "list_notifications",
                description: "Lists the organization's notification channels (id, name, kind, enabled — never their settings or secrets) and message templates (id, name, declared variables). Needed to wire a pnex_notify node: channel_ids and template_id are these UUIDs.",
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
            summary: |v| {
                let (c, t) = (count(&v["channels"]), count(&v["templates"]));
                trace(
                    "ai-trace-notifications",
                    json!({"channels": c, "templates": t}),
                    format!("{c} channel(s), {t} template(s)"),
                )
            },
        },
        ToolDef {
            spec: ToolSpec {
                name: "query_telemetry",
                description: "Points of an OpenObserve telemetry series (metric × device) over a window, + statistical summary. Uses the same source as the Visualization page.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "metric": {"type": "string", "description": "metric name (e.g. soil_moisture, a0 — see the catalog in the context)"},
                        "device_id": {"type": "string", "description": "device slug (device_id dimension of the series)"},
                        "window": {"type": "string", "enum": ["1h", "6h", "24h"], "description": "sliding window"}
                    },
                    "required": ["metric", "device_id", "window"]
                }),
            },
            summary: |v| {
                let (n, last) = (count(&v["points"]), shown(&v["summary"]["last"]));
                trace(
                    "ai-trace-telemetry",
                    json!({"count": n, "last": last}),
                    format!("{n} point(s), last = {last}"),
                )
            },
        },
        ToolDef {
            spec: ToolSpec {
                name: "describe_node_types",
                description: "Documentation of every flow node kind (purpose, config fields, ports, pitfalls), the authoring rules and a valid example graph. Read it before creating or updating a flow; pass `kinds` to fetch only some kinds.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "kinds": {"type": "array", "items": {"type": "string"}, "description": "optional subset of node kinds, e.g. [\"device_read\", \"calc\"]"}
                    },
                    "required": []
                }),
            },
            summary: |_| trace("ai-trace-node-types", json!({}), "node catalogue".into()),
        },
        ToolDef {
            spec: ToolSpec {
                name: "search_knowledge",
                description: "Searches the PneX knowledge cards (features, how-tos, troubleshooting) embedded in the server. Use it before answering how something works, where a setting is, or why something fails; then read_knowledge for the full card. Answers must describe gestures in the UI only.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "keywords, e.g. \"device no telemetry\""},
                        "limit": {"type": "integer", "description": "1..=5 (default 5)"}
                    },
                    "required": ["query"]
                }),
            },
            summary: |v| {
                let n = count(&v["results"]);
                trace(
                    "ai-trace-cards",
                    json!({"count": n}),
                    format!("{n} card(s)"),
                )
            },
        },
        ToolDef {
            spec: ToolSpec {
                name: "read_knowledge",
                description: "Full text of one knowledge card (id from search_knowledge).",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "id": {"type": "string", "description": "card id"}
                    },
                    "required": ["id"]
                }),
            },
            summary: |v| {
                let id = shown(&v["id"]);
                trace("ai-trace-card", json!({"id": id}), format!("card {id}"))
            },
        },
        ToolDef {
            spec: ToolSpec {
                name: "diagnose_device",
                description: "Read-only diagnosis of one device of the organization: online state and last seen, pins (mode, subscription period, last value), firmware version, last OTA update, hints. id = internal id from list_devices.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "device_id": {"type": "integer", "description": "internal device id (list_devices)"}
                    },
                    "required": ["device_id"]
                }),
            },
            summary: |v| {
                let (d, n) = (shown(&v["device"]["slug"]), count(&v["hints"]));
                trace(
                    "ai-trace-diagnose-device",
                    json!({"device": d, "count": n}),
                    format!("device {d}: {n} hint(s)"),
                )
            },
        },
        ToolDef {
            spec: ToolSpec {
                name: "diagnose_flow",
                description: "Read-only diagnosis of one flow: status, last saved vs deployed version, engine state and last engine error, last message of each debug/display node, hints.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "flow_id": {"type": "integer", "description": "internal flow id (list_flows)"}
                    },
                    "required": ["flow_id"]
                }),
            },
            summary: |v| {
                let (id, n) = (shown(&v["flow"]["id"]), count(&v["hints"]));
                trace(
                    "ai-trace-diagnose-flow",
                    json!({"id": id, "count": n}),
                    format!("flow #{id}: {n} hint(s)"),
                )
            },
        },
    ]
}

fn flow_write_tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            spec: ToolSpec {
                name: "validate_flow_graph",
                description: "Validates a flow graph (structure, nodes, wiring) WITHOUT saving it — returns the violations to fix. Call it before every create_flow/update_flow.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "graph": {"type": "object", "description": "graphe FlowGraph {nodes: [...]}"}
                    },
                    "required": ["graph"]
                }),
            },
            summary: |v| {
                if v["valid"].as_bool().unwrap_or(false) {
                    trace("ai-trace-graph-valid", json!({}), "valid graph".into())
                } else {
                    let n = count(&v["violations"]);
                    trace(
                        "ai-trace-graph-violations",
                        json!({"count": n}),
                        format!("{n} violation(s)"),
                    )
                }
            },
        },
        ToolDef {
            spec: ToolSpec {
                name: "validate_calc_expression",
                description: "Validates the syntax of a calc expression (formula of the calc node) WITHOUT saving it. The case of device keys can only be checked in the graph: use validate_flow_graph after wiring to the device.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "expression": {"type": "string", "description": "expression (variables = device payload keys, e.g. probe_1_A0)"}
                    },
                    "required": ["expression"]
                }),
            },
            summary: |v| {
                if v["valid"].as_bool().unwrap_or(false) {
                    trace(
                        "ai-trace-expression-valid",
                        json!({}),
                        "valid expression".into(),
                    )
                } else {
                    trace(
                        "ai-trace-expression-invalid",
                        json!({}),
                        "invalid expression".into(),
                    )
                }
            },
        },
        ToolDef {
            spec: ToolSpec {
                name: "create_flow",
                description: "Creates a NEW flow as a draft (version 1) — never deployed. The graph must be validated first (validate_flow_graph).",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "minLength": 1, "maxLength": 200, "description": "flow name"},
                        "graph": {"type": "object", "description": "graphe FlowGraph {nodes: [...]}"},
                        "device_id": {"type": "integer", "description": "optional internal id of the attached device (list_devices)"}
                    },
                    "required": ["name", "graph"]
                }),
            },
            summary: |v| {
                let (id, nm, ver) = (
                    shown(&v["flow_id"]),
                    shown(&v["name"]),
                    shown(&v["version"]),
                );
                trace(
                    "ai-trace-flow-created",
                    json!({"id": id, "name": nm, "version": ver}),
                    format!("flow #{id} \"{nm}\" created (draft v{ver})"),
                )
            },
        },
        ToolDef {
            spec: ToolSpec {
                name: "update_flow",
                description: "Saves a NEW VERSION of an existing flow (append-only, never deployed). Only a stopped or never-deployed flow can be edited: a deployed flow is refused with ai-flow-running — ask the user to stop it in the flow editor first. Pass the latest_version_number read with get_flow as expected_version; on a version conflict, reload with get_flow and redo the change.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "flow_id": {"type": "integer", "description": "internal flow id (list_flows)"},
                        "expected_version": {"type": "integer", "description": "latest_version_number returned by get_flow"},
                        "graph": {"type": "object", "description": "full FlowGraph graph of the new version"},
                        "note": {"type": "string", "description": "optional version note"}
                    },
                    "required": ["flow_id", "expected_version", "graph"]
                }),
            },
            summary: |v| {
                let (id, ver) = (shown(&v["flow_id"]), shown(&v["version"]));
                trace(
                    "ai-trace-flow-saved",
                    json!({"id": id, "version": ver}),
                    format!("flow #{id}: version {ver} saved (draft)"),
                )
            },
        },
    ]
}

// ───────────────────────── Argument extraction ─────────────────────────

fn arg_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("argument '{key}' missing or not a string"))
}

fn arg_i64(args: &Value, key: &str) -> Result<i64, String> {
    args.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("argument '{key}' missing or not an integer"))
}

fn arg_value<'a>(args: &'a Value, key: &str) -> Result<&'a Value, String> {
    args.get(key)
        .ok_or_else(|| format!("argument '{key}' missing"))
}

// ─────────────────────────── Execution ───────────────────────────

/// Runs a registry tool. **Closed** `match` — any other name is
/// refused. The errors (String) are returned to the model as a failed tool
/// result; it corrects itself and answers the user.
pub async fn execute(
    deps: &ToolDeps<'_>,
    name: &str,
    args: &Value,
) -> Result<ToolOutcome, ToolError> {
    authorize_tool(deps, name)?;
    let Some(at) = deps.frozen_at else {
        return dispatch(deps, name, args).await;
    };
    let (args, brought_back) = match super::frozen::policy(name) {
        None => {
            return Err(ToolError {
                message: format!(
                    "{name} is not available: this conversation is frozen at {} and only reads that answer as of a date remain",
                    at.to_rfc3339()
                ),
                code: Some(pnex_core::err_codes::AI_FROZEN_TOOL),
                args: Some(json!({ "tool": name, "at": at.to_rfc3339() })),
            })
        }
        Some(super::frozen::Policy::AsOf(path)) => super::frozen::set_instant(args, path, at),
        Some(_) => (args.clone(), false),
    };
    let mut out = dispatch(deps, name, &args).await?;
    super::frozen::annotate(&mut out.value, at, brought_back);
    Ok(out)
}

async fn dispatch(deps: &ToolDeps<'_>, name: &str, args: &Value) -> Result<ToolOutcome, ToolError> {
    match name {
        "list_devices" => list_devices(deps).await,
        "get_device_pins" => get_device_pins(deps, args).await,
        "list_flows" => list_flows(deps, args).await,
        "get_flow" => get_flow(deps, args).await,
        "query_telemetry" => query_telemetry(deps, args).await,
        "list_notifications" => list_notifications(deps).await,
        "describe_node_types" => describe_node_types(args),
        "search_knowledge" => search_knowledge(args),
        "read_knowledge" => read_knowledge(args),
        "diagnose_device" => diagnose_device(deps, args).await,
        "diagnose_flow" => diagnose_flow(deps, args).await,
        "validate_flow_graph" => validate_flow_graph(args),
        "validate_calc_expression" => validate_calc_expression(args),
        "create_flow" => create_flow(deps, args).await,
        "update_flow" => update_flow(deps, args).await,
        "list_dashboards" => super::dashboard_tools::list_dashboards(deps).await,
        "get_dashboard" => super::dashboard_tools::get_dashboard(deps, args).await,
        "validate_dashboard_layout" => {
            super::dashboard_tools::validate_dashboard_layout(deps, args).await
        }
        "create_dashboard" => super::dashboard_tools::create_dashboard(deps, args).await,
        "update_dashboard" => super::dashboard_tools::update_dashboard(deps, args).await,
        "get_notification_template" => {
            super::more_tools::get_notification_template(deps, args).await
        }
        "preview_notification_template" => {
            super::more_tools::preview_notification_template(deps, args).await
        }
        "create_notification_template" => {
            super::more_tools::create_notification_template(deps, args).await
        }
        "update_notification_template" => {
            super::more_tools::update_notification_template(deps, args).await
        }
        "list_functions" => super::more_tools::list_functions(deps).await,
        "get_function" => super::more_tools::get_function(deps, args).await,
        "validate_function" => super::more_tools::validate_function(deps, args).await,
        "test_function" => super::more_tools::test_function(deps, args).await,
        "create_function" => super::more_tools::create_function(deps, args).await,
        "update_function" => super::more_tools::update_function(deps, args).await,
        "list_annotation_sets" => super::more_tools::list_annotation_layers(deps).await,
        "list_tours" => super::more_tools::list_tours(deps).await,
        "list_pois" => super::more_tools::list_pois(deps).await,
        "list_controls" => super::more_tools::list_controls(deps).await,
        "read_memory" => super::more_tools::read_memory(deps, args).await,
        "list_taxonomies" => super::more_tools::list_taxonomies(deps).await,
        "get_taxonomy" => super::more_tools::get_taxonomy(deps, args).await,
        "create_taxonomy_version" => super::more_tools::create_taxonomy_version(deps, args).await,
        "describe_ontology" => super::ontology_tools::describe_ontology(deps).await,
        "query_ontology" => super::ontology_tools::query_ontology(deps, args).await,
        "aggregate_objects" => super::ontology_tools::aggregate_objects(deps, args).await,
        "get_object" => super::ontology_tools::get_object(deps, args).await,
        "case_workspace" => super::ontology_tools::case_workspace(deps, args).await,
        "case_signals" => super::ontology_tools::case_signals(deps, args).await,
        "case_board" => super::ontology_tools::case_board(deps, args).await,
        "case_proposals" => super::ontology_tools::case_proposals(deps, args).await,
        "create_object" => super::ontology_tools::create_object(deps, args).await,
        "update_object" => super::ontology_tools::update_object(deps, args).await,
        "open_link" => super::ontology_tools::open_link(deps, args).await,
        "close_link" => super::ontology_tools::close_link(deps, args).await,
        "propose_action" => super::ontology_tools::propose_action(deps, args).await,
        "save_object_type" => super::ontology_tools::save_object_type(deps, args).await,
        "list_calendar" => super::calendar_tools::list_calendar(deps, args).await,
        "propose_occurrence_change" => {
            super::calendar_tools::propose_occurrence_change(deps, args).await
        }
        "market_figures" => super::market_tools::market_figures(deps, args).await,
        "market_rule_test" => super::market_tools::market_rule_test(deps, args).await,
        "market_session" => super::market_tools::market_session(deps, args).await,
        "search_docs" => super::doc_tools::search_docs(deps, args).await,
        "read_chunk" => super::doc_tools::read_chunk(deps, args).await,
        "open_page" => super::doc_tools::open_page(deps, args).await,
        "list_site_recipes" => super::site_tools::list_site_recipes(deps, args).await,
        "get_site_recipe" => super::site_tools::get_site_recipe(deps, args).await,
        "try_site_recipe" => super::site_tools::try_site_recipe(deps, args).await,
        "get_site_recipe_run" => super::site_tools::get_site_recipe_run(deps, args).await,
        "propose_site_recipe" => super::site_tools::propose_site_recipe(deps, args).await,
        "list_mailboxes" => super::mail_tools::list_mailboxes(deps, args).await,
        "list_mails" => super::mail_tools::list_mails(deps, args).await,
        "read_mail" => super::mail_tools::read_mail(deps, args).await,
        _ => Err(format!(
            "unknown tool: {name} — only the tools listed in this conversation are available"
        )
        .into()),
    }
}

/// Short summary of a successful tool call, for the UI trace: `key` is the
/// fluent key the UI renders with `args` (every value a string), `text` the
/// canonical English form (verbatim fallback, and what the history replay
/// shows the model).
pub struct TraceSummary {
    pub key: &'static str,
    pub args: Value,
    pub text: String,
}

/// Length of a JSON array field (0 when absent).
pub(super) fn count(v: &Value) -> String {
    v.as_array().map_or(0, Vec::len).to_string()
}

/// Display form of a scalar JSON field (strings unquoted, absent = "?").
pub(super) fn shown(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "?".to_string(),
        other => other.to_string(),
    }
}

/// Summary of one successful tool call (see [`TraceSummary`]).
pub fn summarize(name: &str, out: &ToolOutcome) -> TraceSummary {
    registry().iter().find(|t| t.spec.name == name).map_or_else(
        || trace("ai-trace-ok", json!({}), "ok".into()),
        |t| (t.summary)(&out.value),
    )
}

/// Builds a [`TraceSummary`].
pub(super) fn trace(key: &'static str, args: Value, text: String) -> TraceSummary {
    TraceSummary { key, args, text }
}

// ─────────────────────────── Tools: devices ───────────────────────────

/// Defensive cap of the device listing (the prompt context already lists
/// a few; here it is the full but bounded view).
const DEVICES_CAP: usize = 50;
/// Cap of the flows listing.
const FLOWS_CAP: usize = 20;

async fn list_devices(deps: &ToolDeps<'_>) -> Result<ToolOutcome, ToolError> {
    let rows = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(deps.org_id))
        .order_by_asc(device_registries::Column::Id)
        .find_also_related(predefined_devices::Entity)
        .all(deps.db)
        .await
        .map_err(internal("reading devices"))?;
    let devices: Vec<Value> = rows
        .into_iter()
        .take(DEVICES_CAP)
        .map(|(d, pre)| {
            json!({
                "id": d.id,
                "slug": d.device_id,
                "model": pre.map(|p| p.name),
                "active": d.active,
            })
        })
        .collect();
    Ok(ToolOutcome {
        value: json!({ "devices": devices }),
        flow_id: None,
    })
}

async fn get_device_pins(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let device_id = arg_i64(args, "device_id")?;
    // Org scoping: the device must belong to the org (404 equivalent).
    let Some(device) = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(deps.org_id))
        .filter(device_registries::Column::Id.eq(device_id))
        .one(deps.db)
        .await
        .map_err(internal("reading the device"))?
    else {
        return Err(format!("device #{device_id} not found in this organization").into());
    };
    let pins = device_capability_instances::Entity::find()
        .filter(device_capability_instances::Column::DeviceRegistryId.eq(device.id))
        .order_by_asc(device_capability_instances::Column::Gpio)
        .all(deps.db)
        .await
        .map_err(internal("reading pins"))?;
    Ok(ToolOutcome {
        value: json!({
            "device_id": device.id,
            "slug": device.device_id,
            "pins": pins.iter().map(|p| json!({
                "gpio": p.gpio,
                "label": p.label,
                "mode": p.mode,
            })).collect::<Vec<_>>(),
        }),
        flow_id: None,
    })
}

// ─────────────────────────── Tools: flows ───────────────────────────

async fn list_flows(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let status_filter = args.get("status").and_then(Value::as_str);
    let rows = flows::Entity::find()
        .filter(flows::Column::OrgId.eq(deps.org_id))
        .order_by_desc(flows::Column::Id)
        .all(deps.db)
        .await
        .map_err(internal("reading flows"))?;
    let page_rows: Vec<&flows::Model> = rows
        .iter()
        .filter(|f| status_filter.is_none_or(|s| f.status == s))
        .take(FLOWS_CAP)
        .collect();
    let ids: Vec<i64> = page_rows.iter().map(|f| f.id).collect();
    let latest: std::collections::HashMap<i64, i64> = if ids.is_empty() {
        Default::default()
    } else {
        flow_versions::Entity::find()
            .select_only()
            .column(flow_versions::Column::FlowId)
            .column_as(flow_versions::Column::VersionNumber.max(), "latest")
            .filter(flow_versions::Column::FlowId.is_in(ids))
            .group_by(flow_versions::Column::FlowId)
            .into_tuple::<(i64, i64)>()
            .all(deps.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect()
    };
    let flows_json: Vec<Value> = page_rows
        .iter()
        .map(|f| {
            json!({
                "id": f.id,
                "name": f.name,
                "status": f.status,
                "latest_version_number": latest.get(&f.id).copied().unwrap_or(0),
                "deployed": f.deployed_version_id.is_some(),
            })
        })
        .collect();
    Ok(ToolOutcome {
        value: json!({ "flows": flows_json }),
        flow_id: None,
    })
}

async fn get_flow(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let flow_id = arg_i64(args, "flow_id")?;
    let Some(flow) = flows::Entity::find_by_id(flow_id)
        .filter(flows::Column::OrgId.eq(deps.org_id))
        .one(deps.db)
        .await
        .map_err(internal("reading the flow"))?
    else {
        return Err(format!("flow #{flow_id} not found in this organization").into());
    };
    let Some(version) = flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(flow.id))
        .order_by_desc(flow_versions::Column::VersionNumber)
        .one(deps.db)
        .await
        .map_err(internal("reading the flow version"))?
    else {
        return Err(format!("flow #{flow_id} has no version (inconsistent state)").into());
    };
    Ok(ToolOutcome {
        value: json!({
            "flow": {
                "id": flow.id,
                "name": flow.name,
                "status": flow.status,
                "latest_version_number": version.version_number,
                "graph": version.graph,
            }
        }),
        flow_id: None,
    })
}

async fn query_telemetry(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let metric = arg_str(args, "metric")?;
    let device_slug = arg_str(args, "device_id")?;
    let window = arg_str(args, "window")?;
    let resp = crate::services::visualization::series_points(
        deps.db,
        deps.o2,
        deps.org_id,
        &metric,
        &device_slug,
        &window,
    )
    .await
    .map_err(|e| {
        // The anti-injection/invalid-name 400s become a tool error
        // the model can use; the other errors too (never a raw 500).
        e.to_string()
    })?;
    let points: Vec<Value> = resp
        .points
        .iter()
        .map(|p| json!({ "t": p.ts, "v": p.value }))
        .collect();
    let values: Vec<f64> = resp.points.iter().map(|p| p.value).collect();
    let summary = if values.is_empty() {
        json!(null)
    } else {
        let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let last = values[values.len() - 1];
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        json!({ "min": min, "max": max, "last": last, "mean": mean })
    };
    Ok(ToolOutcome {
        value: json!({
            "available": resp.available,
            "metric": resp.metric,
            "device_id": resp.device_id,
            "points": points,
            "summary": summary,
        }),
        flow_id: None,
    })
}

// ─────────────────── Static tools (pnex-core) ───────────────────

/// Static node documentation — built from the real config shapes of
/// `pnex-core/src/flow.rs` (a test checks that every `FlowNodeKind`
/// variant is mentioned).
/// Channels and templates of the org, as references for a pnex_notify node.
/// Channel settings (tokens, topics, webhooks) are never exposed to the model.
async fn list_notifications(deps: &ToolDeps<'_>) -> Result<ToolOutcome, ToolError> {
    let channels = notify_channels::Entity::find()
        .filter(notify_channels::Column::OrgId.eq(deps.org_id))
        .order_by_asc(notify_channels::Column::Name)
        .all(deps.db)
        .await
        .map_err(internal("reading notification channels"))?;
    let templates = notify_templates::Entity::find()
        .filter(notify_templates::Column::OrgId.eq(deps.org_id))
        .order_by_asc(notify_templates::Column::Name)
        .all(deps.db)
        .await
        .map_err(internal("reading notification templates"))?;
    let channels: Vec<Value> = channels
        .iter()
        .map(|c| json!({ "id": c.id, "name": c.name, "kind": c.kind, "enabled": c.enabled }))
        .collect();
    let templates: Vec<Value> = templates
        .iter()
        .map(|t| {
            let vars: Vec<Value> = t
                .vars
                .as_array()
                .map(|vs| vs.iter().filter_map(|v| v.get("name").cloned()).collect())
                .unwrap_or_default();
            json!({ "id": t.id, "name": t.name, "subject": t.subject, "vars": vars })
        })
        .collect();
    Ok(ToolOutcome {
        value: json!({ "channels": channels, "templates": templates }),
        flow_id: None,
    })
}

/// Serializes the node documentation table of `pnex_core` (D142): the
/// assistant's knowledge of node kinds has a single source, guarded there.
/// `kinds` (optional) narrows the answer to save tokens.
fn describe_node_types(args: &Value) -> Result<ToolOutcome, ToolError> {
    let wanted: Option<Vec<&str>> = args
        .get("kinds")
        .and_then(Value::as_array)
        .map(|ks| ks.iter().filter_map(Value::as_str).collect());
    let unknown: Vec<&str> = wanted
        .iter()
        .flatten()
        .copied()
        .filter(|k| pnex_core::node_doc(k).is_none())
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "unknown node kind(s): {} — call describe_node_types without kinds for the full list",
            unknown.join(", ")
        )
        .into());
    }
    let nodes: Vec<Value> = pnex_core::NODE_DOCS
        .iter()
        .filter(|d| wanted.as_ref().is_none_or(|w| w.contains(&d.kind)))
        .map(|d| {
            let config: serde_json::Map<String, Value> = d
                .config
                .iter()
                .map(|(field, meaning)| (field.to_string(), json!(meaning)))
                .collect();
            let mut node = json!({ "kind": d.kind, "summary": d.summary, "config": config });
            if !d.notes.is_empty() {
                node["notes"] = json!(d.notes);
            }
            node
        })
        .collect();
    let rules: serde_json::Map<String, Value> = pnex_core::FLOW_AUTHORING_RULES
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect();
    let example: Value =
        serde_json::from_str(pnex_core::FLOW_EXAMPLE).map_err(|e| format!("example: {e}"))?;
    Ok(ToolOutcome {
        value: json!({
            "rules": rules,
            "all_kinds": pnex_core::NODE_DOCS.iter().map(|d| d.kind).collect::<Vec<_>>(),
            "nodes": nodes,
            "example": example,
        }),
        flow_id: None,
    })
}

fn search_knowledge(args: &Value) -> Result<ToolOutcome, ToolError> {
    let query = arg_str(args, "query")?;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(super::knowledge::SEARCH_MAX, |l| l as usize);
    let hits = super::knowledge::search(&query, limit);
    Ok(ToolOutcome {
        value: json!({ "results": hits }),
        flow_id: None,
    })
}

fn read_knowledge(args: &Value) -> Result<ToolOutcome, ToolError> {
    let id = arg_str(args, "id")?;
    let card = super::knowledge::card(&id)
        .ok_or_else(|| format!("unknown card {id} — use search_knowledge to find ids"))?;
    Ok(ToolOutcome {
        value: json!({
            "id": card.id,
            "title": card.title,
            "kind": card.kind,
            "pages": card.pages,
            "body": card.body,
        }),
        flow_id: None,
    })
}

async fn diagnose_device(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let device_id = arg_i64(args, "device_id")?;
    let value = super::diagnose::device(deps.db, deps.config, deps.org_id, device_id).await?;
    Ok(ToolOutcome {
        value,
        flow_id: None,
    })
}

async fn diagnose_flow(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let flow_id = arg_i64(args, "flow_id")?;
    let value = super::diagnose::flow(deps.db, deps.config, deps.org_id, flow_id).await?;
    Ok(ToolOutcome {
        value,
        flow_id: None,
    })
}

fn validate_flow_graph(args: &Value) -> Result<ToolOutcome, ToolError> {
    let graph: pnex_core::FlowGraph = serde_json::from_value(arg_value(args, "graph")?.clone())
        .map_err(|e| format!("unreadable graph (FlowGraph expected): {e}"))?;
    let violations = pnex_core::validate_graph(&graph);
    Ok(ToolOutcome {
        value: json!({
            "valid": violations.is_empty(),
            "violations": violations,
        }),
        flow_id: None,
    })
}

fn validate_calc_expression(args: &Value) -> Result<ToolOutcome, ToolError> {
    let expression = arg_str(args, "expression")?;
    let errors: Vec<String> = pnex_core::validate_calc(&expression)
        .into_iter()
        .map(|e| e.to_string())
        .collect();
    Ok(ToolOutcome {
        value: json!({ "valid": errors.is_empty(), "errors": errors }),
        flow_id: None,
    })
}

// ─────────────────── Write tools (drafts only) ───────────────────

async fn create_flow(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let name = arg_str(args, "name")?;
    let graph: pnex_core::FlowGraph = serde_json::from_value(arg_value(args, "graph")?.clone())
        .map_err(|e| format!("unreadable graph (FlowGraph expected): {e}"))?;
    let device_id = args.get("device_id").and_then(Value::as_i64);
    let (flow, version, _) = flow::create_flow(
        deps.db,
        deps.org_id,
        &name,
        &graph,
        device_id,
        deps.author.clone(),
        Some("created by the AI assistant".into()),
        &ASSISTANT_WRITER,
    )
    .await
    .map_err(flow_write_error_string)?;
    Ok(ToolOutcome {
        value: json!({
            "flow_id": flow.id,
            "name": flow.name,
            "status": flow.status,
            "version": version,
        }),
        flow_id: Some(flow.id),
    })
}

async fn update_flow(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let flow_id = arg_i64(args, "flow_id")?;
    let expected_version = arg_i64(args, "expected_version")?;
    let graph: pnex_core::FlowGraph = serde_json::from_value(arg_value(args, "graph")?.clone())
        .map_err(|e| format!("unreadable graph (FlowGraph expected): {e}"))?;
    let note = args.get("note").and_then(Value::as_str).map(str::to_string);
    // Org scoping: the flow must belong to the org.
    let Some(row) = flows::Entity::find_by_id(flow_id)
        .filter(flows::Column::OrgId.eq(deps.org_id))
        .one(deps.db)
        .await
        .map_err(internal("reading the flow"))?
    else {
        return Err(format!("flow #{flow_id} not found in this organization").into());
    };
    // D143: a running flow is frozen for the assistant. Checked here, at
    // execution time, from the stored status — never from the model's word.
    // Stopping stays a human gesture (no stop tool exists).
    if row.status == pnex_core::FLOW_STATUS_DEPLOYED {
        return Err(flow_running_error(&[(row.id, row.name.clone())]));
    }
    // Optimistic concurrency: the model edits the version it read with
    // get_flow; a human save in between → conflict rendered to the model,
    // which reloads instead of overwriting the human's change. The deployed
    // status is checked again under the write lock (deploy in between).
    flow::append_version_if_stopped(
        deps.db,
        &row,
        expected_version,
        &graph,
        None,
        deps.author.clone(),
        note.or_else(|| Some("changed by the AI assistant".into())),
        &ASSISTANT_WRITER,
    )
    .await
    .map(|(_, version, _)| ToolOutcome {
        value: json!({ "flow_id": row.id, "version": version }),
        flow_id: Some(row.id),
    })
    .map_err(|e| match e {
        flow::FlowWriteError::Deployed => flow_running_error(&[(row.id, row.name.clone())]),
        e => flow_write_error_string(e).into(),
    })
}

/// `ai-flow-running` refusal listing the deployed flows that block the
/// change (D143, D144); `args.flow` = their names for the UI.
pub(super) fn flow_running_error(flows: &[(i64, String)]) -> ToolError {
    let listed: Vec<String> = flows
        .iter()
        .map(|(id, name)| format!("#{id} « {name} »"))
        .collect();
    ToolError {
        message: format!(
            "{}: refused, deployed and running flow(s) {} — ask the user to stop them in the flow editor, then retry. You cannot stop, deploy or delete a flow.",
            pnex_core::err_codes::AI_FLOW_RUNNING,
            listed.join(", ")
        ),
        code: Some(pnex_core::err_codes::AI_FLOW_RUNNING),
        args: Some(json!({
            "flow": flows.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", "),
        })),
    }
}

/// Write errors → messages readable by the model (detailed
/// violations: the model corrects itself).
fn flow_write_error_string(e: flow::FlowWriteError) -> String {
    use crate::services::flow::FlowWriteError as E;
    match e {
        E::Violations(v) => format!(
            "invalid graph: {}",
            serde_json::to_string(&v).unwrap_or_default()
        ),
        E::NameRequired => "flow name required".into(),
        E::NameTooLong => "flow name too long (> 200 characters)".into(),
        E::DeviceUnknown => "device_id not found in this organization".into(),
        E::Conflict { current, .. } => format!(
            "version conflict: the flow is now at version {current} — reload it with get_flow, then retry"
        ),
        E::Db => "database error; tell the user to try again later".into(),
        E::Deployed => "the flow is deployed: it can only be changed once stopped".into(),
        E::Secret(e) => format!("secret field refused: {e}"),
    }
}

/// The assistant never types a secret value: it may only keep or pick
/// vault references (secrets.md D117).
const ASSISTANT_WRITER: crate::services::secrets::flow::GraphWriter<'static> =
    crate::services::secrets::flow::GraphWriter {
        ring: None,
        user_id: None,
        can_write_secrets: false,
    };

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry is **exactly** the allowed set: it is the structural
    /// assertion of the "no deploy / deletion / commands" safeguard.
    #[test]
    fn registry_contains_only_allowed_tools() {
        let names: Vec<&str> = tool_specs().iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            vec![
                "list_devices",
                "get_device_pins",
                "list_flows",
                "get_flow",
                "list_notifications",
                "query_telemetry",
                "describe_node_types",
                "search_knowledge",
                "read_knowledge",
                "diagnose_device",
                "diagnose_flow",
                "list_dashboards",
                "get_dashboard",
                "validate_dashboard_layout",
                "create_dashboard",
                "update_dashboard",
                "get_notification_template",
                "preview_notification_template",
                "create_notification_template",
                "update_notification_template",
                "list_functions",
                "get_function",
                "validate_function",
                "test_function",
                "create_function",
                "update_function",
                "list_annotation_sets",
                "list_tours",
                "list_pois",
                "list_controls",
                "read_memory",
                "list_taxonomies",
                "get_taxonomy",
                "create_taxonomy_version",
                "describe_ontology",
                "query_ontology",
                "aggregate_objects",
                "get_object",
                "case_workspace",
                "case_signals",
                "case_board",
                "case_proposals",
                "create_object",
                "update_object",
                "open_link",
                "close_link",
                "propose_action",
                "save_object_type",
                "list_calendar",
                "propose_occurrence_change",
                "market_figures",
                "market_rule_test",
                "market_session",
                "search_docs",
                "read_chunk",
                "open_page",
                "list_site_recipes",
                "get_site_recipe",
                "try_site_recipe",
                "get_site_recipe_run",
                "propose_site_recipe",
                "list_mailboxes",
                "list_mails",
                "read_mail",
                "validate_flow_graph",
                "validate_calc_expression",
                "create_flow",
                "update_flow",
            ]
        );
        // No name suggests a forbidden action — in-depth assertion
        // (a future "deploy_flow" added to tool_specs would break this test).
        for name in &names {
            for forbidden in [
                "deploy",
                "delete",
                "remove",
                "command",
                "write_gpio",
                "http",
                "sql_exec",
            ] {
                assert!(
                    !name.contains(forbidden),
                    "forbidden tool found in the registry: {name}"
                );
            }
        }
    }

    /// `execute` refuses anything that is not the registry — including
    /// tempting names ("deploy_flow", "run_sql", "http").
    #[tokio::test]
    async fn execute_refuses_names_outside_the_registry() {
        // Refused before any query: no database needed.
        let db = sea_orm::DatabaseConnection::default();
        let deps = ToolDeps {
            db: &db,
            org_id: 1,
            grants: crate::services::iam::Grants::managed(
                &crate::models::_entities::sea_orm_active_enums::OrgMemberRole::Owner,
            ),
            author: None,
            o2: None,
            config: None,
            app: None,
            actor: None,
            frozen_at: None,
        };
        for name in [
            "deploy_flow",
            "delete_flow",
            "delete_device",
            "send_device_command",
            "http",
            "run_sql",
        ] {
            let err = execute(&deps, name, &serde_json::json!({}))
                .await
                .expect_err("forbidden tool must fail");
            assert!(err.message.contains("unknown tool"), "{name} → {err:?}");
        }
        // The engine refuses a name absent from the IAM registry, before
        // any dispatch (SEC-79).
        let err = authorize_tool(&deps, "not_in_the_registry").expect_err("unknown tool");
        assert!(err.message.contains("unknown tool"), "{err:?}");
        // A simple read tool works without anything else.
        let out = execute(&deps, "describe_node_types", &serde_json::json!({}))
            .await
            .expect("static describe_node_types");
        assert!(out.value["nodes"].as_array().expect("nodes").len() >= 8);
    }

    /// The catalogue is the core table: every documented kind is served,
    /// and the `kinds` filter narrows it (the core guard checks the table
    /// against `FlowNodeKind`).
    #[test]
    fn catalogue_serves_the_core_node_docs() {
        let out = describe_node_types(&serde_json::json!({})).expect("catalogue");
        let nodes = out.value["nodes"].as_array().expect("nodes");
        assert_eq!(nodes.len(), pnex_core::NODE_DOCS.len());
        assert!(out.value["example"]["nodes"].is_array());

        let out = describe_node_types(&serde_json::json!({"kinds": ["calc", "reg_pid"]}))
            .expect("subset");
        let kinds: Vec<&str> = out.value["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .filter_map(|n| n["kind"].as_str())
            .collect();
        assert_eq!(kinds, vec!["calc", "reg_pid"]);

        let err = describe_node_types(&serde_json::json!({"kinds": ["device"]}))
            .expect_err("removed kind");
        assert!(err.message.contains("unknown node kind"), "{}", err.message);
    }

    /// Every `ai-trace-*` key a tool summary can emit exists in both UI
    /// locales (the UI falls back to the English summary otherwise, but a
    /// missing key is unfinished work), and every registry tool has its own
    /// summary arm (no silent `ai-trace-ok`).
    #[test]
    fn trace_summary_keys_exist_in_both_locales() {
        // Summaries live with their spec, in each domain module.
        let src = [
            include_str!("tools.rs"),
            include_str!("dashboard_tools.rs"),
            include_str!("more_tools.rs"),
            include_str!("ontology_tools.rs"),
            include_str!("calendar_tools.rs"),
            include_str!("market_tools.rs"),
            include_str!("doc_tools.rs"),
            include_str!("site_tools.rs"),
            include_str!("mail_tools.rs"),
        ]
        .concat();
        let src = src.as_str();
        let keys: std::collections::BTreeSet<&str> = src
            .match_indices("\"ai-trace-")
            .map(|(i, _)| {
                let rest = &src[i + 1..];
                &rest[..rest.find('"').unwrap()]
            })
            // The scan pattern itself is not a key.
            .filter(|k| *k != "ai-trace-")
            .collect();
        assert!(keys.len() > 30, "sterile scan: {keys:?}");
        let locales =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../pnex-frontend/locales");
        for locale in ["en-US.ftl", "fr-FR.ftl"] {
            let ftl = std::fs::read_to_string(locales.join(locale)).unwrap();
            for key in &keys {
                assert!(
                    ftl.lines().any(|l| l.starts_with(&format!("{key} ="))),
                    "{key} missing from {locale}"
                );
            }
        }
        let empty = ToolOutcome {
            value: json!({}),
            flow_id: None,
        };
        for spec in tool_specs() {
            assert_ne!(
                summarize(spec.name, &empty).key,
                "ai-trace-ok",
                "{} has no summary arm",
                spec.name
            );
        }
    }
}
