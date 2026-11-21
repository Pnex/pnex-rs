//! API DTOs: flow CRUD payloads, version summaries/details, runtime status
//! and the debug feed frames.

use super::*;
use serde::{Deserialize, Serialize};

// ───────────────────────────── API DTOs ─────────────────────────────

/// Lifecycle status of a flow: `draft` (never deployed), `deployed`
/// (running), `stopped` (stopped on purpose, deployed version kept for a
/// restart). Stored as text (`flows.status`), see [`FLOW_STATUS_DRAFT`]…
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum FlowStatus {
    #[default]
    Draft,
    Deployed,
    Stopped,
}

impl FlowStatus {
    /// Wire/database text of the status.
    pub fn as_str(self) -> &'static str {
        match self {
            FlowStatus::Draft => FLOW_STATUS_DRAFT,
            FlowStatus::Deployed => FLOW_STATUS_DEPLOYED,
            FlowStatus::Stopped => FLOW_STATUS_STOPPED,
        }
    }

    /// Parses the stored text; an unknown value reads as `draft`.
    pub fn from_db(s: &str) -> Self {
        match s {
            FLOW_STATUS_DEPLOYED => FlowStatus::Deployed,
            FLOW_STATUS_STOPPED => FlowStatus::Stopped,
            _ => FlowStatus::Draft,
        }
    }
}

/// Engine health of one flow, as last announced by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum FlowEngineStatus {
    Running,
    Error,
    Stopped,
}

/// Flow summary (paginated list, without the graph).
/// `PartialEq` is required by the CRUD component props (dioxus 0.7
/// `#[component]` derives a per-field impl).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FlowSummary {
    pub id: i64,
    pub org_id: i64,
    pub device_id: Option<i64>,
    pub name: String,
    pub status: FlowStatus,
    pub deployed_version_number: Option<i64>,
    pub latest_version_number: i64,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// A flow with the graph of its latest saved version.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Flow {
    pub id: i64,
    pub org_id: i64,
    pub device_id: Option<i64>,
    pub name: String,
    pub status: FlowStatus,
    pub deployed_version_number: Option<i64>,
    pub latest_version_number: i64,
    pub graph: FlowGraph,
    /// Stored nodes left out of `graph` because they no longer read
    /// (`flow-node-retired`, `flow-node-unreadable`); saving drops them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable_nodes: Vec<FlowViolation>,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// Version summary (paginated history, without the graph).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FlowVersionSummary {
    pub id: i64,
    pub version_number: i64,
    pub author: Option<String>,
    pub note: Option<String>,
    pub deployed: bool,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
}

/// One version with its graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FlowVersionDetail {
    pub id: i64,
    pub version_number: i64,
    pub author: Option<String>,
    pub note: Option<String>,
    pub deployed: bool,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    pub graph: FlowGraph,
    /// Stored nodes left out of `graph` because they no longer read
    /// (`flow-node-retired`, `flow-node-unreadable`); saving drops them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable_nodes: Vec<FlowViolation>,
}

/// Creation: the flow and its version 1 in one transaction. `Serialize`
/// serves the frontend (the editor builds the typed request).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateFlow {
    pub name: String,
    #[serde(default)]
    pub device_id: Option<i64>,
    pub graph: FlowGraph,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Saves a new version (append-only). Optimistic concurrency:
/// `expected_version_number` must be the current version, else 409 — two
/// editors never overwrite each other.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateFlow {
    pub expected_version_number: i64,
    pub graph: FlowGraph,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Explicit deployment of a version (projection + runtime reload).
/// Absent `version_number` = latest version.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DeployFlow {
    #[serde(default)]
    pub version_number: Option<i64>,
}

/// Flow runtime status seen by the backend supervisor: process health
/// (`runtime.json`) crossed with the health of THIS flow (runtime events
/// `flow_started`/`flow_error`/`flow_stopped`). `deployed_flow_id` and
/// `deployed_version_number` are filled by the handler for the requested
/// flow, from the database.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FlowRuntimeStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub restarts: u64,
    pub deployed_flow_id: Option<i64>,
    pub deployed_version_number: Option<i64>,
    /// Engine state of this flow; `null` = unknown (the runtime has not
    /// announced it yet since the backend started).
    #[serde(default)]
    pub engine_status: Option<FlowEngineStatus>,
    /// Last real engine error of this flow (from the runtime `flow_error`
    /// event), cleared by its next `flow_started`.
    #[serde(default)]
    pub last_error: Option<String>,
    /// Debug tools enabled (`settings.flow.debug_tools`, dev/debug mode
    /// only; in run mode the debug panel answers 403 and is hidden).
    #[serde(default)]
    pub debug_tools: bool,
    /// Latest saved version of this flow (DB). Lets an open editor notice a
    /// version saved by another user without reloading the detail.
    #[serde(default)]
    pub latest_version_number: Option<i64>,
    /// DB status of this flow: an open editor refreshes its runtime
    /// controls when another user changed it.
    #[serde(default)]
    pub flow_status: Option<FlowStatus>,
}

/// One entry of the debug panel (in-memory ring of the supervisor, fed by
/// the runtime stdout `debug` events attributed to a flow).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FlowDebugEntry {
    /// Global increasing order (backend process clock).
    pub seq: u64,
    /// RFC 3339 timestamp (backend clock at reception).
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub ts: String,
    pub flow_id: i64,
    /// Editor id of the emitting node (`"n2"`, never the engine hash).
    pub node_id: String,
    /// Node name, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Captured value, raw: an object for `pnex-display`, a pre-stringified
    /// string for the builtin `debug` (the client tries to re-parse it).
    pub msg: serde_json::Value,
    /// `"debug"` (builtin) or `"pnex-display"` (probe).
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msgid: Option<String>,
}

/// Answer of `GET /flows/{id}/debug`: the panel feed, oldest first.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FlowDebugFeed {
    pub flow_id: i64,
    pub entries: Vec<FlowDebugEntry>,
}
