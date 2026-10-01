//! Health probes — inspired by the legacy POC, but the Rust version is now
//! the reference (no cosmetic parity, e.g. trailing slashes).
//!
//! `/health/ready` runs a real `SELECT 1` on the SeaORM pool (Phase 2)
//! and an OpenObserve check (Phase 5, replaces the legacy Redis check):
//! `ok` / `error` / `not-configured` (missing settings.openobserve
//! section — tests, deployments without telemetry).

use axum::extract::State;
use loco_rs::prelude::*;
use pnex_core::{HealthLive, HealthReady, SERVICE_NAME};
use sea_orm::ConnectionTrait;

use crate::services::openobserve;

#[debug_handler]
async fn live() -> Result<Response> {
    format::json(HealthLive {
        status: "ok".to_string(),
        service: SERVICE_NAME.to_string(),
    })
}

#[debug_handler]
async fn ready(State(ctx): State<AppContext>) -> Result<Response> {
    let database = match ctx.db.execute_unprepared("SELECT 1").await {
        Ok(_) => "ok".to_string(),
        Err(err) => {
            tracing::error!(%err, "health/ready : check PostgreSQL échoué");
            "error".to_string()
        }
    };

    let openobserve_status = match openobserve::OpenobserveSettings::from_config(&ctx.config) {
        Some(settings) => {
            let client = openobserve::Client::new(&settings);
            if client.healthy().await {
                "ok".to_string()
            } else {
                tracing::error!("health/ready : check OpenObserve échoué");
                "error".to_string()
            }
        }
        None => "not-configured".to_string(),
    };

    let degraded = database != "ok" || openobserve_status == "error";
    format::json(HealthReady {
        status: if degraded { "degraded" } else { "ok" }.to_string(),
        database,
        cache: openobserve_status,
    })
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/health/live", get(live))
        .add("/health/ready", get(ready))
}
