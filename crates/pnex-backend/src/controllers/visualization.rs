//! Telemetry reads (OpenObserve series) for the Visualisation page and the
//! dashboards, any org member, read-only. `catalog` and `series` answer
//! 503 `telemetry-unavailable` when OpenObserve is not configured, failing
//! or timed out; the dashboard queries (`series/query`, `aggregate/query`)
//! degrade per item (`available: false`) so one widget never fails the
//! others.

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::Json;
use loco_rs::prelude::*;
use serde::Deserialize;

use crate::auth::OrgContext;
use crate::services::openobserve::{Client, OpenobserveSettings};
use crate::services::visualization;

/// Max specs per batch (defensive: a dashboard stays around ten widgets,
/// each overlaying a few sources).
const BATCH_SPECS_CAP: usize = 24;

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/telemetry")
        .add("/catalog", get(catalog))
        .add("/series", get(series))
        .add("/series/query", post(series_batch))
        .add("/aggregate/query", post(aggregate))
}

#[utoipa::path(
    get,
    path = "/api/v1/telemetry/catalog",
    tag = "telemetry",
    operation_id = "telemetry_catalog",
    summary = "Catalog of the telemetry series",
    description = "Series of the org available in OpenObserve (metric x device, last value over 24 h) and label sets of device-less series, to feed the series pickers. Read-only, any org member.",
    responses(
        (status = 200, body = pnex_core::TelemetryCatalog),
        (status = 503, description = "OpenObserve not configured, failing or timed out (`telemetry-unavailable`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn catalog(org: OrgContext, State(ctx): State<AppContext>) -> Result<Response> {
    let client = OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s));
    let catalog = visualization::series_catalog(&ctx.db, client.as_ref(), org.org.id).await;
    if !catalog.available {
        return Err(unavailable());
    }
    format::json(catalog)
}

/// 503 `telemetry-unavailable` (D240: no `available: false` envelope).
fn unavailable() -> Error {
    crate::controllers::coded_error(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        pnex_core::err_codes::TELEMETRY_UNAVAILABLE,
        "Telemetry is unavailable (OpenObserve not configured, failing or timed out).",
        None,
    )
}

/// Parameters of `GET /series`, validated by the service (closed charset,
/// window preset) before any PromQL query is built.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SeriesParams {
    /// Metric name (closed charset).
    pub metric: String,
    pub device_id: String,
    /// Window preset (`viz::VIZ_WINDOW_PRESETS`: 5m, 15m, 30m, ...).
    pub window: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/telemetry/series",
    tag = "telemetry",
    operation_id = "telemetry_series",
    summary = "Points of one telemetry series",
    description = "Time series points of one metric on one device over a preset window (ascending, capped). Read-only, any org member.",
    params(SeriesParams),
    responses(
        (status = 200, body = pnex_core::TelemetrySeriesResponse),
        (status = 400, description = "Invalid metric, device_id or window (`field-invalid`)", body = crate::openapi::ErrorDetail),
        (status = 503, description = "OpenObserve not configured, failing or timed out (`telemetry-unavailable`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
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
    if !response.available {
        return Err(unavailable());
    }
    format::json(response)
}

/// `POST /api/v1/telemetry/series/query` (D31): every source of a dashboard
/// in one call. Degrades **per item**: never a global 400/500, even for
/// invalid specs (beyond the cap: 400, a caller error, not missing data).
#[utoipa::path(
    post,
    path = "/api/v1/telemetry/series/query",
    tag = "telemetry",
    operation_id = "telemetry_series_batch",
    summary = "Points of several telemetry series in one call",
    description = "Reads up to 24 series specs at once (a whole dashboard). Results come in the same order; an invalid or unserved spec degrades to `available: false` for its item only. Read-only, any org member.",
    request_body = pnex_core::SeriesBatchRequest,
    responses(
        (status = 200, body = pnex_core::SeriesBatchResponse),
        (status = 400, description = "More than 24 specs (`telemetry-batch-too-large`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn series_batch(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Json(body): Json<pnex_core::SeriesBatchRequest>,
) -> Result<Response> {
    if body.specs.len() > BATCH_SPECS_CAP {
        return Err(crate::controllers::coded_error(
            axum::http::StatusCode::BAD_REQUEST,
            pnex_core::err_codes::TELEMETRY_BATCH_TOO_LARGE,
            format!("At most {BATCH_SPECS_CAP} series specs per query."),
            Some(serde_json::json!({ "max": BATCH_SPECS_CAP.to_string() })),
        ));
    }
    let client = OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s));
    let response =
        visualization::series_batch(&ctx.db, client.as_ref(), org.org.id, &body.specs).await;
    format::json(response)
}

/// `POST /api/v1/telemetry/aggregate/query` (D182) — one value per time range or
/// time-of-day slice; every member reads. Invalid input = 400 with a field
/// token, O2 trouble = `available: false`, never a 500 from O2.
#[utoipa::path(
    post,
    path = "/api/v1/telemetry/aggregate/query",
    tag = "telemetry",
    operation_id = "telemetry_aggregate",
    summary = "Aggregate a series per time range or time-of-day slice",
    description = "One value per bucket (`sum`, `avg`, `max`, `min` or counter `increase`): buckets are either the time ranges of a scope met by `[from, to)` or time-of-day slices repeated over the days. OpenObserve trouble degrades to `available: false`. Read-only, any org member.",
    request_body = pnex_core::aggregate::AggregateRequest,
    responses(
        (status = 200, body = pnex_core::aggregate::AggregateResponse),
        (status = 400, description = "Invalid input: `field-invalid` with `errors.fields` (`body` when the shape is wrong)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn aggregate(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Json(body): Json<serde_json::Value>,
) -> Result<Response> {
    use crate::services::telemetry_aggregate::{self, AggregateError};
    let field = |f: &str, token: &str| Ok(crate::controllers::field_error(f, token));
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

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(catalog, series, series_batch, aggregate))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
