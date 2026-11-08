//! Hosted agents, their runs and artifacts (`controllers/agents.rs`, `agent_runs.rs`,
//! `agent_artifacts.rs`; agent-environments.md D731–D761). An artifact's content is only
//! ever downloaded, never shown.

use pnex_core::agent_runs::{AgentArtifact, AgentRun};
use pnex_core::agents::{Agent, AgentCreate, AgentRunStart, AgentToolInfo, AgentVersion};
use pnex_core::environments::{
    Environment, EnvironmentActivate, EnvironmentBuild, EnvironmentInput, EnvironmentScan,
    EnvironmentVersion, EnvironmentVersionInput, EnvironmentVersionSummary, IsolationInput,
    ScanPolicy,
};
use pnex_core::image_registries::{ImageRegistry, ImageRegistryInput, ImageRegistryTest};
use pnex_core::runners::RunnerJobKind;
use pnex_core::runners::{JoinTokenCreate, JoinTokenCreated, RunnerJobLogLine, RunnerView};
use pnex_core::Paginated;

use crate::api::client;
use crate::api::error::ApiError;
use crate::api::media::urlencode;

fn body<T: serde::Serialize>(input: &T) -> Result<serde_json::Value, ApiError> {
    serde_json::to_value(input).map_err(|e| ApiError::new(e.to_string()))
}

pub async fn agents(limit: i64, offset: i64) -> Result<Paginated<Agent>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/agents?limit={limit}&offset={offset}"),
        None,
    )
    .await
}

pub async fn create_agent(input: &AgentCreate) -> Result<Agent, ApiError> {
    client::request(reqwest::Method::POST, "/api/v1/agents", Some(body(input)?)).await
}

/// Every tool an internal agent may be given (D785), all pages.
pub async fn agent_tools() -> Result<Vec<AgentToolInfo>, ApiError> {
    let mut out = Vec::new();
    loop {
        let page: Paginated<AgentToolInfo> = client::request(
            reqwest::Method::GET,
            &format!("/api/v1/agents/tools?limit=100&offset={}", out.len()),
            None,
        )
        .await?;
        let done = page.results.is_empty() || out.len() + page.results.len() >= page.count as usize;
        out.extend(page.results);
        if done {
            return Ok(out);
        }
    }
}

/// Lets a tripped validator agent decide again (validation-gate.md D839).
pub async fn rearm_validator(id: &str) -> Result<Agent, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/agents/{id}/validator/rearm"),
        None,
    )
    .await
}

pub async fn agent(id: &str) -> Result<Agent, ApiError> {
    client::request(reqwest::Method::GET, &format!("/api/v1/agents/{id}"), None).await
}

/// Versions of an agent's record, newest first, each with its spec.
pub async fn agent_versions(id: &str) -> Result<Paginated<AgentVersion>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/agents/{id}/versions?limit=50&offset=0"),
        None,
    )
    .await
}

pub async fn delete_agent(id: &str) -> Result<(), ApiError> {
    client::request_opt::<()>(
        reqwest::Method::DELETE,
        &format!("/api/v1/agents/{id}"),
        None,
    )
    .await?;
    Ok(())
}

pub async fn start_run(agent: &str, input: &AgentRunStart) -> Result<AgentRun, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/agents/{agent}/runs"),
        Some(body(input)?),
    )
    .await
}

pub async fn runs(
    limit: i64,
    offset: i64,
    engine: Option<RunnerJobKind>,
) -> Result<Paginated<AgentRun>, ApiError> {
    let filter = engine
        .map(|e| format!("&engine={}", e.as_str()))
        .unwrap_or_default();
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/agent-runs?limit={limit}&offset={offset}{filter}"),
        None,
    )
    .await
}

pub async fn stop_run(id: &str) -> Result<AgentRun, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/agent-runs/{id}/stop"),
        None,
    )
    .await
}

/// Lines a journal dialog shows: the end of the journal, where a run or a build ends.
const JOURNAL_SHOWN: i64 = 2000;
/// Largest page the server serves.
const JOURNAL_PAGE: i64 = 100;

/// The last `JOURNAL_SHOWN` lines of a journal at `path`, oldest first, page after page.
async fn journal_tail(path: &str) -> Result<Vec<RunnerJobLogLine>, ApiError> {
    let page = |offset: i64| {
        let url = format!("{path}?limit={JOURNAL_PAGE}&offset={offset}");
        async move {
            client::request::<Paginated<RunnerJobLogLine>>(reqwest::Method::GET, &url, None).await
        }
    };
    let first = page(0).await?;
    let start = (first.count - JOURNAL_SHOWN).max(0);
    let mut lines = if start == 0 {
        first.results
    } else {
        Vec::new()
    };
    let mut offset = start + lines.len() as i64;
    while offset < first.count {
        let next = page(offset).await?.results;
        if next.is_empty() {
            break;
        }
        offset += next.len() as i64;
        lines.extend(next);
    }
    Ok(lines)
}

pub async fn run_logs(id: &str) -> Result<Vec<RunnerJobLogLine>, ApiError> {
    journal_tail(&format!("/api/v1/agent-runs/{id}/logs")).await
}

pub async fn run_outputs(id: &str) -> Result<Paginated<AgentArtifact>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/agent-runs/{id}/artifacts?limit=100&offset=0"),
        None,
    )
    .await
}

/// `POST /api/v1/agent-artifacts?name=` — the file as given, for a run to take.
pub async fn upload_artifact(name: &str, bytes: Vec<u8>) -> Result<AgentArtifact, ApiError> {
    client::request_upload(
        reqwest::Method::POST,
        &format!("/api/v1/agent-artifacts?name={}", urlencode(name)),
        bytes,
    )
    .await
}

pub async fn artifact_content(id: &str) -> Result<Vec<u8>, ApiError> {
    client::request_bytes(
        reqwest::Method::GET,
        &format!("/api/v1/agent-artifacts/{id}/content"),
    )
    .await
}

pub async fn environments() -> Result<Paginated<Environment>, ApiError> {
    client::request(
        reqwest::Method::GET,
        "/api/v1/environments?limit=100&offset=0",
        None,
    )
    .await
}

pub async fn set_isolation(
    environment: &str,
    input: &IsolationInput,
) -> Result<Environment, ApiError> {
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/environments/{environment}/isolation"),
        Some(body(input)?),
    )
    .await
}

pub async fn builds(environment: &str) -> Result<Paginated<EnvironmentBuild>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/environments/{environment}/builds?limit=20&offset=0"),
        None,
    )
    .await
}

pub async fn build_logs(environment: &str, build: &str) -> Result<Vec<RunnerJobLogLine>, ApiError> {
    journal_tail(&format!(
        "/api/v1/environments/{environment}/builds/{build}/logs"
    ))
    .await
}

pub async fn runners() -> Result<Paginated<RunnerView>, ApiError> {
    client::request(
        reqwest::Method::GET,
        "/api/v1/runners?limit=100&offset=0",
        None,
    )
    .await
}

pub async fn environment(id: &str) -> Result<Environment, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/environments/{id}"),
        None,
    )
    .await
}

pub async fn create_environment(input: &EnvironmentInput) -> Result<Environment, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/environments",
        Some(body(input)?),
    )
    .await
}

pub async fn versions(environment: &str) -> Result<Paginated<EnvironmentVersionSummary>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/environments/{environment}/versions?limit=20&offset=0"),
        None,
    )
    .await
}

pub async fn version(environment: &str, number: i32) -> Result<EnvironmentVersion, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/environments/{environment}/versions/{number}"),
        None,
    )
    .await
}

pub async fn save_version(
    environment: &str,
    input: &EnvironmentVersionInput,
) -> Result<EnvironmentVersionSummary, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/environments/{environment}/versions"),
        Some(body(input)?),
    )
    .await
}

pub async fn start_build(environment: &str, number: i32) -> Result<EnvironmentBuild, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/environments/{environment}/versions/{number}/builds"),
        None,
    )
    .await
}

pub async fn activate_build(
    environment: &str,
    build: &str,
    input: &EnvironmentActivate,
) -> Result<Environment, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/environments/{environment}/builds/{build}/activate"),
        Some(body(input)?),
    )
    .await
}

pub async fn build_scan(environment: &str, build: &str) -> Result<EnvironmentScan, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/environments/{environment}/builds/{build}/scan"),
        None,
    )
    .await
}

pub async fn scan_policy() -> Result<ScanPolicy, ApiError> {
    client::request(
        reqwest::Method::GET,
        "/api/v1/environments/scan-policy",
        None,
    )
    .await
}

pub async fn set_scan_policy(input: &ScanPolicy) -> Result<ScanPolicy, ApiError> {
    client::request(
        reqwest::Method::PUT,
        "/api/v1/environments/scan-policy",
        Some(body(input)?),
    )
    .await
}

pub async fn registries() -> Result<Paginated<ImageRegistry>, ApiError> {
    client::request(
        reqwest::Method::GET,
        "/api/v1/image-registries?limit=100&offset=0",
        None,
    )
    .await
}

/// Creates a registry, or updates `id` when given.
pub async fn save_registry(
    id: Option<&str>,
    input: &ImageRegistryInput,
) -> Result<ImageRegistry, ApiError> {
    let (method, path) = match id {
        Some(id) => (
            reqwest::Method::PUT,
            format!("/api/v1/image-registries/{id}"),
        ),
        None => (
            reqwest::Method::POST,
            "/api/v1/image-registries".to_string(),
        ),
    };
    client::request(method, &path, Some(body(input)?)).await
}

pub async fn test_registry(id: &str) -> Result<ImageRegistryTest, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/image-registries/{id}/test"),
        None,
    )
    .await
}

pub async fn create_join_token(input: &JoinTokenCreate) -> Result<JoinTokenCreated, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/runners/join-tokens",
        Some(body(input)?),
    )
    .await
}

pub async fn revoke_runner(id: &str) -> Result<(), ApiError> {
    client::request_opt::<()>(
        reqwest::Method::DELETE,
        &format!("/api/v1/runners/{id}"),
        None,
    )
    .await?;
    Ok(())
}
