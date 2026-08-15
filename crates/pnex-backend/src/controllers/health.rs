//! Probes santé — inspirés du Django POC, mais la version Rust fait désormais
//! référence (pas de parité cosmétique, ex. slashs terminaux).
//!
//! `/health/ready` exécute un `SELECT 1` réel sur le pool SeaORM depuis la
//! Phase 2. Le « cache » Django (Redis, non critique) deviendra un check
//! OpenObserve en Phase 5.

use axum::extract::State;
use loco_rs::prelude::*;
use pnex_core::{HealthLive, HealthReady, SERVICE_NAME};
use sea_orm::ConnectionTrait;

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

    format::json(HealthReady {
        status: if database == "ok" { "ok" } else { "degraded" }.to_string(),
        database,
        // Phase 5 : check OpenObserve (remplace le check Redis Django).
        cache: "not-applicable".to_string(),
    })
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/health/live", get(live))
        .add("/health/ready", get(ready))
}
