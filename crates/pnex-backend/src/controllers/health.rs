//! Health probes (no trailing-slash route variants).
//!
//! `/health/ready` runs a real `SELECT 1` on the SeaORM pool (Phase 2)
//! and an OpenObserve check (Phase 5):
//! `ok` / `error` / `not_configured` (missing settings.openobserve
//! section — tests, deployments without telemetry).

use axum::extract::State;
use loco_rs::prelude::*;
use pnex_core::{HealthLive, HealthReady, HealthState, SERVICE_NAME};
use sea_orm::ConnectionTrait;

use crate::services::openobserve;

#[utoipa::path(
    get,
    path = "/health/live",
    tag = "health",
    operation_id = "health_live",
    security(()),
    summary = "Liveness probe",
    responses((status = 200, body = pnex_core::HealthLive))
)]
#[debug_handler]
async fn live() -> Result<Response> {
    format::json(HealthLive {
        status: HealthState::Ok,
        service: SERVICE_NAME.to_string(),
    })
}

#[utoipa::path(
    get,
    path = "/health/ready",
    tag = "health",
    operation_id = "health_ready",
    security(()),
    summary = "Readiness probe (database, telemetry)",
    responses((status = 200, body = pnex_core::HealthReady))
)]
#[debug_handler]
async fn ready(State(ctx): State<AppContext>) -> Result<Response> {
    let database = match ctx.db.execute_unprepared("SELECT 1").await {
        Ok(_) => HealthState::Ok,
        Err(err) => {
            tracing::error!(%err, "health/ready: PostgreSQL check failed");
            HealthState::Error
        }
    };

    let openobserve_status = match openobserve::OpenobserveSettings::from_config(&ctx.config) {
        Some(settings) => {
            let client = openobserve::Client::new(&settings);
            if client.healthy().await {
                HealthState::Ok
            } else {
                tracing::error!("health/ready: OpenObserve check failed");
                HealthState::Error
            }
        }
        None => HealthState::NotConfigured,
    };

    let degraded = database != HealthState::Ok || openobserve_status == HealthState::Error;
    format::json(HealthReady {
        status: if degraded {
            HealthState::Degraded
        } else {
            HealthState::Ok
        },
        database,
        cache: openobserve_status,
    })
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/health/live", get(live))
        .add("/health/ready", get(ready))
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(live, ready))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
