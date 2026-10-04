//! Embedded knowledge base of the assistant (D142, layers 2–3).
//!
//! Cards live in `crates/pnex-backend/assistant-kb/*.md` (English, one per
//! feature, how-to or troubleshooting case) and are compiled into the
//! binary: the assistant depends on no external source. Search is lexical
//! (BM25 over title, tags and body, built once in memory) — the corpus is
//! small and product-wide, it never holds org data.
//!
//! Card format: a front-matter block between `---` lines with `key: value`
//! pairs (lists are comma-separated), then a Markdown body whose first
//! paragraph is the summary injected for the page the user is on.

use std::collections::HashMap;
use std::sync::LazyLock;

use include_dir::{include_dir, Dir};

static KB_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assistant-kb");

/// Allowed card kinds.
pub const KINDS: &[&str] = &["feature", "howto", "troubleshooting"];

/// Results returned by one search.
pub const SEARCH_MAX: usize = 5;
const SNIPPET_MAX_CHARS: usize = 240;

/// One parsed card.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub pages: Vec<String>,
    pub nodes: Vec<String>,
    pub err_codes: Vec<String>,
    pub tools: Vec<String>,
    pub tags: Vec<String>,
    /// First paragraph of the body.
    pub summary: String,
    pub body: String,
}

fn list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parses one card; `stem` is the file name without extension.
pub fn parse(stem: &str, text: &str) -> Result<Card, String> {
    let rest = text
        .strip_prefix("---\n")
        .ok_or_else(|| format!("{stem}: missing front-matter"))?;
    let (head, body) = rest
        .split_once("\n---\n")
        .ok_or_else(|| format!("{stem}: unterminated front-matter"))?;
    let mut fields: HashMap<&str, &str> = HashMap::new();
    for line in head.lines().filter(|l| !l.trim().is_empty()) {
        let (k, v) = line
            .split_once(':')
            .ok_or_else(|| format!("{stem}: front-matter line without ':' — {line}"))?;
        fields.insert(k.trim(), v.trim());
    }
    let get = |k: &str| fields.get(k).copied().unwrap_or("");
    let body = body.trim().to_string();
    let summary = body
        .split("\n\n")
        .map(str::trim)
        .find(|p| !p.is_empty() && !p.starts_with('#'))
        .unwrap_or("")
        .to_string();
    Ok(Card {
        id: get("id").to_string(),
        title: get("title").to_string(),
        kind: get("kind").to_string(),
        pages: list(get("pages")),
        nodes: list(get("nodes")),
        err_codes: list(get("err_codes")),
        tools: list(get("tools")),
        tags: list(get("tags")),
        summary,
        body,
    })
}

/// Every embedded card, parsed once; a malformed card is skipped (the
/// guard tests make it a build failure).
pub static CARDS: LazyLock<Vec<Card>> = LazyLock::new(|| {
    let mut cards: Vec<Card> = KB_DIR
        .files()
        .filter(|f| f.path().extension().is_some_and(|e| e == "md"))
        .filter_map(|f| {
            let stem = f.path().file_stem()?.to_str()?;
            let text = f.contents_utf8()?;
            match parse(stem, text) {
                Ok(card) => Some(card),
                Err(e) => {
                    tracing::warn!("assistant knowledge card skipped: {e}");
                    None
                }
            }
        })
        .collect();
    cards.sort_by(|a, b| a.id.cmp(&b.id));
    cards
});

pub fn card(id: &str) -> Option<&'static Card> {
    CARDS.iter().find(|c| c.id == id)
}

/// Feature card of a UI path (`/flows`, `/dashboards?id=…`): exact route
/// first, then the longest route that prefixes the path.
pub fn card_for_page(path: &str) -> Option<&'static Card> {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let path = if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        path
    };
    CARDS
        .iter()
        .filter(|c| c.kind == "feature")
        .flat_map(|c| c.pages.iter().map(move |p| (c, p.as_str())))
        .filter(|(_, p)| {
            *p == path || (*p != "/" && path.starts_with(p) && path[p.len()..].starts_with('/'))
        })
        .max_by_key(|(_, p)| p.len())
        .map(|(c, _)| c)
}

// ─────────────────────────── BM25 ───────────────────────────

const K1: f64 = 1.2;
const B: f64 = 0.75;
/// Title and tags count as much as this many body occurrences.
const FIELD_BOOST: usize = 3;

/// Lowercase alphanumeric tokens, ≥ 2 chars, naive plural folding.
pub fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 2)
        .map(|t| {
            let t = t.to_lowercase();
            match t.strip_suffix('s') {
                Some(stem) if stem.chars().count() >= 3 && !stem.ends_with('s') => stem.to_string(),
                _ => t,
            }
        })
        .collect()
}

struct Index {
    /// Term frequencies per card (same order as `CARDS`).
    tf: Vec<HashMap<String, usize>>,
    lengths: Vec<usize>,
    avg_len: f64,
    /// Number of cards containing each term.
    df: HashMap<String, usize>,
}

static INDEX: LazyLock<Index> = LazyLock::new(|| {
    let mut tf = Vec::with_capacity(CARDS.len());
    let mut lengths = Vec::with_capacity(CARDS.len());
    let mut df: HashMap<String, usize> = HashMap::new();
    for card in CARDS.iter() {
        let boosted = format!(
            "{} {} {}",
            card.title,
            card.tags.join(" "),
            card.id.replace('-', " ")
        );
        let mut counts: HashMap<String, usize> = HashMap::new();
        for t in tokens(&boosted) {
            *counts.entry(t).or_default() += FIELD_BOOST;
        }
        for t in tokens(&card.body) {
            *counts.entry(t).or_default() += 1;
        }
        for term in counts.keys() {
            *df.entry(term.clone()).or_default() += 1;
        }
        lengths.push(counts.values().sum());
        tf.push(counts);
    }
    let avg_len = if lengths.is_empty() {
        1.0
    } else {
        lengths.iter().sum::<usize>() as f64 / lengths.len() as f64
    };
    Index {
        tf,
        lengths,
        avg_len,
        df,
    }
});

/// One search result.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Hit {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub snippet: String,
    pub score: f64,
}

/// Paragraph of the body sharing the most terms with the query.
fn snippet(card: &Card, terms: &[String]) -> String {
    let best = card
        .body
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty() && !p.starts_with('#'))
        .max_by_key(|p| {
            let toks = tokens(p);
            terms.iter().filter(|t| toks.contains(t)).count()
        })
        .unwrap_or(&card.summary);
    match best.char_indices().nth(SNIPPET_MAX_CHARS) {
        Some((cut, _)) => format!("{}…", &best[..cut]),
        None => best.to_string(),
    }
}

/// BM25 search over the cards, best first, at most `limit` (≤ 5) hits
/// with a positive score.
pub fn search(query: &str, limit: usize) -> Vec<Hit> {
    let mut terms = tokens(query);
    terms.sort();
    terms.dedup();
    let n = CARDS.len() as f64;
    let index = &*INDEX;
    let mut scored: Vec<(usize, f64)> = (0..CARDS.len())
        .map(|i| {
            let len = index.lengths[i] as f64;
            let score = terms
                .iter()
                .map(|t| {
                    let f = *index.tf[i].get(t).unwrap_or(&0) as f64;
                    if f == 0.0 {
                        return 0.0;
                    }
                    let df = *index.df.get(t).unwrap_or(&0) as f64;
                    let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                    idf * f * (K1 + 1.0) / (f + K1 * (1.0 - B + B * len / index.avg_len))
                })
                .sum::<f64>();
            (i, score)
        })
        .filter(|(_, s)| *s > 0.0)
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored
        .into_iter()
        .take(limit.clamp(1, SEARCH_MAX))
        .map(|(i, score)| {
            let card = &CARDS[i];
            Hit {
                id: card.id.clone(),
                title: card.title.clone(),
                kind: card.kind.clone(),
                snippet: snippet(card, &terms),
                score: (score * 100.0).round() / 100.0,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Top-level UI routes of the shell, scanned from the router so a new
    /// page cannot ship without a card (dev-only and public routes aside).
    fn ui_routes() -> BTreeSet<String> {
        let src = include_str!("../../../../pnex-frontend/src/app.rs");
        src.lines()
            .filter_map(|l| {
                let rest = l.trim().strip_prefix("#[route(\"")?;
                let route = rest.split('"').next()?;
                Some(route.split('?').next().unwrap_or(route).to_string())
            })
            .filter(|r| {
                !r.starts_with("/auth/")
                    && !r.starts_with("/share/")
                    && !r.starts_with("/_")
                    && !r.starts_with("/:")
            })
            .collect()
    }

    #[test]
    fn every_card_parses_and_is_well_formed() {
        let files: Vec<_> = KB_DIR
            .files()
            .filter(|f| f.path().extension().is_some_and(|e| e == "md"))
            .collect();
        assert_eq!(
            files.len(),
            CARDS.len(),
            "a card failed to parse (see warn logs)"
        );
        for f in files {
            let stem = f.path().file_stem().and_then(|s| s.to_str()).expect("stem");
            let card = parse(stem, f.contents_utf8().expect("utf8")).expect("parses");
            assert_eq!(card.id, stem, "id must equal the file name");
            assert!(!card.title.is_empty(), "{stem}: empty title");
            assert!(
                KINDS.contains(&card.kind.as_str()),
                "{stem}: kind {}",
                card.kind
            );
            assert!(!card.summary.is_empty(), "{stem}: empty summary");
        }
    }

    #[test]
    fn cards_only_reference_existing_things() {
        let routes = ui_routes();
        assert!(routes.len() >= 20, "route scan broke: {routes:?}");
        for c in CARDS.iter() {
            for p in &c.pages {
                assert!(routes.contains(p), "{}: unknown page {p}", c.id);
            }
            for n in &c.nodes {
                assert!(
                    pnex_core::node_doc(n).is_some(),
                    "{}: unknown node kind {n}",
                    c.id
                );
            }
            for e in &c.err_codes {
                assert!(
                    pnex_core::err_codes::exists(e),
                    "{}: unknown err code {e}",
                    c.id
                );
            }
            for t in &c.tools {
                assert!(
                    super::super::tools::tool_specs()
                        .iter()
                        .any(|s| s.name == t),
                    "{}: unknown tool {t}",
                    c.id
                );
            }
        }
    }

    #[test]
    fn every_ui_route_has_a_feature_card() {
        let covered: BTreeSet<&str> = CARDS
            .iter()
            .filter(|c| c.kind == "feature")
            .flat_map(|c| c.pages.iter().map(String::as_str))
            .collect();
        let missing: Vec<_> = ui_routes()
            .into_iter()
            .filter(|r| !covered.contains(r.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "UI routes without a feature card: {missing:?}"
        );
    }

    #[test]
    fn every_tool_is_described_by_a_card() {
        let described: BTreeSet<&str> = CARDS
            .iter()
            .flat_map(|c| c.tools.iter().map(String::as_str))
            .collect();
        let missing: Vec<&str> = super::super::tools::tool_specs()
            .iter()
            .map(|s| s.name)
            .filter(|n| !described.contains(n))
            .collect();
        assert!(
            missing.is_empty(),
            "assistant tools without a card: {missing:?}"
        );
    }

    /// "The UI is the only user interface": no card tells a user to run a
    /// command or call the API.
    #[test]
    fn cards_never_send_users_to_a_cli_or_the_api() {
        for c in CARDS.iter() {
            let body = c.body.to_lowercase();
            for forbidden in ["/api/v1", "curl ", "cargo ", "psql", "kubectl", "docker "] {
                assert!(
                    !body.contains(forbidden),
                    "{}: mentions {forbidden:?}",
                    c.id
                );
            }
        }
    }

    /// Typical user questions find the right card first (real corpus).
    #[test]
    fn typical_questions_find_their_card() {
        for (query, expected) in [
            (
                "my device sends no telemetry",
                "troubleshooting-no-telemetry",
            ),
            (
                "pin already assigned deploy refused",
                "troubleshooting-pin-already-assigned",
            ),
            (
                "notification never sent trigger",
                "troubleshooting-notify-not-sent",
            ),
            ("conversation history privacy erase", "assistant"),
        ] {
            let hits = search(query, 3);
            assert!(
                hits.iter().any(|h| h.id == expected),
                "{query:?} → {:?}",
                hits.iter().map(|h| &h.id).collect::<Vec<_>>()
            );
        }
        assert_eq!(
            card_for_page("/flows").map(|c| c.id.as_str()),
            Some("flows")
        );
        assert_eq!(
            card_for_page("/dashboards?id=x&mode=edit").map(|c| c.id.as_str()),
            Some("dashboards")
        );
        assert_eq!(
            card_for_page("/orgs/current").map(|c| c.id.as_str()),
            Some("org-detail")
        );
    }

    #[test]
    fn parse_reads_front_matter_lists_and_summary() {
        let card = parse(
            "x",
            "---\nid: x\ntitle: X\nkind: howto\npages: /flows, /map\ntags: a,b\n---\n# Head\n\nThe summary.\n\nMore.\n",
        )
        .expect("parses");
        assert_eq!(card.pages, vec!["/flows", "/map"]);
        assert_eq!(card.tags, vec!["a", "b"]);
        assert_eq!(card.summary, "The summary.");
        assert!(parse("y", "no front matter").is_err());
    }

    #[test]
    fn tokens_fold_case_and_plurals() {
        assert_eq!(
            tokens("Flows, DEVICE-read pins!"),
            vec!["flow", "device", "read", "pin"]
        );
        assert_eq!(tokens("class"), vec!["class"]);
    }
}
