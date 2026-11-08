//! Event studies (markets-research.md D532, D535).

use pnex_core::market_study::{EventStudy, PositionResult, RuleResult};
use pnex_core::markets::moves::MarketSession;
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

/// The moves and world events around a session (D738, D742).
pub async fn session(date: &str) -> Result<MarketSession, ApiError> {
    client::request(Method::GET, &format!("{BASE}/sessions/{date}"), None).await
}

/// A rule on event dates measured on one series (D743): `dates` is
/// `world_events` or `market_moves`, each filter empty for "any".
pub async fn rule_test_on_events(
    dates: &str,
    series: &str,
    filters: &[(&str, String)],
) -> Result<RuleResult, ApiError> {
    let mut url = format!(
        "{BASE}/rule-test?dates={}&series={}",
        urlencode(dates),
        urlencode(series)
    );
    for (k, v) in filters.iter().filter(|(_, v)| !v.is_empty()) {
        url.push_str(&format!("&{k}={}", urlencode(v)));
    }
    client::request(Method::GET, &url, None).await
}

/// The world events placed inside a map view, last 30 days (D739).
pub async fn world_events(
    west: f64,
    south: f64,
    east: f64,
    north: f64,
) -> Result<pnex_core::Paginated<pnex_core::markets::moves::WorldEventPoint>, ApiError> {
    client::request(
        Method::GET,
        &format!("{BASE}/world-events?west={west}&south={south}&east={east}&north={north}"),
        None,
    )
    .await
}

/// Closes of an issuer's instrument with its sessions and filings (D813).
pub async fn issuer_chart(
    id: &str,
    days: u32,
) -> Result<pnex_core::markets::chart::IssuerChart, ApiError> {
    client::request(
        Method::GET,
        &format!("{BASE}/issuers/{id}/chart?days={days}"),
        None,
    )
    .await
}
