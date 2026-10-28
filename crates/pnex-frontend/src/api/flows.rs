//! ETL flow endpoints (D18) — versioned CRUD + deploy/rollback + runtime.
//!
//! The editor talks **only to the Loco API**, never to the runtime (PRD
//! safeguard, docs/architecture/flow-engine.md). Types come from `pnex-core`
//! (shared source of truth, wasm32); `update` carries optimistic
//! concurrency (`expected_version_number`, 409 if stale).

use pnex_core::{
    CreateFlow, DeployFlow, Flow, FlowDebugFeed, FlowRuntimeStatus, FlowSummary, FlowVersionDetail,
    FlowVersionSummary, FlowViolation, Paginated, UpdateFlow,
};

use crate::api::client;
use crate::api::error::ApiError;

/// Filters of `GET /api/v1/flows` + pagination (D14) — absent = server
/// defaults (limit 10, offset 0).
#[derive(Default)]
pub struct FlowFilters {
    /// Case-insensitive substring on `name`.
    pub search: Option<String>,
    /// `draft | deployed | stopped | error` — unknown value ignored by the server.
    pub status: Option<String>,
    /// D42 effective label filter (`name` or `name:value`).
    pub label: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl FlowFilters {
    fn to_query(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(v) = &self.search {
            parts.push(format!("search={}", urlencode(v)));
        }
        if let Some(v) = &self.status {
            parts.push(format!("status={}", urlencode(v)));
        }
        if let Some(v) = self.label.as_deref().filter(|v| !v.is_empty()) {
            parts.push(format!("label={}", urlencode(v)));
        }
        if let Some(v) = self.limit {
            parts.push(format!("limit={v}"));
        }
        if let Some(v) = self.offset {
            parts.push(format!("offset={v}"));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("?{}", parts.join("&"))
        }
    }
}

/// Minimal percent encoding (the front has no url crate) — same code
/// as `api/devices.rs`.
fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `GET /api/v1/flows` — flows of the current org, paginated envelope.
pub async fn list(filters: &FlowFilters) -> Result<Paginated<FlowSummary>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/flows{}", filters.to_query()),
        None,
    )
    .await
}

/// `POST /api/v1/flows` — creation (graph = version 1).
pub async fn create(params: CreateFlow) -> Result<Flow, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/flows",
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/flows/{id}` — detail (graph = latest version).
pub async fn detail(id: i64) -> Result<Flow, ApiError> {
    client::request(reqwest::Method::GET, &format!("/api/v1/flows/{id}"), None).await
}

/// `PATCH /api/v1/flows/{id}` — records a **new version**
/// (append-only); 409 if `expected_version_number` is stale, 400
/// `{"violations": […]}` if the graph is invalid.
pub async fn update(id: i64, params: UpdateFlow) -> Result<Flow, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/flows/{id}"),
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/flows/{id}` — flow + versions (cascade).
pub async fn delete(id: i64) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/flows/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

/// `GET /api/v1/flows/{id}/versions` — version ladder, desc.
pub async fn versions(
    id: i64,
    limit: i64,
    offset: i64,
) -> Result<Paginated<FlowVersionSummary>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/flows/{id}/versions?limit={limit}&offset={offset}"),
        None,
    )
    .await
}

/// `GET /api/v1/flows/{id}/versions/{n}` — historical graph.
pub async fn version(id: i64, version_number: i64) -> Result<FlowVersionDetail, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/flows/{id}/versions/{version_number}"),
        None,
    )
    .await
}

/// `POST /api/v1/flows/{id}/deploy` — projects **all** deployed flows
/// into `flows.json` then hot reload; absent `version_number` =
/// latest version. 503 `flow-runtime` if the engine is off/down.
/// Deploy response: the flow + the stale notify references detected
/// at projection (additive server field, D50 — non-blocking toast).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SkippedTab {
    #[allow(dead_code)]
    pub flow_id: i64,
    #[serde(default)]
    #[allow(dead_code)]
    pub code: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub message: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct DeployedFlow {
    // Typed server response (D50 contract); the editor reads
    // `deployed_version_number` (multi-user awareness), `stale_notify_nodes`
    // and `skipped_tabs` (non-blocking toasts).
    #[allow(dead_code)]
    #[serde(flatten)]
    pub flow: Flow,
    #[serde(default)]
    pub stale_notify_nodes: Vec<pnex_core::StaleNotifyNode>,
    /// Tabs of OTHER flows isolated at projection (unreadable graph) —
    /// the deploy of THIS flow still goes through.
    #[serde(default)]
    pub skipped_tabs: Vec<SkippedTab>,
}

pub async fn deploy(id: i64, version_number: Option<i64>) -> Result<DeployedFlow, ApiError> {
    let body = DeployFlow { version_number };
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/flows/{id}/deploy"),
        Some(serde_json::to_value(body).unwrap_or_default()),
    )
    .await
}

/// `POST /api/v1/flows/{id}/rollback` — deploy of an earlier version
/// (does not create a version).
pub async fn rollback(id: i64, version_number: i64) -> Result<Flow, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/flows/{id}/rollback"),
        Some(
            serde_json::to_value(DeployFlow {
                version_number: Some(version_number),
            })
            .unwrap_or_default(),
        ),
    )
    .await
}

/// `POST /api/v1/flows/{id}/stop` — removes the flow's tab from the artifact
/// (ack `flow_stopped`) then the DB goes `stopped`, deployed version
/// kept. 409 if not deployed, 503 if the engine does not ack.
pub async fn stop(id: i64) -> Result<serde_json::Value, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/flows/{id}/stop"),
        None,
    )
    .await
}

/// `POST /api/v1/flows/{id}/start` — resumes a `stopped` flow by
/// re-deploying **its deployed version**. 409 if not stopped or never
/// deployed.
pub async fn start(id: i64) -> Result<Flow, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/flows/{id}/start"),
        None,
    )
    .await
}

/// `POST /api/v1/flows/{id}/restart` — stop then start chained (two
/// reloads; from `stopped` ≡ start).
pub async fn restart(id: i64) -> Result<Flow, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/flows/{id}/restart"),
        None,
    )
    .await
}

/// `GET /api/v1/flows/{id}/runtime` — supervisor state as seen by the backend.
pub async fn runtime(id: i64) -> Result<FlowRuntimeStatus, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/flows/{id}/runtime"),
        None,
    )
    .await
}

/// `GET /api/v1/flows/{id}/debug` — feed of the debug panel (supervisor
/// memory ring). 403 outside dev/debug mode.
pub async fn debug(id: i64) -> Result<FlowDebugFeed, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/flows/{id}/debug"),
        None,
    )
    .await
}

/// `GET /api/v1/flows/{id}/node-status` — latest camera/vision node
/// statuses (D103), available in run mode too.
pub async fn node_status(id: i64) -> Result<FlowDebugFeed, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/flows/{id}/node-status"),
        None,
    )
    .await
}

/// Save failure, sorted for the UI: concurrency conflict (409,
/// "reload / overwrite" modal), invalid graph (400 violations,
/// highlighting), or other (verbatim toast).
#[derive(Debug, Clone, PartialEq)]
pub enum SaveError {
    Conflict { description: String },
    Invalid(Vec<FlowViolation>),
    Other(String),
}

/// Classifies an `ApiError` from `update()` — only a 409 or a 400
/// with a `violations` field has usable semantics.
pub fn classify_save_error(err: &ApiError) -> SaveError {
    match err.status {
        Some(409) => SaveError::Conflict {
            description: err.message.clone(),
        },
        Some(400) => {
            let violations = err
                .violations()
                .cloned()
                .and_then(|value| serde_json::from_value::<Vec<FlowViolation>>(value).ok());
            match violations {
                Some(violations) => SaveError::Invalid(violations),
                None => SaveError::Other(err.message.clone()),
            }
        }
        _ => SaveError::Other(err.message.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_to_query() {
        let mut f = FlowFilters::default();
        assert_eq!(f.to_query(), "");
        f.search = Some("pipeline température".into());
        f.status = Some("deployed".into());
        f.limit = Some(10);
        f.offset = Some(20);
        assert_eq!(
            f.to_query(),
            "?search=pipeline%20temp%C3%A9rature&status=deployed&limit=10&offset=20"
        );
    }

    #[test]
    fn conflict_409_recognized() {
        let err = ApiError::http(
            409,
            r#"{"error":"conflict","description":"version périmée : attendu 1, version courante 2 — rechargez la dernière version"}"#,
        );
        assert_eq!(
            classify_save_error(&err),
            SaveError::Conflict {
                description: "conflict : version périmée : attendu 1, version courante 2 — rechargez la dernière version".into(),
            }
        );
    }

    #[test]
    fn violations_400_extracted() {
        let err = ApiError::http(
            400,
            r#"{"error":"flow-invalid","errors":{"violations":[{"node_id":"n1","code":"readonly_sql","message":"mot-clé interdit en lecture seule : DELETE"}]}}"#,
        );
        match classify_save_error(&err) {
            SaveError::Invalid(violations) => {
                assert_eq!(violations.len(), 1);
                assert_eq!(violations[0].code, "readonly_sql");
                assert_eq!(violations[0].node_id.as_deref(), Some("n1"));
            }
            other => panic!("expected violations, got {other:?}"),
        }
    }

    #[test]
    fn other_errors_relayed() {
        assert_eq!(
            classify_save_error(&ApiError::new("réseau : timeout")),
            SaveError::Other("réseau : timeout".into())
        );
        // 400 without a violations field (never seen from the API, defensive).
        let err = ApiError::http(400, r#"{"detail":"boom"}"#);
        assert_eq!(classify_save_error(&err), SaveError::Other("boom".into()));
    }
}
