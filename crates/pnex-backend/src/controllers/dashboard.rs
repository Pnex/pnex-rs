//! `GET /api/v1/dashboard/summary` — a single request for the front's
//! Dashboard page (one wasm request = one timer, one error path).
//!
//! Read-only: any org member (viewer included). The telemetry
//! section is degraded by design (`available: false` without O2)
//! — see services/dashboard.rs.

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

#[utoipa::path(
    get,
    path = "/api/v1/dashboard/summary",
    tag = "dashboard",
    operation_id = "dashboard_summary",
    summary = "Get the home dashboard summary",
    description = "One call for the home page: device liveness (counts + the ~10 most recent devices), build statistics and the latest telemetry samples. Read-only, any member. The telemetry section is degraded by design (`available: false` when OpenObserve is not configured or fails), the call itself does not fail because of it.",
    responses((status = 200, body = pnex_core::DashboardSummary)),
    security(("bearer" = [], "org" = []))
)]
async fn summary(org: OrgContext, State(ctx): State<AppContext>) -> Result<Response> {
    let ingest = IngestSettings::from_config(&ctx.config);
    let o2_client =
        OpenobserveSettings::from_config(&ctx.config).map(|settings| Client::new(&settings));

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

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(summary))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
