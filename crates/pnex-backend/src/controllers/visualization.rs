//! `GET /api/v1/telemetry/catalog` et `GET /api/v1/telemetry/series` —
//! lecture des séries OpenObserve pour la page Visualisation (viewer
//! inclus, lecture seule). La branche O2 est dégradée par conception
//! (`available: false`), jamais de 500 — cf. services/visualization.rs.

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::Json;
use loco_rs::prelude::*;
use serde::Deserialize;

use crate::auth::OrgContext;
use crate::services::openobserve::{Client, OpenobserveSettings};
use crate::services::visualization;

/// Specs max par batch (défensif — un dashboard SCADA V1 reste sous la
/// dizaine de widgets ; chaque widget peut superposer quelques sources).
const BATCH_SPECS_CAP: usize = 24;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/telemetry")
        .add("/catalog", get(catalog))
        .add("/series", get(series))
        .add("/series-batch", post(series_batch))
        .add("/aggregate", post(aggregate))
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

/// `POST /api/v1/telemetry/series-batch` (D31) — toutes les sources d'un
/// dashboard en un appel. Dégradation **par item** : jamais de 400/500
/// globale, même pour des specs invalides (au-delà du cap : 400, c'est
/// une erreur d'appel, pas une donnée manquante).
async fn series_batch(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Json(body): Json<pnex_core::SeriesBatchRequest>,
) -> Result<Response> {
    if body.specs.len() > BATCH_SPECS_CAP {
        return Err(Error::BadRequest(format!(
            "specs: au plus {BATCH_SPECS_CAP} par batch"
        )));
    }
    let client = OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s));
    let response =
        visualization::series_batch(&ctx.db, client.as_ref(), org.org.id, &body.specs).await;
    format::json(response)
}

/// `POST /api/v1/telemetry/aggregate` (D182) — one value per time range or
/// time-of-day slice; every member reads. Invalid input = 400 with a field
/// token, O2 trouble = `available: false`, never a 500 from O2.
async fn aggregate(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Json(body): Json<serde_json::Value>,
) -> Result<Response> {
    use crate::services::telemetry_aggregate::{self, AggregateError};
    let field = |f: &str, token: &str| {
        Ok((
            axum::http::StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({ f: token })),
        )
            .into_response())
    };
    // Parsed by hand: a bad shape answers 400 like the other field errors.
    let Ok(req) = serde_json::from_value::<pnex_core::aggregate::AggregateRequest>(body) else {
        return field("body", pnex_core::err_codes::FIELD_INVALID);
    };
    let client = OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s));
    match telemetry_aggregate::aggregate(&ctx.db, client.as_ref(), org.org.id, &req).await {
        Ok(response) => format::json(response),
        Err(AggregateError::Invalid { field: f, token }) => field(&f, &token),
        Err(AggregateError::Db(e)) => {
            tracing::error!(error = %e, "aggregate: database error");
            Err(Error::InternalServerError)
        }
    }
}
