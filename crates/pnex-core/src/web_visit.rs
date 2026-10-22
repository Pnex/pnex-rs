//! The `web_visit` flow node (web-presence.md D364): one look at a
//! candidate website to identify it, never a crawl. At most three requests
//! on the candidate's origin (`robots.txt`, the page, its legal notice),
//! robots obeyed, identified client, 2 s between two requests to a host.
//! It writes nothing.

use serde::{Deserialize, Serialize};

pub const NODE_PORT_OK: usize = 0;
pub const NODE_PORT_FAILED: usize = 1;
pub const NODE_PORT_COUNT: usize = 2;

/// Characters kept of each page's readable text.
pub const TEXT_MAX: usize = 50_000;
pub const URL_MAX: usize = 2048;

/// Path fragments of a legal notice or an about page (closed list): the
/// first internal link whose path holds one is the third request.
pub const LEGAL_PATHS: &[&str] = &[
    "mentions-legales",
    "mentions_legales",
    "mentionslegales",
    "legal",
    "cgv",
    "conditions-generales",
    "qui-sommes-nous",
    "a-propos",
    "about",
    "imprint",
    "impressum",
];

/// `web_visit` node config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebVisitConfig {
    /// Also read the legal notice when the page links to one.
    #[serde(default = "yes")]
    pub legal_page: bool,
    /// Also capture the page with the org's own browser, kept as an image
    /// of its media library (web-presence.md Q-W4); none configured = no
    /// capture.
    #[serde(default)]
    pub screenshot: bool,
}

fn yes() -> bool {
    true
}

impl Default for WebVisitConfig {
    fn default() -> Self {
        Self {
            legal_page: true,
            screenshot: false,
        }
    }
}

/// The first link of `links` (absolute URLs) whose path names a legal
/// notice, on the same origin as `origin`.
pub fn legal_link<'a>(origin: &str, links: &'a [String]) -> Option<&'a str> {
    links
        .iter()
        .map(String::as_str)
        .filter(|l| l.starts_with(origin))
        .find(|l| {
            let path = l[origin.len()..].to_ascii_lowercase();
            LEGAL_PATHS.iter().any(|p| path.contains(p))
        })
}

/// Text cut at `TEXT_MAX` characters.
pub fn clip(text: &str) -> String {
    text.chars().take(TEXT_MAX).collect()
}

/// Runtime → backend: one visit of a deployed node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebVisitNodeRequest {
    pub node: crate::ontology::flow::FlowNodeRef,
    pub url: String,
}

/// Backend → runtime.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WebVisitNodeReply {
    pub port: usize,
    /// Where the page was read, after same-origin redirects.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub final_url: String,
    #[serde(default)]
    pub status: u16,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub legal_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub legal_text: String,
    /// Media asset of the capture, when asked and a browser answered.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub screenshot_asset_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub args: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_legal_notice_is_an_internal_link_of_the_closed_list() {
        let links = vec![
            "https://other.fr/mentions-legales".to_string(),
            "https://acme.fr/produits".to_string(),
            "https://acme.fr/Mentions-Legales.html".to_string(),
            "https://acme.fr/cgv".to_string(),
        ];
        assert_eq!(
            legal_link("https://acme.fr", &links),
            Some("https://acme.fr/Mentions-Legales.html")
        );
        assert_eq!(legal_link("https://acme.fr", &links[..2]), None);
        assert_eq!(clip(&"é".repeat(TEXT_MAX + 5)).chars().count(), TEXT_MAX);
    }
}
