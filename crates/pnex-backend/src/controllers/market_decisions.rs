//! Bars and decisions of an external model (markets-research.md appendix G,
//! D891–D896). The runner is a service account: it reads bars and posts
//! decisions, the only write open to it. Advice only: nothing here orders.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::markets::decisions::{
    self as dec, BarOut, Barriers, Decision, ModelScores, NewDecision, ReliabilityBin, Scores,
};
use pnex_core::rca::{Chain, Hop, Signal, SignalKind};
use pnex_core::Paginated;
use serde::Deserialize;

use super::{coded_error, field_error};
use crate::auth::OrgContext;
use crate::controllers::pagination::{self, PageParams};
use crate::services::markets::decisions::{self as svc, BarsError, BarsQuery, CreateError, Filter};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/markets")
        .add("/bars", get(bars))
        .add("/chains", get(chains))
        .add("/decisions", get(list).post(create))
        .add("/decisions/scores", get(scores))
        .add("/decisions/forecast-scores", get(forecast_scores))
        .add("/prediction-features", get(prediction_features))
        .add("/account-trades", get(account_trades))
        .add(
            "/account-trades/import",
            post(import_account_trades)
                .layer(axum::extract::DefaultBodyLimit::max(IMPORT_MAX_BYTES * 2)),
        )
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(
            bars,
            chains,
            create,
            list,
            scores,
            forecast_scores,
            prediction_features,
            account_trades,
            import_account_trades
        ),
        components(schemas(
            BarOut,
            Chain,
            Hop,
            Signal,
            SignalKind,
            NewDecision,
            Decision,
            Barriers,
            ModelScores,
            Scores,
            ReliabilityBin,
            pnex_core::markets::prediction::Forecast,
            pnex_core::markets::prediction::ForecastScores,
            crate::services::markets::accounts::AccountTrade,
            crate::services::markets::accounts::ImportResult,
            pnex_core::markets::prediction::CalibrationBin,
            pnex_core::markets::prediction::MarketFeature,
            pnex_core::markets::prediction::Features
        ))
    )]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn instant(field: &str, raw: Option<&str>) -> std::result::Result<Option<DateTime<Utc>>, Response> {
    match raw.filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => DateTime::parse_from_rfc3339(s)
            .map(|t| Some(t.with_timezone(&Utc)))
            .map_err(|_| field_error(field, err_codes::FIELD_INVALID)),
    }
}

fn bars_error(e: BarsError) -> Result<Response> {
    match e {
        BarsError::TooManyFiles => Err(coded_error(
            StatusCode::BAD_REQUEST,
            err_codes::MARKET_BARS_TOO_MANY_FILES,
            "This read would open too many files: narrow the period.",
            Some(serde_json::json!({ "max": svc::FILES_MAX })),
        )),
        BarsError::Dataset(e) => super::datasets::dataset_error(e),
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct BarsParams {
    /// Instrument as written in the table (`XAUUSD`, `BTCUSDT`…).
    instrument: String,
    /// `source` column: `histdata`, `binance_public` or `mt5:<broker>`. Never mixed.
    source: String,
    /// `1m`, `5m`, `15m`, `1h`, `4h` or `1d`, aligned on 1970-01-01 UTC.
    interval: String,
    /// First bar open (RFC 3339); without it, the last `limit` bars.
    #[param(format = DateTime)]
    from: Option<String>,
    /// Bars opening before this instant (RFC 3339).
    #[param(format = DateTime)]
    to: Option<String>,
    /// Bars returned, 1 to 5000; 500 by default.
    limit: Option<i64>,
    /// Instant of the read (RFC 3339), now by default: only bars closed then.
    #[param(format = DateTime)]
    as_of: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/markets/bars",
    tag = "markets",
    operation_id = "markets_bars",
    summary = "Read the price bars of an instrument as known at a date",
    description = "Bars of one instrument and one source, resampled from the minute bars of the price tables, oldest first. A bar is known at its close: at `as_of` (now by default) the bar in progress is never returned. A minute written twice counts once. Requires `datasets:read`, and a clearance for each marking of the table. Raw prices, never adjusted.",
    params(BarsParams),
    responses(
        (status = 200, body = Paginated<BarOut>),
        (status = 400, description = "Invalid parameter (field → token) or `market-bars-too-many-files`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied` or `marking-required`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`dataset-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn bars(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<BarsParams>,
) -> Result<Response> {
    let Some(interval_us) = dec::interval_us(&q.interval) else {
        return Ok(field_error("interval", err_codes::FIELD_INVALID));
    };
    if dec::bar_table(&q.source).is_none() {
        return Ok(field_error("source", err_codes::FIELD_INVALID));
    }
    if q.instrument.is_empty() || q.instrument.len() > 64 {
        return Ok(field_error("instrument", err_codes::FIELD_INVALID));
    }
    let limit = q.limit.unwrap_or(500);
    if !(1..=dec::BARS_MAX as i64).contains(&limit) {
        return Ok(field_error("limit", "range:1..5000"));
    }
    let (from, to, as_of) = match (
        instant("from", q.from.as_deref()),
        instant("to", q.to.as_deref()),
        instant("as_of", q.as_of.as_deref()),
    ) {
        (Ok(a), Ok(b), Ok(c)) => (a, b, c),
        (Err(r), _, _) | (_, Err(r), _) | (_, _, Err(r)) => return Ok(r),
    };
    let empty = || {
        format::json(Paginated::<BarOut> {
            count: 0,
            next: None,
            previous: None,
            results: Vec::new(),
        })
    };
    let Some(table) = svc::table_of(&ctx.db, org.org.id, &q.source).await? else {
        return empty();
    };
    if let Some(refused) = super::datasets::governed(&ctx, &org, table, "bars").await {
        return refused;
    }
    let store = match crate::services::datasets::store_of(&ctx.config) {
        Ok(s) => s,
        Err(e) => return super::datasets::dataset_error(e),
    };
    let query = BarsQuery {
        instrument: &q.instrument,
        source: &q.source,
        interval_us,
        from_us: from.map(|t| t.timestamp_micros()),
        to_us: to.map(|t| t.timestamp_micros()),
        limit: limit as usize,
        as_of_us: as_of.unwrap_or_else(Utc::now).timestamp_micros(),
    };
    match svc::bars(&ctx.db, &store, org.org.id, table, &query).await {
        Ok(rows) => format::json(Paginated {
            count: rows.len() as i64,
            next: None,
            previous: None,
            results: rows.iter().map(svc::bar_out).collect::<Vec<_>>(),
        }),
        Err(e) => bars_error(e),
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ChainsParams {
    /// Ticker of the `instrument` object, as written in the price table.
    instrument: String,
    /// Price source (`source` column): `histdata`, `binance_public` or `mt5:<broker>`.
    source: String,
    /// Instant of the read (RFC 3339), now by default: everything as known then.
    #[param(format = DateTime)]
    as_of: Option<String>,
    /// First instant of the price moves explained (RFC 3339); 7 days before
    /// `as_of` by default, 31 days at most.
    #[param(format = DateTime)]
    from: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/markets/chains",
    tag = "markets",
    operation_id = "markets_chains",
    summary = "Read the candidate causal chains of an instrument's price moves as known at a date",
    description = "For each 1-hour price move of the instrument beyond its threshold in [`from`, `as_of`], the 3 best candidate chains from a world event, a supply disruption or a macro release, through the links a member declared as propagating (`exposed_to`), scored by the pairs of the installed packs, each with its placebo and lift (never filtered: an input of an external model). Computed at reading time, never stored, never a cause (D742). Everything as known at `as_of`. Requires `datasets:read` and `ontology:read`.",
    params(ChainsParams, pagination::PageQuery),
    responses(
        (status = 200, body = Paginated<Chain>),
        (status = 400, description = "Invalid parameter (field → token: `instrument` `unknown` when no instrument object has this ticker, `from` `range:31d`) or `market-bars-too-many-files`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied` or `marking-required`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`dataset-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn chains(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ChainsParams>,
    Query(pq): Query<pagination::PageQuery>,
) -> Result<Response> {
    use crate::services::markets::chains::{self as ch, ChainsError, ChainsQuery};
    if !org.can("ontology:read") {
        return Err(crate::auth::access_denied("ontology:read"));
    }
    if dec::bar_table(&q.source).is_none() {
        return Ok(field_error("source", err_codes::FIELD_INVALID));
    }
    if q.instrument.is_empty() || q.instrument.len() > 64 {
        return Ok(field_error("instrument", err_codes::FIELD_INVALID));
    }
    let (as_of, from) = match (
        instant("as_of", q.as_of.as_deref()),
        instant("from", q.from.as_deref()),
    ) {
        (Ok(a), Ok(f)) => (a.unwrap_or_else(Utc::now), f),
        (Err(r), _) | (_, Err(r)) => return Ok(r),
    };
    let from = from.unwrap_or(as_of - chrono::Duration::days(ch::WINDOW_DEFAULT_DAYS));
    if from > as_of || as_of - from > chrono::Duration::days(ch::WINDOW_MAX_DAYS) {
        return Ok(field_error("from", "range:31d"));
    }
    let page = PageParams::from(pq.limit.as_deref(), pq.offset.as_deref());
    let mut filters = vec![
        ("instrument".to_string(), q.instrument.clone()),
        ("source".to_string(), q.source.clone()),
        ("as_of".to_string(), as_of.to_rfc3339()),
        ("from".to_string(), from.to_rfc3339()),
    ];
    filters.sort();
    let Some(table) = svc::table_of(&ctx.db, org.org.id, &q.source).await? else {
        return pagination::paged(
            "/api/v1/markets/chains",
            &filters,
            page,
            Vec::<Chain>::new(),
        );
    };
    if let Some(refused) = super::datasets::governed(&ctx, &org, table, "bars").await {
        return refused;
    }
    let store = match crate::services::datasets::store_of(&ctx.config) {
        Ok(s) => s,
        Err(e) => return super::datasets::dataset_error(e),
    };
    let query = ChainsQuery {
        instrument: &q.instrument,
        source: &q.source,
        table,
        from,
        as_of,
    };
    // A chain touching an object the caller is not cleared for (marking,
    // governance.md D801) is left out whole: its ids would reveal it.
    let hidden = match super::ontology::redaction(&ctx, &org).await {
        Ok(r) => r.objects,
        Err(e) => {
            tracing::error!(error = %e, "markings read failed");
            return Err(Error::InternalServerError);
        }
    };
    let visible = |s: &Signal| {
        hidden.as_ref().is_none_or(|h| {
            !h.contains(&s.on)
                && s.source_ref
                    .strip_prefix("object:")
                    .is_none_or(|id| !h.contains(id))
        })
    };
    match ch::chains(&ctx.db, &store, org.org.id, &query).await {
        Ok(mut rows) => {
            rows.retain(|c| {
                c.hops
                    .iter()
                    .all(|h| visible(&h.cause) && visible(&h.effect))
            });
            pagination::paged("/api/v1/markets/chains", &filters, page, rows)
        }
        Err(ChainsError::UnknownInstrument) => Ok(field_error("instrument", "unknown")),
        Err(ChainsError::Bars(e)) => bars_error(e),
        Err(ChainsError::Db(e)) => Err(e.into()),
        Err(ChainsError::Onto(e)) => {
            tracing::error!(error = %e, "causal pairs read failed");
            Err(Error::InternalServerError)
        }
    }
}

/// Field defects of a posted decision, `None` when it may be stored.
fn defects(d: &NewDecision, now: DateTime<Utc>) -> Option<Response> {
    let mut fields = serde_json::Map::new();
    let mut bad = |f: &str, token: &str| {
        fields.entry(f.to_string()).or_insert(token.into());
    };
    for (f, v, max) in [
        ("model", &d.model, 64),
        ("model_version", &d.model_version, 64),
        ("instrument", &d.instrument, 64),
    ] {
        if v.trim().is_empty() {
            bad(f, err_codes::FIELD_REQUIRED);
        } else if v.len() > max {
            bad(f, "max_length:64");
        }
    }
    if dec::bar_table(&d.source).is_none() {
        bad("source", err_codes::FIELD_INVALID);
    }
    if dec::interval_us(&d.interval).is_none() {
        bad("interval", err_codes::FIELD_INVALID);
    }
    if !(1..=10_000).contains(&d.horizon_bars) {
        bad("horizon_bars", "range:1..10000");
    }
    if d.options != dec::OPTIONS {
        bad("options", err_codes::FIELD_INVALID);
    }
    if let Some(token) = dec::probabilities_defect(&d.probabilities) {
        bad("probabilities", token);
    }
    if let Some(b) = d.barriers {
        let ok = |x: f64| x.is_finite() && x > 0.0 && x <= 100.0;
        if !ok(b.take_profit_atr) || !ok(b.stop_atr) || !(1..=500).contains(&b.atr_period) {
            bad("barriers", err_codes::FIELD_INVALID);
        }
    }
    if d.inputs
        .as_ref()
        .is_some_and(|v| v.to_string().len() > dec::INPUTS_MAX)
    {
        bad("inputs", "max_bytes:4096");
    }
    if let Some((field, token)) = pnex_core::markets::prediction::forecast_defect(&d.forecasts) {
        bad(&field, &token);
    }
    match DateTime::parse_from_rfc3339(&d.as_of) {
        Err(_) => bad("as_of", err_codes::FIELD_INVALID),
        Ok(t) if t.with_timezone(&Utc) > now => bad("as_of", "future"),
        Ok(_) => {}
    }
    (!fields.is_empty()).then(|| super::fields_error(StatusCode::BAD_REQUEST, fields))
}

#[utoipa::path(
    post,
    path = "/api/v1/markets/decisions",
    tag = "markets",
    operation_id = "markets_decisions_create",
    summary = "Post a decision of an external model",
    description = "Stores one decision (probabilities of `buy`, `neutral`, `sell`) of a model run outside PNeX, about data read at `as_of`. The server checks it and computes `best` and `confidence`. One decision per model, version, instrument, interval and `as_of`: the same one again answers 200, a different one 409. Advice only: nothing is ordered. Requires `market-decisions:write`, the only write a service account may make.",
    request_body = NewDecision,
    responses(
        (status = 201, body = Decision),
        (status = 200, description = "The very same decision was already stored", body = Decision),
        (status = 400, description = "Invalid field (field → token) or `market-decision-stale`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`market-decision-conflict`", body = crate::openapi::ErrorDetail),
        (status = 429, description = "`market-decisions-quota`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(d): Json<NewDecision>,
) -> Result<Response> {
    let now = Utc::now();
    if let Some(r) = defects(&d, now) {
        return Ok(r);
    }
    let as_of = DateTime::parse_from_rfc3339(&d.as_of)
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or(now);
    let iv = dec::interval_us(&d.interval).unwrap_or_default();
    if now.timestamp_micros() - as_of.timestamp_micros() > 2 * iv {
        return Err(coded_error(
            StatusCode::BAD_REQUEST,
            err_codes::MARKET_DECISION_STALE,
            "The decision reads data older than two intervals.",
            Some(serde_json::json!({ "interval": d.interval })),
        ));
    }
    if !svc::known_instrument(&ctx.db, org.org.id, &d.instrument).await? {
        return Ok(field_error("instrument", "unknown"));
    }
    let principal = org.auth.service.as_ref().map(|s| s.id);
    match svc::create(&ctx.db, org.org.id, principal, &d, as_of).await {
        Ok((true, stored)) => Ok((StatusCode::CREATED, Json(stored)).into_response()),
        Ok((false, stored)) => format::json(stored),
        Err(CreateError::Conflict) => Err(coded_error(
            StatusCode::CONFLICT,
            err_codes::MARKET_DECISION_CONFLICT,
            "Another decision exists for this model, version, instrument, interval and date.",
            None,
        )),
        Err(CreateError::Quota) => Err(coded_error(
            StatusCode::TOO_MANY_REQUESTS,
            err_codes::MARKET_DECISIONS_QUOTA,
            "The organization reached its decisions of the day.",
            Some(serde_json::json!({ "max": dec::DAILY_MAX })),
        )),
        Err(CreateError::Db(e)) => Err(e.into()),
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListParams {
    model: Option<String>,
    model_version: Option<String>,
    instrument: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

fn filter<'a>(
    model: &'a Option<String>,
    version: &'a Option<String>,
    instrument: &'a Option<String>,
) -> Filter<'a> {
    let keep = |v: &'a Option<String>| v.as_deref().filter(|s| !s.is_empty());
    Filter {
        model: keep(model),
        model_version: keep(version),
        instrument: keep(instrument),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/markets/decisions",
    tag = "markets",
    operation_id = "markets_decisions_list",
    summary = "List the decisions of external models",
    description = "Decisions posted by the runners of the org, newest first, filtered by model, version or instrument. Advice only, not investment advice. Requires `market-decisions:read`. Paginated (`limit`/`offset`).",
    params(ListParams),
    responses(
        (status = 200, body = Paginated<Decision>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListParams>,
) -> Result<Response> {
    let page = PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let f = filter(&q.model, &q.model_version, &q.instrument);
    let (count, rows) = svc::list(
        &ctx.db,
        org.org.id,
        &f,
        page.limit as u64,
        page.offset as u64,
    )
    .await?;
    let filters: Vec<(String, String)> = [
        ("model", &q.model),
        ("model_version", &q.model_version),
        ("instrument", &q.instrument),
    ]
    .into_iter()
    .filter_map(|(k, v)| {
        v.clone()
            .filter(|s| !s.is_empty())
            .map(|v| (k.to_string(), v))
    })
    .collect();
    format::json(pagination::envelope(
        "/api/v1/markets/decisions",
        &filters,
        page,
        count as i64,
        rows,
    ))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ScoresParams {
    model: Option<String>,
    model_version: Option<String>,
    instrument: Option<String>,
    /// Top probability counted as confident, 0 to 1; 0.6 by default.
    threshold: Option<f64>,
}

#[utoipa::path(
    get,
    path = "/api/v1/markets/decisions/scores",
    tag = "markets",
    operation_id = "markets_decisions_scores",
    summary = "Measure the decisions of external models",
    description = "For each model, version, instrument, source and interval: hit rate, Brier score and reliability by tenth of probability, measured by the triple barrier on the bars of the decision's own source once its horizon has passed. A group whose table needs a clearance the caller lacks is left out; at most 100 groups. Advice only, not investment advice. Requires `market-decisions:read`.",
    params(ScoresParams),
    responses(
        (status = 200, body = Paginated<ModelScores>),
        (status = 400, description = "Invalid `threshold` (field → token) or `market-bars-too-many-files`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`dataset-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn scores(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ScoresParams>,
) -> Result<Response> {
    let threshold = q.threshold.unwrap_or(0.6);
    if !(0.0..=1.0).contains(&threshold) {
        return Ok(field_error("threshold", "range:0..1"));
    }
    let f = filter(&q.model, &q.model_version, &q.instrument);
    let store = match crate::services::datasets::store_of(&ctx.config) {
        Ok(s) => s,
        Err(e) => return super::datasets::dataset_error(e),
    };
    let mut out = Vec::new();
    for g in svc::groups(&ctx.db, org.org.id, &f).await? {
        let table = svc::table_of(&ctx.db, org.org.id, &g.3).await?;
        if let Some(t) = table {
            if super::datasets::governed(&ctx, &org, t, "scores")
                .await
                .is_some()
            {
                continue;
            }
        }
        match svc::score_group(&ctx.db, &store, org.org.id, table, &g, threshold).await {
            Ok(s) => out.push(s),
            Err(e) => return bars_error(e),
        }
    }
    format::json(Paginated {
        count: out.len() as i64,
        next: None,
        previous: None,
        results: out,
    })
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ForecastParams {
    model: Option<String>,
    model_version: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/markets/decisions/forecast-scores",
    tag = "markets",
    operation_id = "markets_decisions_forecast_scores",
    summary = "Measure the forecasts of external models against the prediction market",
    description = "For each model and version: Brier score and calibration by tenth of the probabilities its decisions gave on prediction markets, next to the market's own price at the decision's instant, on the markets resolved final as known now; a market in dispute is not scored. At most 100 groups. Advice only, not investment advice. Requires `market-decisions:read`.",
    params(ForecastParams),
    responses(
        (status = 200, body = Paginated<pnex_core::markets::prediction::ForecastScores>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn forecast_scores(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ForecastParams>,
) -> Result<Response> {
    let rows = crate::services::markets::prediction::forecast_scores(
        &ctx.db,
        org.org.id,
        q.model.as_deref().filter(|s| !s.is_empty()),
        q.model_version.as_deref().filter(|s| !s.is_empty()),
    )
    .await?;
    format::json(Paginated {
        count: rows.len() as i64,
        next: None,
        previous: None,
        results: rows,
    })
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct PredictionParams {
    /// Ticker of the `instrument` object.
    instrument: String,
    /// Instant of the read (RFC 3339), now by default: everything as known then.
    #[param(format = DateTime)]
    as_of: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/markets/prediction-features",
    tag = "markets",
    operation_id = "markets_prediction_features",
    summary = "Read the prediction markets bearing on an instrument as known at a date",
    description = "The prediction markets a member linked to the instrument (`bears_on`), as known at `as_of` and unresolved then (a proposal or a dispute is no resolution), each with its probability, its change over 1 h and 24 h and its days to the end, read only from prices known at `as_of`. Nothing is filtered by relevance: an input of an external model. A market the caller is not cleared for is left out. Requires `ontology:read`.",
    params(PredictionParams),
    responses(
        (status = 200, body = Paginated<pnex_core::markets::prediction::MarketFeature>),
        (status = 400, description = "Invalid parameter (field → token: `instrument` `unknown` when no instrument object has this ticker)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn prediction_features(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<PredictionParams>,
) -> Result<Response> {
    use crate::services::markets::prediction as pm;
    if q.instrument.is_empty() || q.instrument.len() > 64 {
        return Ok(field_error("instrument", err_codes::FIELD_INVALID));
    }
    let as_of = match instant("as_of", q.as_of.as_deref()) {
        Ok(a) => a.unwrap_or_else(Utc::now),
        Err(r) => return Ok(r),
    };
    let ids = pm::instruments(&ctx.db, org.org.id, &q.instrument).await?;
    if ids.is_empty() {
        return Ok(field_error("instrument", "unknown"));
    }
    let mut rows = pm::features(&ctx.db, org.org.id, &ids, as_of).await?;
    // A market the caller is not cleared for is left out (governance.md D801).
    let hidden = match super::ontology::redaction(&ctx, &org).await {
        Ok(r) => r.objects,
        Err(e) => {
            tracing::error!(error = %e, "markings read failed");
            return Err(Error::InternalServerError);
        }
    };
    rows.retain(|f| hidden.as_ref().is_none_or(|h| !h.contains(&f.market_id)));
    format::json(Paginated {
        count: rows.len() as i64,
        next: None,
        previous: None,
        results: rows,
    })
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct AccountTradesParams {
    /// `<platform>:<account id>`, e.g. `myfxbook:12345`; every account when empty.
    account: Option<String>,
    /// Instant of the read (RFC 3339), now by default: only trades known then.
    #[param(format = DateTime)]
    as_of: Option<String>,
    /// Page size, 1 to 50000 (default 1000).
    limit: Option<String>,
    offset: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/markets/account-trades",
    tag = "markets",
    operation_id = "markets_account_trades",
    summary = "Read the imported trades of public trading accounts as known at a date",
    description = "Trades of public trading accounts imported by members, by opening time, only those known at `as_of` (a trade imported later is absent; `backfilled` marks a trade with no proof it was public before its import). An account the caller is not cleared for is left out. Requires `ontology:read`.",
    params(AccountTradesParams),
    responses(
        (status = 200, body = Paginated<crate::services::markets::accounts::AccountTrade>),
        (status = 400, description = "Invalid parameter (field → token: `account` `invalid`, `as_of` `invalid`)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn account_trades(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<AccountTradesParams>,
) -> Result<Response> {
    use crate::services::markets::accounts as acc;
    let account = q.account.as_deref().filter(|a| !a.is_empty());
    if account.is_some_and(|a| !acc::account_ref_ok(a)) {
        return Ok(field_error("account", err_codes::FIELD_INVALID));
    }
    let as_of = match instant("as_of", q.as_of.as_deref()) {
        Ok(a) => a.unwrap_or_else(Utc::now),
        Err(r) => return Ok(r),
    };
    let limit = match q.limit.as_deref().map(str::parse::<u64>) {
        None => 1_000,
        Some(Ok(n)) if (1..=acc::IMPORT_MAX as u64).contains(&n) => n,
        Some(_) => return Ok(field_error("limit", err_codes::FIELD_INVALID)),
    };
    let offset = match q.offset.as_deref().map(str::parse::<u64>) {
        None => 0,
        Some(Ok(n)) => n,
        Some(Err(_)) => return Ok(field_error("offset", err_codes::FIELD_INVALID)),
    };
    // An account the caller is not cleared for is left out (governance.md D801).
    let hidden = match super::ontology::redaction(&ctx, &org).await {
        Ok(r) => r.objects,
        Err(e) => {
            tracing::error!(error = %e, "markings read failed");
            return Err(Error::InternalServerError);
        }
    };
    let hidden_refs = acc::refs_of(&ctx.db, org.org.id, hidden.as_ref()).await?;
    let (count, rows) = acc::list(
        &ctx.db,
        org.org.id,
        account,
        &hidden_refs,
        as_of,
        limit,
        offset,
    )
    .await?;
    format::json(Paginated {
        count: count as i64,
        next: None,
        previous: None,
        results: rows,
    })
}

/// Size cap of one trades import.
const IMPORT_MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ImportTradesParams {
    /// `<platform>:<account id>`, e.g. `myfxbook:12345`.
    account: String,
    /// Display name of the account on the platform (a pseudonym), kept when the account is created.
    name: Option<String>,
    /// Name of the source file, kept on each row.
    source: Option<String>,
    /// Currency of the account, ISO 4217 or `USC` (cent account): the unit of its profits, kept on the account.
    currency: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/markets/account-trades/import",
    tag = "markets",
    operation_id = "markets_account_trades_import",
    summary = "Import the trades of a public trading account from a CSV",
    description = "Stores the trades of a public trading account sent as the canonical CSV (comma-separated, header among `ticket, symbol, side, lots, open_at, close_at, open_price, close_price, sl, tp, profit, pips, commission, swap, gain_pct, magic, comment`; `symbol`, `side` (buy or sell), `open_at` and `open_price` required, `lots` absent when the owner hides sizes; instants in RFC 3339 with an offset). A trade already stored (same ticket, else same opening, close and rank among identical rows of the file) is kept as first known. Every trade is known from the import, flagged `backfilled`. Creates the `trading_account` object when missing and links it to the instruments it traded. The whole file is refused at its first faulty cell. Requires `ontology:write`.",
    params(ImportTradesParams),
    request_body(content = String, content_type = "text/csv"),
    responses(
        (status = 200, body = crate::services::markets::accounts::ImportResult),
        (status = 400, description = "`account-trades-csv-invalid` (args `line`, `column`), or `field-invalid` on `account`, `name`, `source`, `currency`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 413, description = "`account-trades-too-large` (args `max_kib`, `max_rows`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn import_account_trades(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ImportTradesParams>,
    body: axum::body::Bytes,
) -> Result<Response> {
    use crate::services::markets::accounts as acc;
    if !acc::account_ref_ok(&q.account) {
        return Ok(field_error("account", err_codes::FIELD_INVALID));
    }
    if q.name.as_deref().is_some_and(|n| n.chars().count() > 200) {
        return Ok(field_error("name", err_codes::FIELD_INVALID));
    }
    let source = q
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("import.csv");
    if source.chars().count() > 200 {
        return Ok(field_error("source", err_codes::FIELD_INVALID));
    }
    let currency = q.currency.as_deref().filter(|c| !c.is_empty());
    if currency.is_some_and(|c| !acc::currency_ok(c)) {
        return Ok(field_error("currency", err_codes::FIELD_INVALID));
    }
    let too_large = || {
        Err(coded_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            err_codes::ACCOUNT_TRADES_TOO_LARGE,
            "trades file too large",
            Some(
                serde_json::json!({ "max_kib": IMPORT_MAX_BYTES / 1024, "max_rows": acc::IMPORT_MAX }),
            ),
        ))
    };
    if body.len() > IMPORT_MAX_BYTES {
        return too_large();
    }
    let refused = |line: usize, column: &str| {
        Err(coded_error(
            StatusCode::BAD_REQUEST,
            err_codes::ACCOUNT_TRADES_CSV_INVALID,
            "trades file is not the canonical CSV",
            Some(serde_json::json!({ "line": line, "column": column })),
        ))
    };
    let Ok(text) = std::str::from_utf8(&body) else {
        return refused(0, "encoding");
    };
    let trades = match pnex_core::markets::accounts::parse_trades_csv(text) {
        Ok(t) => t,
        Err(e) => return refused(e.line, &e.column),
    };
    if trades.len() > acc::IMPORT_MAX {
        return too_large();
    }
    let actor = super::ontology::actor(&org);
    match acc::import(
        &ctx.db,
        org.org.id,
        &actor,
        &q.account,
        q.name.as_deref(),
        currency,
        &trades,
        source,
        Utc::now(),
    )
    .await
    {
        Ok(r) => format::json(r),
        Err(e) => super::ontology::onto_error(e),
    }
}
