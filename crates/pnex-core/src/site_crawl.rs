//! The `site_crawl` flow node (network-check.md D407): the server crawl of
//! D307 started by a flow, on the site and relevance rules of the message.
//! The flow's trigger says when a site is walked, its function says what
//! makes a page relevant (codes, titles, names); registry, gone pages and
//! proofs stay those of the `crawl_site` mission the server keeps per site.

use serde::{Deserialize, Serialize};

use crate::browser_agent::{Matcher, PageRecord, CRAWL_PAGES_MAX};

pub const NODE_PORT_PAGE: usize = 0;
pub const NODE_PORT_SUMMARY: usize = 1;
pub const NODE_PORT_FAILED: usize = 2;
pub const NODE_PORT_COUNT: usize = 3;

/// Characters of a page's text sent to the flow.
pub const TEXT_MAX: usize = 50_000;
/// Pages sent to the flow per crawl.
pub const PAGES_OUT_MAX: usize = 200;

/// `site_crawl` node config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteCrawlConfig {
    /// Pages read per crawl (default 200, at most 2,000).
    #[serde(default = "pages_default")]
    pub max_pages: u32,
    /// Path prefixes never visited (`/blog/`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Relevant pages new or changed get a proof (D308, D408).
    #[serde(default = "yes")]
    pub proofs: bool,
    /// Send only the relevant pages whose text changed since the last
    /// crawl (default: every relevant page, so a rule sees an accreditation
    /// that ended on a page that did not change).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub only_changed: bool,
    /// Relevant pages written as objects (the mission's `record`); its
    /// link goes to `msg.payload.link_to` (the website object).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<PageRecord>,
}

fn pages_default() -> u32 {
    crate::browser_agent::CRAWL_PAGES_DEFAULT
}

fn yes() -> bool {
    true
}

impl Default for SiteCrawlConfig {
    fn default() -> Self {
        Self {
            max_pages: pages_default(),
            exclude: Vec::new(),
            proofs: true,
            only_changed: false,
            record: None,
        }
    }
}

impl SiteCrawlConfig {
    pub fn check(&self) -> Option<(&'static str, &'static str)> {
        if self.max_pages == 0 || self.max_pages > CRAWL_PAGES_MAX {
            return Some(("site_crawl_config_invalid", "1 to 2,000 pages"));
        }
        if self.exclude.len() > 20 || self.exclude.iter().any(|x| !x.starts_with('/')) {
            return Some((
                "site_crawl_config_invalid",
                "at most 20 excluded paths, each starting with /",
            ));
        }
        if self.record.as_ref().is_some_and(|r| {
            !crate::ontology::schema::valid_key(&r.type_key) || !r.properties.contains_key("url")
        }) {
            return Some((
                "site_crawl_config_invalid",
                "the record needs a type and a property for the url",
            ));
        }
        None
    }
}

/// Runtime → backend: one site of a deployed node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiteCrawlNodeRequest {
    pub node: crate::ontology::flow::FlowNodeRef,
    pub url: String,
    #[serde(default, rename = "match")]
    pub matchers: Vec<Matcher>,
    /// Target of the record's link (`msg.payload.link_to`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_to: Option<String>,
}

/// A relevant page of the crawl.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SiteCrawlPage {
    pub url: String,
    pub title: String,
    pub text: String,
    pub page_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
    /// Its text differs from the last crawl (or it is new).
    pub changed: bool,
}

/// What the crawl did on the site.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SiteCrawlSummary {
    pub url: String,
    pub pages: usize,
    pub relevant: usize,
    pub truncated: bool,
    /// Why the crawl stopped (`crawl-blocked`, `crawl-robots-disallowed`,
    /// `crawl-unreachable`, `ext-registry-full`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
    pub mission_id: String,
}

/// Backend → runtime: the pages (port 0, one message each) then the
/// summary (port 1), or a refusal (port 2).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SiteCrawlNodeReply {
    pub port: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<SiteCrawlPage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<SiteCrawlSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub args: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_is_bounded() {
        assert!(SiteCrawlConfig::default().check().is_none());
        let c = SiteCrawlConfig {
            max_pages: 0,
            ..Default::default()
        };
        assert!(c.check().is_some());
        let c = SiteCrawlConfig {
            exclude: vec!["blog".into()],
            ..Default::default()
        };
        assert!(c.check().is_some());
        let c: SiteCrawlConfig = serde_json::from_str("{}").unwrap();
        assert_eq!((c.max_pages, c.proofs), (200, true));
    }
}
