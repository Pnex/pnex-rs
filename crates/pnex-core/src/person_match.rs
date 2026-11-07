//! Matching a person entered by a sales rep with a network profile
//! (agent-unification.md D788). Pure and wasm-safe, and site-agnostic: the
//! queries typed in a search bar, the candidates read from the cards of its
//! results, the score of an opened profile and what that score allows. Where
//! and how a network is searched is a default navigation rule
//! (`default_rules::person_search`), never code; the server keeps the state.

use crate::browser_agent as ba;
use crate::fold::fold_words;
use serde::{Deserialize, Serialize};

/// Queries of the ladder kept at most, from the most precise to the widest.
pub const QUERIES_MAX: usize = 5;

/// Result cards considered per search; the default rule opens the first
/// three profiles.
pub const CANDIDATES_PER_SEARCH: usize = 5;

/// Searches of one org per day, whatever its setting asks (D788); each is a
/// rule run of a browser agent, counted in its daily runs besides.
pub const SEARCHES_PER_DAY_MAX: i64 = 30;

/// Characters of a query, like a search bar keeps them.
const QUERY_MAX: usize = 120;

/// Text right after the name on a profile: headline, place and the current
/// position's company, before the history of positions.
const HEADER_CHARS: usize = 600;

/// Weights of the score (D788). The write threshold of the pack (80 by
/// default) needs the name and the current company.
pub const W_NAME: u32 = 30;
pub const W_NAME_PARTIAL: u32 = 10;
pub const W_CURRENT: u32 = 50;
pub const W_PAST: u32 = 25;
pub const W_GROUP: u32 = 15;
pub const W_CITY: u32 = 10;

/// What is known of the person and of the company they were entered with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    /// The person's name as entered.
    pub name: String,
    /// The company's names, the most usual first: trade name, legal name
    /// without its legal form, names in the press.
    pub org_names: Vec<String>,
    /// Names of its group: parent and subsidiaries.
    pub group_names: Vec<String>,
    /// Cities of its sites.
    pub cities: Vec<String>,
}

/// Legal forms dropped from a company name, as a search bar would not have
/// them ("ACME SAS" is found as "ACME").
const LEGAL_FORMS: &[&str] = &[
    "sa",
    "sas",
    "sasu",
    "sarl",
    "eurl",
    "sca",
    "scs",
    "snc",
    "sci",
    "inc",
    "llc",
    "ltd",
    "limited",
    "gmbh",
    "ag",
    "bv",
    "nv",
    "spa",
    "srl",
    "plc",
    "corp",
    "corporation",
    "co",
];

/// A company name as typed in a search bar: folded words, legal forms and
/// bracketed parts dropped. Empty when nothing is left.
pub fn company_words(name: &str) -> Vec<String> {
    let cut = name.split(['(', '[']).next().unwrap_or_default();
    fold_words(cut)
        .into_iter()
        .filter(|w| !LEGAL_FORMS.contains(&w.as_str()))
        .collect()
}

/// City of a French register address ("10 RUE X 13008 MARSEILLE 8EME" →
/// "marseille"): the words after the postal code, an arrondissement dropped.
pub fn city_of_address(address: &str) -> Option<String> {
    let words = fold_words(address);
    let at = words
        .iter()
        .rposition(|w| w.len() == 5 && w.chars().all(|c| c.is_ascii_digit()))?;
    let city: Vec<&str> = words[at + 1..]
        .iter()
        .map(String::as_str)
        .filter(|w| !is_arrondissement(w) && *w != "cedex")
        .filter(|w| !w.chars().all(|c| c.is_ascii_digit()))
        .collect();
    (!city.is_empty()).then(|| city.join(" "))
}

fn is_arrondissement(w: &str) -> bool {
    let digits = w.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit())
        && matches!(&w[digits.len()..], "e" | "er" | "eme")
}

/// The ladder of queries (D788), from the most precise to the widest, each
/// at most once: name and usual company name, name and each other name of
/// the company or its group, name and a city of its sites, name alone.
pub fn queries(s: &Subject) -> Vec<String> {
    let name = fold_words(&s.name).join(" ");
    if name.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut push = |q: String| {
        let q: String = q.chars().take(QUERY_MAX).collect();
        if !out.contains(&q) && out.len() < QUERIES_MAX {
            out.push(q);
        }
    };
    for org in s.org_names.iter().chain(&s.group_names) {
        let words = company_words(org);
        if !words.is_empty() {
            push(format!("{name} {}", words.join(" ")));
        }
    }
    if let Some(city) = s.cities.first() {
        push(format!("{name} {}", fold_words(city).join(" ")));
    }
    // The name alone always ends the ladder, even when the list is full.
    if !out.contains(&name) {
        if out.len() == QUERIES_MAX {
            out.pop();
        }
        out.push(name);
    }
    out
}

/// One result card: its profile address (canonical) and its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub url: String,
    pub text: String,
}

/// The cards of a results list that point to one person's profile, canonical,
/// in page order, each once, the first `max` kept; any other card (a company,
/// a post, the reader's own) is not a candidate.
pub fn candidates(cards: &[std::collections::BTreeMap<String, String>], max: usize) -> Vec<Card> {
    let mut out: Vec<Card> = Vec::new();
    for c in cards {
        let link = c.get("link").map(String::as_str).unwrap_or_default();
        if let Ok((_, url)) = ba::profile_of(link) {
            if !out.iter().any(|o| o.url == url) {
                let text = c.get("card").cloned().unwrap_or_default();
                out.push(Card { url, text });
            }
        }
        if out.len() >= max {
            break;
        }
    }
    out
}

/// Score of one opened profile against the subject, with its reasons.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Score {
    /// 0..=100.
    pub total: u32,
    /// Machine reasons (`name`, `current_company:<name>`…), shown as proof.
    pub detail: Vec<String>,
    /// The company is in the history, not in the current position: a job
    /// change, never written alone.
    pub left_company: bool,
}

/// What a score allows (D788): written, proposed to a human, or dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Write,
    Propose,
    Drop,
}

/// The verdict of a score under the pack's two thresholds. A job change is
/// proposed at best.
pub fn verdict(score: &Score, write_at: u32, propose_at: u32) -> Verdict {
    if score.total >= write_at && !score.left_company {
        Verdict::Write
    } else if score.total >= propose_at.min(write_at) {
        Verdict::Propose
    } else {
        Verdict::Drop
    }
}

/// Folded words joined by single spaces and padded, so a phrase is looked
/// for on word boundaries (" acme " never matches "acmeco").
fn padded(words: &[String]) -> String {
    format!(" {} ", words.join(" "))
}

fn has(haystack: &str, phrase: &[String]) -> bool {
    !phrase.is_empty() && haystack.contains(&padded(phrase))
}

/// Scores the readable text of an opened profile (D788). Deterministic and
/// explained; no model reads the page (D785).
///
/// shortcut: the current position is told from the text right after the
/// name (headline and top card), not from the dates of the history; upgrade
/// to the dated positions once the recipe extracts them by selectors (U4).
pub fn score(s: &Subject, profile_text: &str) -> Score {
    let text = fold_words(profile_text);
    let name = fold_words(&s.name);
    let mut out = Score::default();
    if name.is_empty() || text.is_empty() {
        return out;
    }
    let full = padded(&text);
    // Where the person's name first appears: the header follows it.
    let at = full.find(&padded(&name));
    let header = match at {
        Some(i) => {
            let end = (i + HEADER_CHARS).min(full.len());
            let end = (end..=full.len())
                .find(|&e| full.is_char_boundary(e))
                .unwrap_or(full.len());
            format!("{} ", &full[i..end])
        }
        None => {
            let mut head: String = full.chars().take(HEADER_CHARS).collect();
            head.push(' ');
            head
        }
    };
    if at.is_some() {
        out.total += W_NAME;
        out.detail.push("name".into());
    } else if name.iter().all(|w| full.contains(&format!(" {w} "))) {
        out.total += W_NAME_PARTIAL;
        out.detail.push("name_partial".into());
    } else {
        // Not the person: nothing else counts.
        return out;
    }
    let orgs: Vec<Vec<String>> = s.org_names.iter().map(|n| company_words(n)).collect();
    if let Some(i) = orgs.iter().position(|o| has(&header, o)) {
        out.total += W_CURRENT;
        out.detail
            .push(format!("current_company:{}", orgs[i].join(" ")));
    } else if let Some(i) = orgs.iter().position(|o| has(&full, o)) {
        out.total += W_PAST;
        out.left_company = true;
        out.detail
            .push(format!("past_company:{}", orgs[i].join(" ")));
    } else if let Some(g) = s
        .group_names
        .iter()
        .map(|n| company_words(n))
        .find(|g| has(&header, g))
    {
        out.total += W_GROUP;
        out.detail.push(format!("group:{}", g.join(" ")));
    }
    if let Some(c) = s
        .cities
        .iter()
        .map(|c| fold_words(c))
        .find(|c| has(&header, c))
    {
        out.total += W_CITY;
        out.detail.push(format!("city:{}", c.join(" ")));
    }
    out.total = out.total.min(100);
    out
}

/// Where a match stands (D788).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MatchState {
    /// A query of the ladder waits for a browser agent or plays.
    Searching,
    /// A profile was attached to the person.
    Matched,
    /// Candidates wait in "To validate".
    Proposed,
    /// Every query was tried, nobody matched; tried again later.
    NotFound,
    /// The person objected, or was erased, while it ran.
    Stopped,
}

impl MatchState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Searching => "searching",
            Self::Matched => "matched",
            Self::Proposed => "proposed",
            Self::NotFound => "not_found",
            Self::Stopped => "stopped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "searching" => Self::Searching,
            "matched" => Self::Matched,
            "proposed" => Self::Proposed,
            "not_found" => Self::NotFound,
            "stopped" => Self::Stopped,
            _ => return None,
        })
    }

    /// Still running: a new match of the person returns this one.
    pub fn open(self) -> bool {
        matches!(self, Self::Searching | Self::Proposed)
    }
}

/// One profile found by a query, with its score.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MatchCandidate {
    pub url: String,
    /// Index of the query that found it.
    pub query: u32,
    #[serde(default)]
    pub score: Option<u32>,
    #[serde(default)]
    pub detail: Vec<String>,
    #[serde(default)]
    pub left_company: bool,
    #[serde(default)]
    pub verdict: Option<Verdict>,
    /// The profile text, a document of the person kept 30 days.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub asset_id: Option<String>,
    /// The proposal in "To validate", for a proposed candidate.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub proposal_id: Option<String>,
}

/// A match of a person, as listed on the person's page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PersonMatch {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub person_id: String,
    /// The company the person was entered with.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub organization_id: Option<String>,
    pub state: MatchState,
    /// Index of the current query.
    pub step: u32,
    pub queries: Vec<String>,
    pub candidates: Vec<MatchCandidate>,
    /// Last obstacle (`ext-no-agent`, `ext-login-required`…).
    pub state_code: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acme() -> Subject {
        Subject {
            name: "Élodie Martin".into(),
            org_names: vec!["ACME INDUSTRIES SAS".into(), "Acme".into()],
            group_names: vec!["Globex".into()],
            cities: vec!["Toulouse".into()],
        }
    }

    #[test]
    fn the_ladder_goes_from_precise_to_wide_and_ends_on_the_name() {
        assert_eq!(
            queries(&acme()),
            [
                "elodie martin acme industries",
                "elodie martin acme",
                "elodie martin globex",
                "elodie martin toulouse",
                "elodie martin",
            ]
        );
        let mut many = acme();
        many.org_names = (0..9).map(|i| format!("Co{i}")).collect();
        let q = queries(&many);
        assert_eq!(q.len(), QUERIES_MAX);
        assert_eq!(q.last().map(String::as_str), Some("elodie martin"));
        assert!(queries(&Subject::default()).is_empty());
    }

    #[test]
    fn cities_and_company_names_read_like_a_search_bar() {
        assert_eq!(
            city_of_address("10 RUE DE CASSIS 13008 MARSEILLE 8EME").as_deref(),
            Some("marseille")
        );
        assert_eq!(
            city_of_address("1 AV X 92320 CHATILLON CEDEX").as_deref(),
            Some("chatillon")
        );
        assert_eq!(city_of_address("no postal code"), None);
        assert_eq!(
            company_words("SAFRAN ANTENNAS (10 UNITS)"),
            ["safran", "antennas"]
        );
        assert_eq!(company_words("ACME SAS"), ["acme"]);
    }

    #[test]
    fn only_profiles_of_the_results_are_candidates() {
        let card = |link: &str, text: &str| {
            std::collections::BTreeMap::from([
                ("link".to_string(), link.to_string()),
                ("card".to_string(), text.to_string()),
            ])
        };
        let cards = [
            card("https://www.linkedin.com/company/acme/", "ACME"),
            card(
                "https://www.linkedin.com/in/elodie-martin-123?miniProfileUrn=x",
                "Elodie Martin · Buyer",
            ),
            card("https://www.linkedin.com/in/elodie-martin-123/", "again"),
            card("https://www.linkedin.com/in/e-martin/", "E. Martin"),
            card("https://www.linkedin.com/in/third/", "Third"),
        ];
        let c = candidates(&cards, 2);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].url, "https://www.linkedin.com/in/elodie-martin-123");
        assert_eq!(c[0].text, "Elodie Martin · Buyer");
        assert!(c[1].url.ends_with("/in/e-martin"), "{c:?}");
    }

    #[test]
    fn the_score_tells_current_past_group_and_strangers() {
        let s = acme();
        let nav = "Accueil Réseau Emplois Messagerie ";
        let current = format!("{nav}Elodie Martin\nHead of purchasing at Acme\nToulouse, Occitanie\nExperience\nBuyer · Initech");
        let sc = score(&s, &current);
        assert_eq!(sc.total, W_NAME + W_CURRENT + W_CITY, "{sc:?}");
        assert!(!sc.left_company);
        assert_eq!(verdict(&sc, 80, 40), Verdict::Write);

        // Left ACME: the company is far down the history.
        let filler = "lorem ipsum ".repeat(80);
        let past = format!("Élodie MARTIN\nBuyer at Initech\nParis\n{filler}\nExperience\nBuyer · ACME Industries · 2019 - 2023");
        let sc = score(&s, &past);
        assert!(sc.left_company, "{sc:?}");
        assert_eq!(sc.total, W_NAME + W_PAST);
        assert_eq!(verdict(&sc, 80, 40), Verdict::Propose);
        // A job change is proposed even above the write threshold.
        assert_eq!(verdict(&sc, 40, 20), Verdict::Propose);

        let group = "Elodie Martin\nEngineer at Globex\nLyon";
        assert_eq!(score(&s, group).total, W_NAME + W_GROUP);

        let stranger = "John Roe\nHead of purchasing at Acme\nToulouse";
        let sc = score(&s, stranger);
        assert_eq!((sc.total, verdict(&sc, 80, 40)), (0, Verdict::Drop));

        // "Acme" is a word, not a prefix of "Acmeco".
        let other = "Elodie Martin\nCEO at Acmeco\nNantes";
        assert_eq!(score(&s, other).total, W_NAME);
    }
}
