//! Skills of hosted agents (`controllers/agent_skills.rs`, agent-environments.md D711).

use pnex_core::agent_skills::{
    AgentSkill, AgentSkillActivate, AgentSkillCreate, AgentSkillUpdate, AgentSkillVersion,
};
use pnex_core::agents::{Agent, AgentUpdate};
use pnex_core::Paginated;

use crate::api::client;
use crate::api::error::ApiError;

fn body<T: serde::Serialize>(input: &T) -> Result<serde_json::Value, ApiError> {
    serde_json::to_value(input).map_err(|e| ApiError::new(e.to_string()))
}

pub async fn list(limit: i64, offset: i64) -> Result<Paginated<AgentSkill>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/agent-skills?limit={limit}&offset={offset}"),
        None,
    )
    .await
}

pub async fn create(input: &AgentSkillCreate) -> Result<AgentSkill, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/agent-skills",
        Some(body(input)?),
    )
    .await
}

pub async fn get(id: &str) -> Result<AgentSkill, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/agent-skills/{id}"),
        None,
    )
    .await
}

pub async fn update(id: &str, input: &AgentSkillUpdate) -> Result<AgentSkill, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/agent-skills/{id}"),
        Some(body(input)?),
    )
    .await
}

pub async fn delete(id: &str) -> Result<(), ApiError> {
    client::request_opt::<()>(
        reqwest::Method::DELETE,
        &format!("/api/v1/agent-skills/{id}"),
        None,
    )
    .await?;
    Ok(())
}

pub async fn versions(id: &str) -> Result<Paginated<AgentSkillVersion>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/agent-skills/{id}/versions?limit=100&offset=0"),
        None,
    )
    .await
}

pub async fn activate(id: &str, input: &AgentSkillActivate) -> Result<AgentSkill, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/agent-skills/{id}/activate"),
        Some(body(input)?),
    )
    .await
}

/// A new version of an agent record, here to pin a skill (D711).
pub async fn update_agent(id: &str, input: &AgentUpdate) -> Result<Agent, ApiError> {
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/agents/{id}"),
        Some(body(input)?),
    )
    .await
}
