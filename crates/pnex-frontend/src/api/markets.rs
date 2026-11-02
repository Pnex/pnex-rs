//! Event studies (markets-research.md D532, D535).

use pnex_core::market_study::{EventStudy, PositionResult, RuleResult};
use reqwest::Method;

use crate::api::client;
use crate::api::error::ApiError;
use crate::api::media::urlencode;

const BASE: &str = "/api/v1/markets";

pub async fn event_study(instrument: &str, date: &str) -> Result<Option<EventStudy>, ApiError> {
    client::request(
        Method::GET,
        &format!("{BASE}/instruments/{instrument}/event-study?date={date}"),
        None,
    )
    .await
}

pub async fn rule_test(form: &str, item: &str) -> Result<RuleResult, ApiError> {
    client::request(
        Method::GET,
        &format!(
            "{BASE}/rule-test?form={}&item={}",
            urlencode(form),
            urlencode(item)
        ),
        None,
    )
    .await
}

pub async fn position_result(id: &str) -> Result<Option<PositionResult>, ApiError> {
    client::request(Method::GET, &format!("{BASE}/positions/{id}/result"), None).await
}
