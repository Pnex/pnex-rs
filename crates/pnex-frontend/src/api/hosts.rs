//! Client API du référentiel des hosts serveur PNeX (`pnex_core::edge`) —
//! wizard device.

use crate::api::client;
use crate::api::error::ApiError;
use pnex_core::{Paginated, PnexHost, PnexHostInput};

/// `GET /api/v1/edge/hosts?limit=200` — référentiel de l'org (le picker ne
/// pagine pas).
pub async fn list() -> Result<Vec<PnexHost>, ApiError> {
    let page: Paginated<PnexHost> =
        client::request(reqwest::Method::GET, "/api/v1/edge/hosts?limit=200", None).await?;
    Ok(page.results)
}

/// `POST /api/v1/edge/hosts` — upsert (200 ou 201, même traitement).
pub async fn create(input: PnexHostInput) -> Result<PnexHost, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/edge/hosts",
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `PUT /api/v1/edge/hosts/{id}` — édition in-place (id conservé) :
/// host rename (always wss since D70). 409 when the target host is already
/// used by another entry of the org.
pub async fn update(id: i64, input: PnexHostInput) -> Result<PnexHost, ApiError> {
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/edge/hosts/{id}"),
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/edge/lan-scan` — scan LAN côté serveur : sondes meta sur
/// les /24 des interfaces privées (ou `prefix` explicite « a.b.c. ») et
/// retour des serveurs s'identifiant `pnex-server`.
pub async fn lan_scan(prefix: Option<&str>) -> Result<pnex_core::LanScanResult, ApiError> {
    // Un préfixe « a.b.c. » (chiffres + points) est URL-safe tel quel ;
    // tout le reste est rejeté par la garde serveur.
    let suffix = prefix
        .map(|p| format!("?prefix={}", p.trim()))
        .unwrap_or_default();
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/edge/lan-scan{suffix}"),
        None,
    )
    .await
}

/// `DELETE /api/v1/edge/hosts/{id}` — 204 sans corps (`request_opt` : un
/// `request` classique lèverait « réponse vide »).
pub async fn delete(id: i64) -> Result<(), ApiError> {
    client::request_opt::<()>(
        reqwest::Method::DELETE,
        &format!("/api/v1/edge/hosts/{id}"),
        None,
    )
    .await?;
    Ok(())
}

/// `GET /api/v1/edge/hosts/locked` — server host imposed by the deployment
/// (`PNEX_PROD_HOST`), `None` when users pick from the referential.
pub async fn locked() -> Result<Option<String>, ApiError> {
    let locked: pnex_core::LockedHost =
        client::request(reqwest::Method::GET, "/api/v1/edge/hosts/locked", None).await?;
    Ok(locked.host)
}
