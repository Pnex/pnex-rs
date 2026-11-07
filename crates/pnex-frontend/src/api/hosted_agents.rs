//! Hosted agents, their runs and artifacts (`controllers/agents.rs`, `agent_runs.rs`,
//! `agent_artifacts.rs`; agent-environments.md D731–D761). An artifact's content is only
//! ever downloaded, never shown.

use pnex_core::agent_runs::{AgentArtifact, AgentRun};
use pnex_core::agents::{Agent, AgentCreate, AgentRunStart};
use pnex_core::environments::{Environment, EnvironmentBuild, IsolationInput};
use pnex_core::runners::{RunnerJobLogLine, RunnerView};
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

pub async fn runs(limit: i64, offset: i64) -> Result<Paginated<AgentRun>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/agent-runs?limit={limit}&offset={offset}"),
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
