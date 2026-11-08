//! Read-only SQL queries (analysis.md D825–D828).

use pnex_core::analysis::{AnalysisQuery, AnalysisQueryInput};
use reqwest::Method;

use super::client;
use super::error::ApiError;

pub async fn submit(sql: &str) -> Result<AnalysisQuery, ApiError> {
    let body = AnalysisQueryInput {
        sql: sql.to_string(),
    };
    let body = Some(serde_json::to_value(body).unwrap_or_default());
    client::request(Method::POST, "/api/v1/analysis/queries", body).await
}

pub async fn get(id: &str) -> Result<AnalysisQuery, ApiError> {
    client::request(Method::GET, &format!("/api/v1/analysis/queries/{id}"), None).await
}
