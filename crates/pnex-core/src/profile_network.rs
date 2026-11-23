//! Profile networks as data (pipelines.md D951, PE9): which addresses are
//! one person's profile page, or a company's employee list, on a network a
//! pack declares. The engine keeps the grammar only; hosts, path shapes and
//! reserved words live in the pack (`packs/<pack>/networks/<key>.yaml`).
//!
//! Whitelist only (D254, D779, D976): an address matches a declared shape
//! exactly (segment count, charset, length, reserved words) or it is
//! refused, so a capture never becomes a way to discover people.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::browser_agent::ProfileRefusal;

/// One network of a pack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileNetwork {
    pub key: String,
    /// Hosts of the network: each one or any of its subdomains.
    pub domains: Vec<String>,
    /// Host its canonical addresses carry, the site its rules play on.
    pub site: String,
    /// Shapes of one person's profile page.
    pub profiles: Vec<PageShape>,
    /// Shapes of a company's employee list (D779).
    #[serde(default)]
    pub people: Vec<PageShape>,
    /// What each `{name}` of a shape may hold.
    pub names: BTreeMap<String, NameRule>,
    /// How the browser agent follows an account (D283), on a video platform.
    #[serde(default)]
    pub follow: Option<Follow>,
}

/// How the browser agent follows an account of the network.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Follow {
    /// Pages of an account it reads, in order, `{handle}` replaced.
    pub pages: Vec<String>,
    /// Sites it must be allowed on: the pages, and where files come from.
    pub sites: Vec<String>,
    /// Address of a post, `{author}` and `{id}` replaced.
    pub post: String,
    /// Address of a short video, when the platform has another one.
    #[serde(default)]
    pub short: Option<String>,
    /// Responses of the web app it reads: their address holds one of these.
    #[serde(default)]
    pub responses: Vec<String>,
    /// Global of a list page that holds its first posts.
    #[serde(default)]
    pub page_data: Option<String>,
    /// How the posts are found in a response.
    pub posts: crate::post_reader::PostReader,
    /// A caption track read on the post's own page.
    #[serde(default)]
    pub captions: Option<crate::post_reader::PageTrack>,
    /// Form of the platform's caption files.
    #[serde(default)]
    pub cues: crate::post_reader::Cues,
}

impl Follow {
    /// The plan of the agent for the account `handle`.
    pub fn plan(&self, handle: &str) -> crate::browser_agent::FollowPlan {
        crate::browser_agent::FollowPlan {
            pages: self
                .pages
                .iter()
                .map(|p| p.replace("{handle}", handle))
                .collect(),
            sites: self.sites.clone(),
            post: self.post.clone(),
            short: self.short.clone(),
            responses: self.responses.clone(),
            page_data: self.page_data.clone(),
            posts: Some(self.posts.clone()),
            captions: self.captions.clone(),
            cues: self.cues.clone(),
        }
    }
}

/// A page: its path segments (lowercased before matching), the query
/// parameters it reads, and its canonical address.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageShape {
    /// A literal segment, or a literal prefix then one `{name}`
    /// (`"@{h}"`). The count must match exactly.
    pub path: Vec<String>,
    /// Query parameter → `{name}` it fills; required when listed.
    #[serde(default)]
    pub query: BTreeMap<String, String>,
    /// Address template, `{name}` replaced by its value.
    pub canonical: String,
}

/// What a `{name}` may hold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NameRule {
    /// Characters allowed besides `a-z0-9` (`"-_%"`), or all of them when
    /// `digits` is set.
    #[serde(default)]
    pub chars: String,
    /// Digits only.
    #[serde(default)]
    pub digits: bool,
    pub max: usize,
    /// Values that are pages of the network, not names.
    #[serde(default)]
    pub not: Vec<String>,
}

impl NameRule {
    fn accepts(&self, v: &str) -> bool {
        !v.is_empty()
            && v.len() <= self.max
            && !self.not.iter().any(|n| n == v)
            && v.chars().all(|c| match self.digits {
                true => c.is_ascii_digit(),
                false => c.is_ascii_alphanumeric() || self.chars.contains(c),
            })
    }
}

/// The network of an http(s) address without credentials, and the address.
fn network_of<'a>(
    nets: &'a [ProfileNetwork],
    raw: &str,
) -> Result<(&'a ProfileNetwork, url::Url), ProfileRefusal> {
    let url = url::Url::parse(raw.trim()).map_err(|_| ProfileRefusal::NotNetwork)?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() {
        return Err(ProfileRefusal::NotNetwork);
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let on = |d: &String| host == *d || host.ends_with(&format!(".{d}"));
    nets.iter()
        .find(|n| n.domains.iter().any(on))
        .map(|n| (n, url))
        .ok_or(ProfileRefusal::NotNetwork)
}

/// The canonical address of the first shape the address matches.
fn matched(net: &ProfileNetwork, shapes: &[PageShape], url: &url::Url) -> Option<String> {
    let segs: Vec<String> = url
        .path_segments()
        .map(|s| {
            s.filter(|x| !x.is_empty())
                .map(str::to_ascii_lowercase)
                .collect()
        })
        .unwrap_or_default();
    'shape: for shape in shapes {
        if shape.path.len() != segs.len() {
            continue;
        }
        let mut values = BTreeMap::new();
        for (pat, seg) in shape.path.iter().zip(&segs) {
            match pat.split_once('{') {
                None if pat == seg => {}
                Some((prefix, rest)) => {
                    let name = rest.trim_end_matches('}');
                    let Some(v) = seg.strip_prefix(prefix) else {
                        continue 'shape;
                    };
                    if !net.names.get(name).is_some_and(|r| r.accepts(v)) {
                        continue 'shape;
                    }
                    values.insert(name.to_string(), v.to_string());
                }
                None => continue 'shape,
            }
        }
        for (param, name) in &shape.query {
            let v = url.query_pairs().find(|(k, _)| k == param).map(|(_, v)| v);
            match v {
                Some(v) if net.names.get(name).is_some_and(|r| r.accepts(&v)) => {
                    values.insert(name.clone(), v.into_owned());
                }
                _ => continue 'shape,
            }
        }
        let mut out = shape.canonical.clone();
        for (k, v) in &values {
            out = out.replace(&format!("{{{k}}}"), v);
        }
        return Some(out);
    }
    None
}

/// The network key and canonical address of one person's profile page (D333).
pub fn profile_in(nets: &[ProfileNetwork], raw: &str) -> Result<(String, String), ProfileRefusal> {
    let (net, url) = network_of(nets, raw)?;
    matched(net, &net.profiles, &url)
        .map(|c| (net.key.clone(), c))
        .ok_or(ProfileRefusal::NotProfile)
}

/// Canonical address of a company's employee list (D779), on a network
/// that declares one.
pub fn people_in(nets: &[ProfileNetwork], raw: &str) -> Result<String, ProfileRefusal> {
    let (net, url) = network_of(nets, raw)?;
    if net.people.is_empty() {
        return Err(ProfileRefusal::NotNetwork);
    }
    matched(net, &net.people, &url).ok_or(ProfileRefusal::NotProfile)
}

/// Faults of a network: `{name}`s not declared, an empty shape list.
pub fn violations(net: &ProfileNetwork) -> Vec<String> {
    let mut out = Vec::new();
    if net.domains.is_empty() || net.profiles.is_empty() || net.site.is_empty() {
        out.push(format!(
            "{}: domains, site and profiles are required",
            net.key
        ));
    }
    for shape in net.profiles.iter().chain(&net.people) {
        let names = shape
            .path
            .iter()
            .filter_map(|p| p.split_once('{').map(|(_, r)| r.trim_end_matches('}')))
            .chain(shape.query.values().map(String::as_str));
        for n in names {
            if !net.names.contains_key(n) {
                out.push(format!("{}: {{{n}}} has no rule in names", net.key));
            }
        }
    }
    out
}
