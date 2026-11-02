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
use crate::services::markets::study::{self, Rule};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/markets")
        .add("/instruments/{id}/event-study", get(event_study))
        .add("/rule-test", get(rule_test))
        .add("/positions/{id}/result", get(position_result))
}

pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(
        paths(event_study, rule_test, position_result),
        components(schemas(
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
struct RuleQuery {
    /// Form of the filings (`8-K`, `10-Q`…).
    form: String,
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
    summary = "Test how instruments moved after a kind of filing",
    description = "Distribution of the abnormal return after every matching filing of the followed instruments, next to the same measure on drawn dates, with the number of events. Each filing is dated by its filing day. Instruments no longer followed are missing, which can flatter a result. An aid to analysis, not investment advice. Any org member.",
    params(RuleQuery),
    responses(
        (status = 200, body = RuleResult),
        (status = 400, description = "Invalid `form`, `from` or `to` (field → token)", body = crate::openapi::ErrorDetail),
        (status = 503, description = "`server-busy`: too many rule tests at once", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn rule_test(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<RuleQuery>,
) -> Result<Response> {
    if q.form.trim().is_empty() || q.form.len() > 16 {
        return Ok(field_error("form", err_codes::FIELD_INVALID));
    }
    let (from, to) = match (day("from", q.from.as_deref()), day("to", q.to.as_deref())) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(r), _) | (_, Err(r)) => return Ok(r),
    };
    let rule = Rule {
        form: q.form.trim().to_string(),
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
