//! Data governance (governance.md D798–D806): markings, clearances, the markings
//! of a table or an object, the journal and the export of an object.

use pnex_core::datasets::Dataset;
use pnex_core::governance::{
    Clearance, ClearanceInput, Declassify, Marking, MarkingInput, MarkingsAdd,
};
use reqwest::Method;

use super::client;
use super::error::ApiError;

fn body<T: serde::Serialize>(v: &T) -> Option<serde_json::Value> {
    Some(serde_json::to_value(v).unwrap_or_default())
}

pub async fn markings() -> Result<Vec<Marking>, ApiError> {
    client::request_all("/api/v1/governance/markings").await
}

pub async fn create_marking(input: &MarkingInput) -> Result<Marking, ApiError> {
    client::request(Method::POST, "/api/v1/governance/markings", body(input)).await
}

pub async fn delete_marking(id: &str) -> Result<(), ApiError> {
    let path = format!("/api/v1/governance/markings/{id}");
    client::request_opt::<serde_json::Value>(Method::DELETE, &path, None)
        .await
        .map(|_| ())
}

pub async fn clearances() -> Result<Vec<Clearance>, ApiError> {
    client::request_all("/api/v1/governance/clearances").await
}

pub async fn grant(input: &ClearanceInput) -> Result<Clearance, ApiError> {
    client::request(Method::POST, "/api/v1/governance/clearances", body(input)).await
}

pub async fn revoke(id: &str) -> Result<(), ApiError> {
    let path = format!("/api/v1/governance/clearances/{id}/revoke");
    client::request_opt::<serde_json::Value>(Method::POST, &path, None)
        .await
        .map(|_| ())
}

/// The CSV of the last 30 days of the journal.
pub async fn journal() -> Result<Vec<u8>, ApiError> {
    client::request_bytes(Method::GET, "/api/v1/governance/journal").await
}

pub async fn mark_dataset(id: &str, keys: Vec<String>) -> Result<Dataset, ApiError> {
    let path = format!("/api/v1/datasets/{id}/markings");
    client::request(Method::POST, &path, body(&MarkingsAdd { markings: keys })).await
}

pub async fn declassify_dataset(id: &str, input: &Declassify) -> Result<Dataset, ApiError> {
    let path = format!("/api/v1/datasets/{id}/declassify");
    client::request(Method::POST, &path, body(input)).await
}

pub async fn object_markings(id: &str) -> Result<Vec<String>, ApiError> {
    let path = format!("/api/v1/ontology/objects/{id}/markings");
    client::request::<MarkingsAdd>(Method::GET, &path, None)
        .await
        .map(|m| m.markings)
}

pub async fn mark_object(id: &str, keys: Vec<String>) -> Result<Vec<String>, ApiError> {
    let path = format!("/api/v1/ontology/objects/{id}/markings");
    client::request::<MarkingsAdd>(Method::POST, &path, body(&MarkingsAdd { markings: keys }))
        .await
        .map(|m| m.markings)
}

pub async fn declassify_object(id: &str, input: &Declassify) -> Result<Vec<String>, ApiError> {
    let path = format!("/api/v1/ontology/objects/{id}/declassify");
    client::request::<MarkingsAdd>(Method::POST, &path, body(input))
        .await
        .map(|m| m.markings)
}

/// Everything held about an object, as the JSON file to hand over (D806).
pub async fn export_object(id: &str) -> Result<Vec<u8>, ApiError> {
    client::request_bytes(
        Method::GET,
        &format!("/api/v1/ontology/objects/{id}/export"),
    )
    .await
}
