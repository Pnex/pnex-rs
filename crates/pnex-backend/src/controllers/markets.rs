//! Event studies (markets-research.md D532, D535). Aids to analysis: nothing
//! here places an order or changes anything.

use axum::extract::{Path, Query, State};
use axum::response::Response;
use chrono::NaiveDate;
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::market_study::{
    Distribution, EventStudy, PositionResult, RuleResult, RuleWindow, WindowResult,
};
use serde::Deserialize;

use super::field_error;
use crate::auth::OrgContext;
use crate::services::markets::study::{self, DateSource, DatedRule, Rule};
use pnex_core::markets::chart::{ChartEvent, ChartEventKind, ChartPoint, IssuerChart};
use pnex_core::markets::moves::{MarketSession, SessionMove, SessionWorldEvent, WorldEventPoint};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/markets")
        .add("/instruments/{id}/event-study", get(event_study))
        .add("/issuers/{id}/chart", get(issuer_chart))
        .add("/rule-test", get(rule_test))
        .add("/sessions/{date}", get(session))
        .add("/world-events", get(world_events))
        .add("/positions/{id}/result", get(position_result))
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(
            event_study,
            issuer_chart,
            rule_test,
            position_result,
            session,
            world_events
        ),
        components(schemas(
            IssuerChart,
            ChartPoint,
            ChartEvent,
            ChartEventKind,
            DateSource,
            MarketSession,
            SessionMove,
            SessionWorldEvent,
            WorldEventPoint,
            EventStudy,
            WindowResult,
            RuleResult,
            RuleWindow,
            Distribution,
            PositionResult
        ))
    )]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}

fn day(field: &str, raw: Option<&str>) -> std::result::Result<Option<NaiveDate>, Response> {
    match raw.filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(Some)
            .map_err(|_| field_error(field, err_codes::FIELD_INVALID)),
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct StudyQuery {
    /// Day of the event (`YYYY-MM-DD`).
    #[param(format = Date)]
    date: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/markets/instruments/{id}/event-study",
    tag = "markets",
    operation_id = "markets_event_study",
    summary = "Measure how an instrument moved around a date",
    description = "Raw and abnormal return around the date, against the org's benchmark, with a market model estimated before the event, and the relative volume. `null` without a benchmark or enough prices. An aid to analysis, not investment advice. Any org member.",
    params(
        ("id" = String, Path, format = "uuid", description = "Instrument object id (UUID)"),
        StudyQuery
    ),
    responses(
        (status = 200, body = Option<EventStudy>),
        (status = 400, description = "Invalid `date` (field → token)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn event_study(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<StudyQuery>,
) -> Result<Response> {
    let id = super::ontology::parse_uuid(&id)?;
    let date = match day("date", Some(&q.date)) {
        Ok(Some(d)) => d,
        Ok(None) => return Ok(field_error("date", err_codes::FIELD_REQUIRED)),
        Err(r) => return Ok(r),
    };
    if let Err(e) = crate::services::ontology::objects::find(&ctx.db, org.org.id, id).await {
        return super::ontology::onto_error(e);
    }
    format::json(study::one(&ctx.db, org.org.id, id, date).await?)
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ChartQuery {
    /// Days back from today, 1 to 3650; 365 by default.
    days: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/api/v1/markets/issuers/{id}/chart",
    tag = "markets",
    operation_id = "markets_issuer_chart",
    summary = "Read the price chart of an issuer with its sessions and filings",
    description = "Raw closes of the issuer's instrument (or of the instrument itself), with its abnormal sessions and the issuer's filings on the same days, oldest first. Empty closes when no price was read. Any org member.",
    params(
        ("id" = String, Path, format = "uuid", description = "Organization or instrument object id (UUID)"),
        ChartQuery
    ),
    responses(
        (status = 200, body = IssuerChart),
        (status = 400, description = "Invalid `days` (field → token)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn issuer_chart(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
    Query(q): Query<ChartQuery>,
) -> Result<Response> {
    let id = super::ontology::parse_uuid(&id)?;
    let days = q.days.unwrap_or(365);
    if !(1..=3650).contains(&days) {
        return Ok(field_error("days", err_codes::FIELD_INVALID));
    }
    let object = match crate::services::ontology::objects::find(&ctx.db, org.org.id, id).await {
        Ok(o) => o,
        Err(e) => return super::ontology::onto_error(e),
    };
    let since = chrono::Utc::now().date_naive() - chrono::Duration::days(days);
    let mut chart =
        crate::services::markets::chart::issuer(&ctx.db, org.org.id, id, &object.type_key, since)
            .await?;
    if let Some(ticker) = chart.ticker.clone() {
        decisions_on_chart(&ctx, &org, &mut chart, &ticker, since).await?;
    }
    format::json(chart)
}

/// D897: the decisions of external models about this ticker; without daily
/// closes, the daily bars of the source of its latest decision are drawn.
async fn decisions_on_chart(
    ctx: &AppContext,
    org: &OrgContext,
    chart: &mut IssuerChart,
    ticker: &str,
    since: NaiveDate,
) -> Result<()> {
    use crate::services::markets::decisions as dec;
    let mut marks = dec::chart_markers(&ctx.db, org.org.id, ticker, since).await?;
    if marks.is_empty() || !org.can("market-decisions:read") {
        return Ok(());
    }
    // The daily bars are table content: they need the table right too.
    if chart.closes.is_empty() && org.can("datasets:read") {
        let latest = dec::list(
            &ctx.db,
            org.org.id,
            &dec::Filter {
                instrument: Some(ticker),
                ..Default::default()
            },
            1,
            0,
        )
        .await?
        .1;
        let source = latest.first().map(|d| d.source.clone()).unwrap_or_default();
        let table = dec::table_of(&ctx.db, org.org.id, &source).await?;
        let store = crate::services::datasets::store_of(&ctx.config).ok();
        if let (Some(table), Some(store)) = (table, store) {
            if super::datasets::governed(ctx, org, table, "chart")
                .await
                .is_none()
            {
                let q = dec::BarsQuery {
                    instrument: ticker,
                    source: &source,
                    interval_us: 86_400_000_000,
                    from_us: since
                        .and_hms_opt(0, 0, 0)
                        .map(|t| t.and_utc().timestamp_micros()),
                    to_us: None,
                    limit: pnex_core::markets::decisions::BARS_MAX,
                    as_of_us: chrono::Utc::now().timestamp_micros(),
                };
                // A table that cannot be read leaves the chart without closes.
                if let Ok(bars) = dec::bars(&ctx.db, &store, org.org.id, table, &q).await {
                    let unit = 10f64.powi(pnex_core::markets::history::SCALE as i32);
                    chart.closes = bars
                        .iter()
                        .filter_map(|b| {
                            Some(ChartPoint {
                                date: chrono::DateTime::from_timestamp_micros(b.ts_us)?
                                    .date_naive()
                                    .to_string(),
                                value: b.close as f64 / unit,
                            })
                        })
                        .collect();
                }
            }
        }
    }
    chart.events.append(&mut marks);
    chart.events.sort_by(|a, b| a.date.cmp(&b.date));
    Ok(())
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct RuleQuery {
    /// Where the dates come from; `filings` by default.
    dates: Option<DateSource>,
    /// Form of the filings (`8-K`, `10-Q`…); required with `filings`.
    form: Option<String>,
    /// Ticker or macro series code measured; required with `geo_events`
    /// and `market_moves`.
    series: Option<String>,
    /// `geo_events`: kind of event (`armed_conflict`, `attack`, `sanctions`…).
    kind: Option<String>,
    /// `geo_events`: ISO 3166 alpha-2 country code.
    country: Option<String>,
    /// `geo_events`: also count unconfirmed events (default false).
    include_unconfirmed: Option<bool>,
    /// `market_moves`: asset class of the moves.
    asset_class: Option<String>,
    /// `market_moves`: `up` or `down`.
    direction: Option<String>,
    /// One item of the form (`2.02` = results in an 8-K).
    item: Option<String>,
    /// First filing day (`YYYY-MM-DD`).
    #[param(format = Date)]
    from: Option<String>,
    /// Last filing day (`YYYY-MM-DD`).
    #[param(format = Date)]
    to: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/markets/rule-test",
    tag = "markets",
    operation_id = "markets_rule_test",
    summary = "Test how prices moved after a kind of filing or event",
    description = "With `dates=filings`, the distribution of the abnormal return after every matching filing of the followed instruments. With `dates=geo_events` or `market_moves`, how one series (`series`) moved after each matching event, dated by when it became known; events closer than the longest window count once (`merged`), unconfirmed world events stay out unless asked. Always next to the same measure on drawn dates, with the number of events. Instruments no longer followed are missing, which can flatter a result. An aid to analysis, not investment advice, and never a cause. Any org member.",
    params(RuleQuery),
    responses(
        (status = 200, body = RuleResult),
        (status = 400, description = "Invalid `form`, `series`, `kind`, `asset_class`, `direction`, `from` or `to` (field → token)", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`server-busy`: too many rule tests at once", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn rule_test(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<RuleQuery>,
) -> Result<Response> {
    let (from, to) = match (day("from", q.from.as_deref()), day("to", q.to.as_deref())) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(r), _) | (_, Err(r)) => return Ok(r),
    };
    let source = q.dates.unwrap_or_default();
    if source != DateSource::Filings {
        let text = |v: &Option<String>| v.as_deref().unwrap_or("").trim().to_string();
        let series = text(&q.series);
        if series.is_empty() || series.len() > 64 {
            return Ok(field_error("series", err_codes::FIELD_REQUIRED));
        }
        let (kind, asset_class, direction) =
            (text(&q.kind), text(&q.asset_class), text(&q.direction));
        const KINDS: [&str; 7] = [
            "armed_conflict",
            "attack",
            "sanctions",
            "blockade",
            "ceasefire",
            "natural_disaster",
            "other",
        ];
        if !kind.is_empty() && !KINDS.contains(&kind.as_str()) {
            return Ok(field_error("kind", err_codes::FIELD_INVALID));
        }
        if !asset_class.is_empty()
            && pnex_core::markets::moves::AssetClass::parse(&asset_class).is_none()
        {
            return Ok(field_error("asset_class", err_codes::FIELD_INVALID));
        }
        if !direction.is_empty() && direction != "up" && direction != "down" {
            return Ok(field_error("direction", err_codes::FIELD_INVALID));
        }
        let rule = DatedRule {
            source,
            series,
            kind,
            country: text(&q.country),
            include_unconfirmed: q.include_unconfirmed.unwrap_or(false),
            asset_class,
            direction,
            from,
            to,
        };
        return match study::rule_on_dates(&ctx.db, org.org.id, &rule).await? {
            Some(r) => format::json(r),
            None => Ok(field_error("series", err_codes::FIELD_INVALID)),
        };
    }
    let form = q.form.unwrap_or_default();
    if form.trim().is_empty() || form.len() > 16 {
        return Ok(field_error("form", err_codes::FIELD_INVALID));
    }
    let rule = Rule {
        form: form.trim().to_string(),
        item: q.item.unwrap_or_default().trim().to_string(),
        from,
        to,
    };
    format::json(study::rule(&ctx.db, org.org.id, &rule).await?)
}

#[utoipa::path(
    get,
    path = "/api/v1/markets/positions/{id}/result",
    tag = "markets",
    operation_id = "markets_position_result",
    summary = "Measure a fictitious position on closing prices",
    description = "Result of a `paper_position` from the first close at or after its entry day to its exit day or the latest close, dividends and splits counted. `null` without an instrument or prices. Fictitious: no order exists in PNEX. Any org member.",
    params(("id" = String, Path, format = "uuid", description = "Position object id (UUID)")),
    responses(
        (status = 200, body = Option<PositionResult>),
        (status = 404, description = "`ontology-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn position_result(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<String>,
) -> Result<Response> {
    let id = super::ontology::parse_uuid(&id)?;
    let object = match crate::services::ontology::objects::find(&ctx.db, org.org.id, id).await {
        Ok(o) if o.type_key == "paper_position" => o,
        Ok(_) => {
            return super::ontology::onto_error(crate::services::ontology::OntologyError::NotFound)
        }
        Err(e) => return super::ontology::onto_error(e),
    };
    format::json(study::position(&ctx.db, org.org.id, id, &object.properties).await?)
}

#[utoipa::path(
    get,
    path = "/api/v1/markets/sessions/{date}",
    tag = "markets",
    operation_id = "markets_session",
    summary = "List the abnormal sessions and world events around a date",
    description = "The abnormal moves of the watched series in the window of one session before and after the date (those of the date first, by size against their usual volatility), and the world events of the same window, dismissed ones left out. Computed at reading: being in the same window is never a cause. Any org member.",
    params(("date" = String, Path, format = Date, description = "Session day (`YYYY-MM-DD`)")),
    responses(
        (status = 200, body = MarketSession),
        (status = 400, description = "Invalid `date` (field → token)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn session(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(date): Path<String>,
) -> Result<Response> {
    let date = match day("date", Some(&date)) {
        Ok(Some(d)) => d,
        Ok(None) => return Ok(field_error("date", err_codes::FIELD_REQUIRED)),
        Err(r) => return Ok(r),
    };
    format::json(crate::services::markets::moves::session(&ctx.db, org.org.id, date).await?)
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct WorldEventsQuery {
    /// West longitude of the view (-180 to 180).
    west: f64,
    /// South latitude of the view (-90 to 90).
    south: f64,
    east: f64,
    north: f64,
    /// Days back from today (1 to 366, default 30).
    days: Option<i64>,
    /// Page size (1 to 300, default 300).
    limit: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/api/v1/markets/world-events",
    tag = "markets",
    operation_id = "markets_world_events",
    summary = "List the world events placed inside a map view",
    description = "The world events of the last `days` days whose location falls inside the bounding box, dismissed ones left out, the most significant first (deaths, then sources, then newest), at most `limit`. `count` is the total in the box. Any org member.",
    params(WorldEventsQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<WorldEventPoint>),
        (status = 400, description = "Invalid `west`, `south`, `east`, `north`, `days` or `limit` (field → token)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn world_events(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<WorldEventsQuery>,
) -> Result<Response> {
    for (field, v, max) in [
        ("west", q.west, 180.0),
        ("east", q.east, 180.0),
        ("south", q.south, 90.0),
        ("north", q.north, 90.0),
    ] {
        if !v.is_finite() || v.abs() > max {
            return Ok(field_error(field, err_codes::FIELD_INVALID));
        }
    }
    let days = q.days.unwrap_or(30);
    if !(1..=366).contains(&days) {
        return Ok(field_error("days", err_codes::FIELD_INVALID));
    }
    let limit = q.limit.unwrap_or(300);
    if !(1..=300).contains(&limit) {
        return Ok(field_error("limit", err_codes::FIELD_INVALID));
    }
    let bbox = (q.west, q.south, q.east, q.north);
    let (count, results) =
        crate::services::markets::moves::world_events_in(&ctx.db, org.org.id, bbox, days, limit)
            .await?;
    format::json(pnex_core::Paginated {
        count,
        next: None,
        previous: None,
        results,
    })
}
