//! `robots.txt` as the crawler reads it (D307): the group of our agent
//! (`pnex-crawler`) or else `*`, its `Allow` / `Disallow` path prefixes
//! (longest match wins, `Allow` on a tie, `*` and `$` understood), its
//! `Crawl-delay`, and the `Sitemap` lines.

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Robots {
    rules: Vec<(bool, String)>,
    pub crawl_delay_secs: Option<u64>,
    pub sitemaps: Vec<String>,
}

const AGENT: &str = "pnex-crawler";

/// A `User-agent` group: its agents, `(allow, pattern)` rules, delay.
type Group = (Vec<String>, Vec<(bool, String)>, Option<u64>);

impl Robots {
    pub fn parse(text: &str) -> Self {
        let mut out = Robots::default();
        // Groups: (agents, rules, delay).
        let mut groups: Vec<Group> = Vec::new();
        let mut open_agents = false;
        for line in text.lines().take(10_000) {
            let line = line.split('#').next().unwrap_or_default().trim();
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            match k.as_str() {
                "user-agent" => {
                    if !open_agents {
                        groups.push((Vec::new(), Vec::new(), None));
                        open_agents = true;
                    }
                    if let Some(g) = groups.last_mut() {
                        g.0.push(v.to_ascii_lowercase());
                    }
                }
                "allow" | "disallow" => {
                    open_agents = false;
                    if let Some(g) = groups.last_mut() {
                        // `Disallow:` (empty) allows everything.
                        if !v.is_empty() {
                            g.1.push((k == "allow", v.to_string()));
                        }
                    }
                }
                "crawl-delay" => {
                    open_agents = false;
                    if let Some(g) = groups.last_mut() {
                        g.2 = v.parse::<f64>().ok().map(|d| d.ceil() as u64);
                    }
                }
                "sitemap" => out.sitemaps.push(v.to_string()),
                _ => {}
            }
        }
        let pick = |name: &str| groups.iter().find(|g| g.0.iter().any(|a| a == name));
        if let Some((_, rules, delay)) = pick(AGENT).or_else(|| pick("*")) {
            out.rules = rules.clone();
            out.crawl_delay_secs = *delay;
        }
        out
    }

    /// May `path` (with its query) be fetched?
    pub fn allows(&self, path: &str) -> bool {
        let mut best: Option<(usize, bool)> = None;
        for (allow, pattern) in &self.rules {
            if let Some(len) = matches(pattern, path) {
                best = match best {
                    Some((l, a)) if l > len || (l == len && a) => Some((l, a)),
                    _ => Some((len, *allow)),
                };
            }
        }
        best.is_none_or(|(_, allow)| allow)
    }
}

/// Length of the pattern when it matches the start of `path`.
fn matches(pattern: &str, path: &str) -> Option<usize> {
    let (pat, anchored) = match pattern.strip_suffix('$') {
        Some(p) => (p, true),
        None => (pattern, false),
    };
    let parts: Vec<&str> = pat.split('*').collect();
    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            if !path.starts_with(part) {
                return None;
            }
            pos = part.len();
        } else {
            let found = path[pos..].find(part)?;
            pos += found + part.len();
        }
    }
    if anchored && pos != path.len() && !pat.ends_with('*') {
        return None;
    }
    Some(pattern.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_group_wins_and_longest_rule_decides() {
        let r = Robots::parse(
            "User-agent: *\nDisallow: /\n\nUser-agent: PNEX-Crawler\nUser-agent: other\nDisallow: /admin\nAllow: /admin/public\nDisallow: /*.pdf$\nCrawl-delay: 5\nSitemap: https://e.fr/sitemap.xml\n",
        );
        assert!(r.allows("/formations/rncp1"));
        assert!(!r.allows("/admin/x"));
        assert!(r.allows("/admin/public/page"));
        assert!(!r.allows("/docs/brochure.pdf"));
        assert!(r.allows("/docs/brochure.pdf?v=1"));
        assert_eq!(r.crawl_delay_secs, Some(5));
        assert_eq!(r.sitemaps, vec!["https://e.fr/sitemap.xml".to_string()]);
        let star = Robots::parse("User-agent: *\nDisallow: /\n");
        assert!(!star.allows("/"));
        assert!(Robots::parse("User-agent: *\nDisallow:\n").allows("/x"));
        assert!(Robots::parse("").allows("/x"));
    }
}
