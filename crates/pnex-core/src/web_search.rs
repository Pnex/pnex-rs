//! Web search providers of an org and the `web_search` flow node
//! (web-presence.md D360–D361). The node returns candidates and writes
//! nothing; the provider key lives in the org vault.

use serde::{Deserialize, Serialize};

/// Closed list of provider kinds; one module each on the server.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum WebSearchKind {
    /// Serper (`POST /search`, `X-API-KEY`).
    #[default]
    Serper,
    /// A SearXNG instance (`GET /search?format=json`, key optional).
    Searxng,
}

impl WebSearchKind {
    pub const ALL: [Self; 2] = [Self::Serper, Self::Searxng];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Serper => "serper",
            Self::Searxng => "searxng",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// Base URL used when the provider leaves it empty (`None`: required).
    pub const fn default_base_url(self) -> Option<&'static str> {
        match self {
            Self::Serper => Some("https://google.serper.dev"),
            Self::Searxng => None,
        }
    }

    pub const fn key_required(self) -> bool {
        matches!(self, Self::Serper)
    }
}

/// Read model; the key is a vault reference, never its value (R4, R16).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WebSearchProvider {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub name: String,
    pub kind: WebSearchKind,
    /// Empty = the kind's default address.
    pub base_url: String,
    pub api_key: Option<crate::SecretFieldView>,
    pub is_default: bool,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// Create or replace a provider; `api_key: None` keeps the current key.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WebSearchProviderInput {
    pub name: String,
    #[serde(default)]
    pub kind: WebSearchKind,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<crate::SecretFieldInput>,
    #[serde(default)]
    pub is_default: bool,
}

/// Outcome of a provider test: one real query, nothing stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WebSearchTestResult {
    pub ok: bool,
    pub latency_ms: u64,
    pub results: u32,
    /// Diagnostic as is, not translated.
    pub detail: Option<String>,
}

/// One candidate of a search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebSearchHit {
    pub url: String,
    pub title: String,
    pub snippet: String,
    /// 1-based, after exclusions.
    pub rank: u32,
    /// Host without `www.`.
    pub domain: String,
}

pub const LIMIT_MAX: u32 = 20;
pub const QUERY_MAX: usize = 400;
const EXCLUDE_MAX: usize = 100;

/// Directories and social networks: they name a company, they are not its site.
pub const DEFAULT_EXCLUDED: [&str; 14] = [
    "facebook.com",
    "linkedin.com",
    "x.com",
    "twitter.com",
    "instagram.com",
    "youtube.com",
    "societe.com",
    "pappers.fr",
    "verif.com",
    "infogreffe.fr",
    "annuaire-entreprises.data.gouv.fr",
    "pagesjaunes.fr",
    "manageo.fr",
    "wikipedia.org",
];

fn default_limit() -> u32 {
    10
}

fn default_excluded() -> Vec<String> {
    DEFAULT_EXCLUDED.iter().map(|s| s.to_string()).collect()
}

/// `web_search` node config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchConfig {
    /// Provider name; empty = the org default.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// ISO 3166-1 alpha-2, e.g. `fr`; empty = the provider's choice.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub country: String,
    /// ISO 639-1, e.g. `fr`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lang: String,
    /// Results on these domains (or their subdomains) are dropped.
    #[serde(default = "default_excluded")]
    pub exclude_domains: Vec<String>,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            provider: String::new(),
            limit: default_limit(),
            country: String::new(),
            lang: String::new(),
            exclude_domains: default_excluded(),
        }
    }
}

impl WebSearchConfig {
    /// Shape errors, shown while editing.
    pub fn check(&self) -> Option<(&'static str, &'static str)> {
        let bad = |m| Some(("web_search_config_invalid", m));
        if !(1..=LIMIT_MAX).contains(&self.limit) {
            return bad("limit must be between 1 and 20");
        }
        let code =
            |s: &str| s.is_empty() || (s.len() == 2 && s.bytes().all(|b| b.is_ascii_alphabetic()));
        if !code(&self.country) || !code(&self.lang) {
            return bad("country and lang are two-letter codes");
        }
        if self.provider.chars().count() > 255
            || self.exclude_domains.len() > EXCLUDE_MAX
            || self
                .exclude_domains
                .iter()
                .any(|d| d.is_empty() || d.len() > 255)
        {
            return bad("at most 100 excluded domains");
        }
        None
    }

    /// Whether `domain` is excluded (itself or a subdomain of an entry).
    pub fn excludes(&self, domain: &str) -> bool {
        self.exclude_domains.iter().any(|e| {
            let e = e.trim().trim_start_matches("www.").to_ascii_lowercase();
            domain == e || domain.ends_with(&format!(".{e}"))
        })
    }
}

/// Host of `url` in lower case, without `www.` (`None`: not http(s)).
pub fn domain_of(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    let host = u.host_str()?.to_ascii_lowercase();
    Some(host.strip_prefix("www.").unwrap_or(&host).to_string())
}

/// Cuts `s` to `max` characters.
pub fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Bounds of one hit's texts.
pub const TITLE_MAX: usize = 500;
pub const SNIPPET_MAX: usize = 2000;
pub const URL_MAX: usize = 2048;

/// Turns raw `(url, title, snippet)` into ranked hits: non-http(s) and
/// excluded domains dropped, duplicates of a URL kept once, `limit` kept.
pub fn rank(raw: Vec<(String, String, String)>, config: &WebSearchConfig) -> Vec<WebSearchHit> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (url, title, snippet) in raw {
        if out.len() >= config.limit.min(LIMIT_MAX) as usize {
            break;
        }
        if url.len() > URL_MAX {
            continue;
        }
        let Some(domain) = domain_of(&url) else {
            continue;
        };
        if config.excludes(&domain) || !seen.insert(url.clone()) {
            continue;
        }
        out.push(WebSearchHit {
            rank: out.len() as u32 + 1,
            url,
            title: clip(&title, TITLE_MAX),
            snippet: clip(&snippet, SNIPPET_MAX),
            domain,
        });
    }
    out
}

/// Ports of the node: 0 results (possibly none), 1 failed.
pub const NODE_PORT_OK: usize = 0;
pub const NODE_PORT_FAILED: usize = 1;
pub const NODE_PORT_COUNT: usize = 2;

/// Runtime → backend: one query of a deployed node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebSearchNodeRequest {
    pub node: crate::ontology::flow::FlowNodeRef,
    pub query: String,
}

/// Backend → runtime.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WebSearchNodeReply {
    pub port: usize,
    #[serde(default)]
    pub results: Vec<WebSearchHit>,
    /// Provider name used.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub args: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_bounds() {
        let mut c = WebSearchConfig::default();
        assert!(c.check().is_none());
        c.limit = 0;
        assert!(c.check().is_some());
        c.limit = 5;
        c.country = "fra".into();
        assert!(c.check().is_some());
        c.country = "fr".into();
        assert!(c.check().is_none());
    }

    #[test]
    fn rank_drops_excluded_duplicates_and_odd_urls() {
        let c = WebSearchConfig {
            limit: 2,
            ..Default::default()
        };
        let raw = vec![
            (
                "https://fr.linkedin.com/company/acme".into(),
                "ACME".into(),
                "".into(),
            ),
            ("ftp://acme.fr".into(), "".into(), "".into()),
            (
                "https://www.Acme.fr/".into(),
                "ACME".into(),
                "Formations".into(),
            ),
            ("https://www.Acme.fr/".into(), "ACME".into(), "".into()),
            ("https://b.example.org/x".into(), "B".into(), "".into()),
            ("https://c.example.org".into(), "C".into(), "".into()),
        ];
        let hits = rank(raw, &c);
        assert_eq!(hits.len(), 2);
        assert_eq!((hits[0].rank, hits[0].domain.as_str()), (1, "acme.fr"));
        assert_eq!(hits[1].domain, "b.example.org");
    }
}
