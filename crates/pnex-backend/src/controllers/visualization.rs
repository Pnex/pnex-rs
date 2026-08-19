//! `GET /api/v1/telemetry/catalog` et `GET /api/v1/telemetry/series` —
//! lecture des séries OpenObserve pour la page Visualisation (viewer
//! inclus, lecture seule). La branche O2 est dégradée par conception
//! (`available: false`), jamais de 500 — cf. services/visualization.rs.

use axum::extract::{Query, State};
use axum::routing::get;
use loco_rs::prelude::*;
use serde::Deserialize;

use crate::auth::OrgContext;
use crate::services::openobserve::{Client, OpenobserveSettings};
use crate::services::visualization;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/telemetry")
        .add("/catalog", get(catalog))
        .add("/series", get(series))
}

async fn catalog(org: OrgContext, State(ctx): State<AppContext>) -> Result<Response> {
    let client = OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s));
    format::json(visualization::series_catalog(&ctx.db, client.as_ref(), org.org.id).await)
}

/// Paramètres de `GET /series` — validés côté service (charset fermé,
/// fenêtre preset) avant toute construction de requête PromQL.
#[derive(Debug, Deserialize)]
pub struct SeriesParams {
    pub metric: String,
    pub device_id: String,
    pub window: String,
}

async fn series(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Query(params): Query<SeriesParams>,
) -> Result<Response> {
    let client = OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s));
    let response = visualization::series_points(
        &ctx.db,
        client.as_ref(),
        org.org.id,
        &params.metric,
        &params.device_id,
        &params.window,
    )
    .await?;
    format::json(response)
}
