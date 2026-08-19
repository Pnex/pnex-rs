//! `GET /api/v1/dashboard/summary` — une seule requête pour la page
//! Dashboard du front (une requête wasm = un timer, un chemin d'erreur).
//!
//! Lecture seule : tout membre de l'org (viewer inclus). La section
//! télémétrie est dégradée par conception (`available: false` sans O2)
//! — cf. services/dashboard.rs.

use axum::extract::State;
use axum::routing::get;
use loco_rs::prelude::*;
use pnex_core::{DashboardSummary, TelemetrySummary};

use crate::auth::OrgContext;
use crate::services::dashboard;
use crate::services::openobserve::{Client, OpenobserveSettings};
use crate::services::settings::IngestSettings;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/dashboard")
        .add("/summary", get(summary))
}

async fn summary(org: OrgContext, State(ctx): State<AppContext>) -> Result<Response> {
    let ingest = IngestSettings::from_config(&ctx.config);
    let o2_client = OpenobserveSettings::from_config(&ctx.config)
        .map(|settings| Client::new(&settings));

    let (liveness, builds, telemetry) = tokio::join!(
        dashboard::liveness(&ctx.db, org.org.id, ingest.silence_ttl_secs),
        dashboard::build_stats(&ctx.db, org.org.id),
        async {
            match o2_client.as_ref() {
                Some(client) => dashboard::latest_measurements(&ctx.db, client, org.org.id).await,
                None => TelemetrySummary {
                    available: false,
                    latest: vec![],
                },
            }
        }
    );

    format::json(DashboardSummary {
        liveness: liveness?,
        builds: builds?,
        telemetry,
    })
}
