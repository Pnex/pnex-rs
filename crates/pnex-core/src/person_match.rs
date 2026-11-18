//! Matching a person entered by a sales rep with a network profile
//! (agent-unification.md D788). Pure and wasm-safe: the subject and the
//! ladder of queries typed in a search bar, and the view of a person's
//! searches. Searching, scoring and writing are the pack pipeline
//! `prospecting_b2b/person_match_linkedin` (pipeline-engine.md D996).

use crate::fold::fold_words;
use serde::{Deserialize, Serialize};

/// Queries of the ladder kept at most, from the most precise to the widest.
pub const QUERIES_MAX: usize = 5;

/// Searches of one org per day, whatever its setting asks (D788); each is a
/// rule run of a browser agent, counted in its daily runs besides.
pub const SEARCHES_PER_DAY_MAX: i64 = 30;

/// Characters of a query, like a search bar keeps them.
const QUERY_MAX: usize = 120;

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

/// What a candidate's score allowed (D788): attached, proposed to a human,
/// or dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Write,
    Propose,
    Drop,
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
}
