//! Events API (camera-video.md D84) — JSON events stored as OpenObserve
//! logs, written by the flow `event-log` node.

use pnex_core::events::EventRecord;
use serde::Deserialize;

use crate::api::client;
use crate::api::error::ApiError;
use crate::api::media::urlencode;

/// D14 page of events + `available` (false = O2 not configured, or the org
/// never wrote an event).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EventPage {
    pub count: i64,
    #[serde(default)]
    pub results: Vec<EventRecord>,
    #[serde(default = "default_true")]
    pub available: bool,
}

fn default_true() -> bool {
    true
}

/// `GET /api/v1/events/streams` — event streams of the org (`ev_…`).
pub async fn streams() -> Result<Vec<String>, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/events/streams", None).await
}

/// Event list filters.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct EventFilters {
    pub stream: Option<String>,
    pub level: Option<String>,
    /// Full-text search.
    pub q: Option<String>,
    /// RFC 3339 bounds.
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl EventFilters {
    pub(crate) fn to_query(&self) -> String {
        let mut pairs: Vec<String> = Vec::new();
        for (key, value) in [
            ("stream", &self.stream),
            ("level", &self.level),
            ("q", &self.q),
            ("from", &self.from),
            ("to", &self.to),
        ] {
            if let Some(v) = value.as_deref().filter(|v| !v.trim().is_empty()) {
                pairs.push(format!("{key}={}", urlencode(v.trim())));
            }
        }
        if let Some(limit) = self.limit {
            pairs.push(format!("limit={limit}"));
        }
        if let Some(offset) = self.offset {
            pairs.push(format!("offset={offset}"));
        }
        if pairs.is_empty() {
            String::new()
        } else {
            format!("?{}", pairs.join("&"))
        }
    }
}

/// `GET /api/v1/events` — D14 envelope + `available`.
pub async fn list(filters: &EventFilters) -> Result<EventPage, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/events{}", filters.to_query()),
        None,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_skips_blank_filters() {
        let f = EventFilters {
            stream: Some("ev_doors".into()),
            level: Some(String::new()),
            q: Some("front door".into()),
            limit: Some(10),
            offset: Some(0),
            ..Default::default()
        };
        assert_eq!(
            f.to_query(),
            "?stream=ev_doors&q=front%20door&limit=10&offset=0"
        );
        assert_eq!(EventFilters::default().to_query(), "");
    }

    #[test]
    fn page_defaults_available() {
        let page: EventPage =
            serde_json::from_str(r#"{"count":0,"next":null,"previous":null,"results":[]}"#)
                .unwrap();
        assert!(page.available);
        let page: EventPage =
            serde_json::from_str(r#"{"count":0,"results":[],"available":false}"#).unwrap();
        assert!(!page.available);
    }
}
