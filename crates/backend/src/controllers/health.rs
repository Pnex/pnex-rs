//! Probes santé — parité Django `health/views.py`, service renommé `pnex-server`.
//!
//! Phase 1 : `/health/live` complet ; `/health/ready` honnête sans DB
//! (le check PG arrive en Phase 2, le check cache deviendra OpenObserve).
//! Divergence assumée : pas de slash terminal (Django servait `/health/live/`).

use loco_rs::prelude::*;
use pnex_core::{HealthLive, HealthReady, SERVICE_NAME};

#[debug_handler]
async fn live() -> Result<Response> {
    format::json(HealthLive {
        status: "ok".to_string(),
        service: SERVICE_NAME.to_string(),
    })
}

#[debug_handler]
async fn ready() -> Result<Response> {
    // Phase 2 branchera le check PostgreSQL ; le « cache » Django (Redis,
    // non critique dans la réponse ready) devient OpenObserve en Phase 5.
    format::json(HealthReady {
        status: "ok".to_string(),
        database: "unconfigured".to_string(),
        cache: "not-applicable".to_string(),
    })
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/health/live", get(live))
        .add("/health/ready", get(ready))
}
