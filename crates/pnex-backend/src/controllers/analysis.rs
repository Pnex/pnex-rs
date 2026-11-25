//! Read-only SQL API (analysis.md D825–D828). Org from the principal (R1), the
//! permission applied by `OrgContext` (R2), what a query reads fixed by the server
//! under the caller's rights and markings; a query is its author's only.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use pnex_core::analysis::AnalysisQueryInput;
use pnex_core::err_codes;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::services::analysis::{self, AnalysisError};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/analysis/queries", post(submit))
        .add("/analysis/queries/{id}", get(one))
        .add("/analysis/named", get(named))
        .add("/analysis/named/{pack}/{key}", post(run_named))
        .add("/analysis/event-study", get(event_study))
        .add("/analysis/chains", get(chains))
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(submit, one, named, run_named, event_study, chains),
        components(schemas(
            pnex_core::market_study::DatedEventStudy,
            pnex_core::rca::Chain,
            pnex_core::rca::Hop,
            pnex_core::rca::Signal,
            pnex_core::rca::SignalKind
        ))
    )]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn analysis_error(e: AnalysisError) -> Result<Response> {
    let (status, code, args) = match e {
        AnalysisError::Invalid { field, token } => return Ok(super::field_error(field, &token)),
        AnalysisError::Busy => (StatusCode::CONFLICT, err_codes::ANALYSIS_BUSY, None),
        AnalysisError::NotFound => (StatusCode::NOT_FOUND, err_codes::ANALYSIS_NOT_FOUND, None),
        AnalysisError::Coded(code, args) => {
            let status = if code == err_codes::ANALYSIS_TOO_MANY_OBJECTS {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            };
            (status, code, Some(args))
        }
        AnalysisError::Db(e) => {
            tracing::error!(error = %e, "analysis database error");
            return Err(Error::InternalServerError);
        }
    };
    Err(super::coded_error(
        status,
        code,
        code.replace('-', " "),
        args,
    ))
}

/// `POST /api/v1/analysis/queries`
#[utoipa::path(
    post,
    path = "/api/v1/analysis/queries",
    tag = "analysis",
    operation_id = "analysis_queries_create",
    summary = "Send a read-only SQL query",
    description = "Queues one read-only SELECT on the tables of the org and on `objects.<type>` (the objects of a type as the caller sees them), run by the pipeline worker before any pipeline: 60 s at most, 1,000 rows returned. A marked table the caller is not cleared for fails the query with `marking-required`. Read the result with `GET /analysis/queries/{id}`. One query at a time per person; with `max_age_secs` (300 at most) the caller's own running or recent run of the same text is answered instead. Requires `analysis:run`.",
    request_body = pnex_core::analysis::AnalysisQueryInput,
    responses(
        (status = 202, body = pnex_core::analysis::AnalysisQuery),
        (status = 400, description = "`field-invalid` on `sql`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`analysis-busy`", body = crate::openapi::ErrorDetail),
        (status = 422, description = "`analysis-too-many-objects` (args `type`, `max`)", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`analysis-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn submit(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<AnalysisQueryInput>,
) -> Result<Response> {
    let store = match crate::services::datasets::store_of(&ctx.config) {
        Ok(s) => s,
        Err(_) => {
            return analysis_error(AnalysisError::Coded(
                err_codes::ANALYSIS_UNAVAILABLE,
                serde_json::Value::Null,
            ));
        }
    };
    let hide = super::ontology::redaction(&ctx, &org)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let sent = analysis::submit(
        &ctx.db,
        &store,
        org.org.id,
        org.auth.user.id,
        hide,
        &input.sql,
        input.max_age_secs,
    )
    .await;
    match sent {
        Ok(q) => Ok((StatusCode::ACCEPTED, Json(q)).into_response()),
        Err(e) => analysis_error(e),
    }
}

/// `GET /api/v1/analysis/queries/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/analysis/queries/{id}",
    tag = "analysis",
    operation_id = "analysis_queries_get",
    summary = "Read a query and its rows",
    description = "The state of one of the caller's queries; once `done`, its columns and first rows as text (`truncated` when there were more), once `failed`, its code: `analysis-sql-invalid` (args `reason`), `analysis-table-unknown` (args `table`), `marking-required` (args `table`), `analysis-timeout`, `analysis-result-too-large`, `pipeline-memory-exceeded`. Kept a day. Requires `analysis:run`.",
    params(("id" = String, Path, format = "uuid", description = "Query id (UUID)")),
    responses(
        (status = 200, body = pnex_core::analysis::AnalysisQuery),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`analysis-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match analysis::get(&ctx.db, org.org.id, org.auth.user.id, id).await {
        Ok(q) => format::json(q),
        Err(e) => analysis_error(e),
    }
}

/// The named queries of the packs the org installed.
async fn installed(
    db: &sea_orm::DatabaseConnection,
    org: i64,
) -> Result<Vec<pnex_core::named_query::NamedQueryView>> {
    use sea_orm::ConnectionTrait;
    let keys: Vec<String> = db
        .query_all_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT key FROM packs WHERE org_id = $1",
            vec![org.into()],
        ))
        .await?
        .iter()
        .filter_map(|r| r.try_get::<String>("", "key").ok())
        .collect();
    Ok(crate::services::ontology::packs::shipped()
        .into_iter()
        .filter(|p| keys.contains(&p.key))
        .flat_map(|p| {
            let pack = p.key.clone();
            p.queries
                .into_iter()
                .map(move |query| pnex_core::named_query::NamedQueryView {
                    pack: pack.clone(),
                    query,
                })
        })
        .collect())
}

/// `GET /api/v1/analysis/named`
#[utoipa::path(
    get,
    path = "/api/v1/analysis/named",
    tag = "analysis",
    operation_id = "analysis_named_list",
    summary = "List the named queries of the installed packs",
    description = "Each read-only query a pack the org installed declares (pipelines.md D948): its key, its typed parameters and its SQL. Run one with `POST /analysis/named/{pack}/{key}`. Requires `analysis:run`.",
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::named_query::NamedQueryView>),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn named(State(ctx): State<AppContext>, org: OrgContext) -> Result<Response> {
    let results = installed(&ctx.db, org.org.id).await?;
    format::json(pnex_core::Paginated {
        count: results.len() as i64,
        next: None,
        previous: None,
        results,
    })
}

/// `POST /api/v1/analysis/named/{pack}/{key}`
#[utoipa::path(
    post,
    path = "/api/v1/analysis/named/{pack}/{key}",
    tag = "analysis",
    operation_id = "analysis_named_run",
    summary = "Run a named query of a pack",
    description = "Checks each parameter against its type and queues the pack's query as an analysis query of the caller (same rights, limits and reading as `POST /analysis/queries`): read its rows with `GET /analysis/queries/{id}`. A value never becomes SQL: it is written as a literal of its type. Requires `analysis:run`.",
    params(
        ("pack" = String, Path, description = "Key of an installed pack"),
        ("key" = String, Path, description = "Key of one of its queries")
    ),
    request_body = pnex_core::named_query::NamedQueryRun,
    responses(
        (status = 202, body = pnex_core::analysis::AnalysisQuery),
        (status = 400, description = "`field-invalid` or `field-required` on `params.<name>`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`named-query-not-found`", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`analysis-busy`", body = crate::openapi::ErrorDetail),
        (status = 422, description = "`analysis-too-many-objects` (args `type`, `max`)", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`analysis-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn run_named(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((pack, key)): Path<(String, String)>,
    Json(input): Json<pnex_core::named_query::NamedQueryRun>,
) -> Result<Response> {
    let Some(q) = installed(&ctx.db, org.org.id)
        .await?
        .into_iter()
        .find(|v| v.pack == pack && v.query.key == key)
    else {
        return Err(super::coded_error(
            StatusCode::NOT_FOUND,
            err_codes::NAMED_QUERY_NOT_FOUND,
            "named query not found",
            None,
        ));
    };
    let sql = match q.query.filled(&input.params) {
        Ok(s) => s,
        Err((name, token)) => {
            let token = match token.as_str() {
                "required" => err_codes::FIELD_REQUIRED,
                _ => err_codes::FIELD_INVALID,
            };
            return Ok(super::field_error(&format!("params.{name}"), token));
        }
    };
    submit(
        State(ctx),
        org,
        Json(AnalysisQueryInput {
            sql,
            max_age_secs: input.max_age_secs,
        }),
    )
    .await
}

/// Event study: at most this many dates per read.
const STUDY_DATES_MAX: usize = 100;

/// Documents the query of [`event_study`]; read from the raw query, since
/// `date` repeats.
#[allow(dead_code)]
#[derive(utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct StudyParams {
    /// Object whose series is measured (UUID).
    #[param(format = "uuid")]
    object: String,
    /// Metric of the series (`market_observations`), `close` by default.
    metric: Option<String>,
    /// Object of the benchmark series (UUID), same metric; the org's benchmark instrument by default.
    #[param(format = "uuid")]
    benchmark: Option<String>,
    /// Day of an event (`YYYY-MM-DD`), repeated for each event, 100 at most.
    #[param(format = Date)]
    date: Vec<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/analysis/event-study",
    tag = "analysis",
    operation_id = "analysis_event_study",
    summary = "Measure how a series moved around dates",
    description = "For each `date`, in the order given: the raw and abnormal change of the object's series around it, against the benchmark series, with a market model estimated before the date, and the relative volume. `study` is absent without a benchmark or enough values. An aid to analysis, never a cause. Requires `ontology:read`.",
    params(StudyParams),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::market_study::DatedEventStudy>),
        (status = 400, description = "Invalid parameter (field → token)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn event_study(
    State(ctx): State<AppContext>,
    org: OrgContext,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> Result<Response> {
    let (mut object, mut metric, mut benchmark, mut dates) = (None, None, None, Vec::new());
    for (k, v) in form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        match k.as_ref() {
            "object" => object = Some(v.into_owned()),
            "metric" => metric = Some(v.into_owned()),
            "benchmark" => benchmark = Some(v.into_owned()).filter(|b| !b.is_empty()),
            "date" => match super::markets::day("date", Some(&v)) {
                Ok(Some(d)) => dates.push(d),
                Ok(None) => {}
                Err(r) => return Ok(r),
            },
            _ => {}
        }
    }
    let Some(object) = object else {
        return Ok(super::field_error("object", err_codes::FIELD_REQUIRED));
    };
    let metric = metric.unwrap_or_else(|| "close".into());
    if metric.is_empty() || metric.len() > 64 {
        return Ok(super::field_error("metric", err_codes::FIELD_INVALID));
    }
    if dates.is_empty() {
        return Ok(super::field_error("date", err_codes::FIELD_REQUIRED));
    }
    if dates.len() > STUDY_DATES_MAX {
        return Ok(super::field_error("date", "max_items:100"));
    }
    let object = super::ontology::parse_uuid(&object)?;
    let benchmark = benchmark
        .map(|b| super::ontology::parse_uuid(&b))
        .transpose()?;
    for id in std::iter::once(object).chain(benchmark) {
        if let Err(e) = crate::services::ontology::objects::find(&ctx.db, org.org.id, id).await {
            return super::ontology::onto_error(e);
        }
    }
    let rows = crate::services::markets::study::dated(
        &ctx.db, org.org.id, object, &metric, benchmark, &dates,
    )
    .await?;
    format::json(pnex_core::Paginated {
        count: rows.len() as i64,
        next: None,
        previous: None,
        results: rows,
    })
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ChainsParams {
    /// Bar Table read (UUID): minute bars with `ts`, `instrument`, `source` and the OHLCV columns.
    #[param(format = "uuid")]
    table: String,
    /// Value of the `instrument` column, and ticker of the `instrument` object the moves are on.
    key: String,
    /// Value of the `source` column (`mt5:<broker>`…); every source of the Table by default.
    source: Option<String>,
    /// Instant of the read (RFC 3339), now by default: everything as known then.
    #[param(format = DateTime)]
    as_of: Option<String>,
    /// First instant of the moves explained (RFC 3339); 7 days before
    /// `as_of` by default, 31 days at most.
    #[param(format = DateTime)]
    from: Option<String>,
    /// Smallest 1-hour change (percent) that is a move; the object's
    /// `move_threshold_pct`, then the org's setting, by default.
    threshold_pct: Option<f64>,
}

#[utoipa::path(
    get,
    path = "/api/v1/analysis/chains",
    tag = "analysis",
    operation_id = "analysis_chains",
    summary = "Read the candidate causal chains of a series' moves as known at a date",
    description = "For each 1-hour move of the series beyond its threshold in [`from`, `as_of`], the 3 best candidate chains from a world event, a supply disruption or a macro release, through the links a member declared as propagating (`exposed_to`), scored by the pairs of the installed packs, each with its placebo and lift (never filtered: an input of an external model). Computed at reading time, never stored, never a cause (D742). Everything as known at `as_of`. Requires `datasets:read` and `ontology:read`.",
    params(ChainsParams, super::pagination::PageQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::rca::Chain>),
        (status = 400, description = "Invalid parameter (field → token: `key` `unknown` when no instrument object has this ticker, `from` `range:31d`) or `market-bars-too-many-files`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`access-denied` or `marking-required`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`dataset-not-found`", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`dataset-storage-unavailable`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn chains(
    State(ctx): State<AppContext>,
    org: OrgContext,
    axum::extract::Query(q): axum::extract::Query<ChainsParams>,
    axum::extract::Query(pq): axum::extract::Query<super::pagination::PageQuery>,
) -> Result<Response> {
    use super::market_decisions::{bars_error, instant};
    use crate::services::markets::chains::{self as ch, ChainsError, ChainsQuery};
    use pnex_core::rca::Signal;
    if !org.can("ontology:read") {
        return Err(crate::auth::access_denied("ontology:read"));
    }
    let table = super::ontology::parse_uuid(&q.table)?;
    if q.key.is_empty() || q.key.len() > 64 {
        return Ok(super::field_error("key", err_codes::FIELD_INVALID));
    }
    let source = q.source.clone().unwrap_or_default();
    if source.len() > 64 {
        return Ok(super::field_error("source", err_codes::FIELD_INVALID));
    }
    if q.threshold_pct.is_some_and(|t| !(t > 0.0 && t.is_finite())) {
        return Ok(super::field_error(
            "threshold_pct",
            err_codes::FIELD_INVALID,
        ));
    }
    let (as_of, from) = match (
        instant("as_of", q.as_of.as_deref()),
        instant("from", q.from.as_deref()),
    ) {
        (Ok(a), Ok(f)) => (a.unwrap_or_else(chrono::Utc::now), f),
        (Err(r), _) | (_, Err(r)) => return Ok(r),
    };
    let from = from.unwrap_or(as_of - chrono::Duration::days(ch::WINDOW_DEFAULT_DAYS));
    if from > as_of || as_of - from > chrono::Duration::days(ch::WINDOW_MAX_DAYS) {
        return Ok(super::field_error("from", "range:31d"));
    }
    if let Some(refused) = super::datasets::governed(&ctx, &org, table, "bars").await {
        return refused;
    }
    let store = match crate::services::datasets::store_of(&ctx.config) {
        Ok(s) => s,
        Err(e) => return super::datasets::dataset_error(e),
    };
    let page = super::pagination::PageParams::from(pq.limit.as_deref(), pq.offset.as_deref());
    let mut filters = vec![
        ("table".to_string(), table.to_string()),
        ("key".to_string(), q.key.clone()),
        ("as_of".to_string(), as_of.to_rfc3339()),
        ("from".to_string(), from.to_rfc3339()),
    ];
    if !source.is_empty() {
        filters.push(("source".to_string(), source.clone()));
    }
    if let Some(t) = q.threshold_pct {
        filters.push(("threshold_pct".to_string(), t.to_string()));
    }
    filters.sort();
    let query = ChainsQuery {
        key: &q.key,
        source: &source,
        table,
        from,
        as_of,
        threshold_pct: q.threshold_pct,
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
            super::pagination::paged("/api/v1/analysis/chains", &filters, page, rows)
        }
        Err(ChainsError::UnknownKey) => Ok(super::field_error("key", "unknown")),
        Err(ChainsError::Bars(e)) => bars_error(e),
        Err(ChainsError::Db(e)) => Err(e.into()),
        Err(ChainsError::Onto(e)) => {
            tracing::error!(error = %e, "causal pairs read failed");
            Err(Error::InternalServerError)
        }
    }
}
