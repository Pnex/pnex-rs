//! Client API des mélanges de fluides personnalisés — miroir de
//! `controllers/fluid_mixtures.rs`. Les types viennent de `pnex-core`
//! (wasm-safe) : `pnex_core::FluidMixture` / `FluidMixtureComposition`.

use serde::Deserialize;

use pnex_core::{FluidMixture, FluidMixtureInput};

use crate::api::client;
use crate::api::error::ApiError;
use crate::api::media::urlencode;

#[derive(Clone, Debug, Deserialize)]
pub struct FluidMixturePage {
    #[allow(dead_code)]
    pub count: i64,
    pub results: Vec<FluidMixture>,
}

/// `GET /api/v1/fluid-mixtures`.
pub async fn list_fluid_mixtures(search: Option<&str>) -> Result<FluidMixturePage, ApiError> {
    let q = match search {
        Some(s) if !s.trim().is_empty() => format!("?search={}", urlencode(s)),
        _ => String::new(),
    };
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/fluid-mixtures{q}"),
        None,
    )
    .await
}

/// `POST /api/v1/fluid-mixtures`.
pub async fn create_fluid_mixture(body: &FluidMixtureInput) -> Result<FluidMixture, ApiError> {
    let value = serde_json::to_value(body).map_err(|e| ApiError::new(e.to_string()))?;
    client::request(reqwest::Method::POST, "/api/v1/fluid-mixtures", Some(value)).await
}

/// `PUT /api/v1/fluid-mixtures/{id}` — remplacement complet.
pub async fn update_fluid_mixture(
    id: &str,
    body: &FluidMixtureInput,
) -> Result<FluidMixture, ApiError> {
    let value = serde_json::to_value(body).map_err(|e| ApiError::new(e.to_string()))?;
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/fluid-mixtures/{id}"),
        Some(value),
    )
    .await
}

/// `DELETE /api/v1/fluid-mixtures/{id}` — 204.
pub async fn delete_fluid_mixture(id: &str) -> Result<Option<()>, ApiError> {
    client::request_opt(
        reqwest::Method::DELETE,
        &format!("/api/v1/fluid-mixtures/{id}"),
        None,
    )
    .await
}
