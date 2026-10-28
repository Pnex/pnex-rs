//! Company name → SIREN (account-mapping.md D332): the `org_resolve` flow
//! node asks the public company register for a name (and a city or postcode)
//! and returns up to five scored candidates. It writes nothing: a clear
//! match leaves on its own port for a flow to propose the link (`to_validate`),
//! anything else goes to a human.

use serde::{Deserialize, Serialize};

pub const NODE_PORT_MATCHED: usize = 0;
pub const NODE_PORT_REVIEW: usize = 1;
pub const NODE_PORT_FAILED: usize = 2;
pub const NODE_PORT_COUNT: usize = 3;

pub const CANDIDATES_MAX: u32 = 5;
pub const NAME_MAX: usize = 255;

/// Points of each criterion; the total is at most 100.
const NAME_POINTS: f64 = 70.0;
const POSTCODE_POINTS: u32 = 20;
const CITY_POINTS: u32 = 15;
const ACTIVE_POINTS: u32 = 10;

fn default_threshold() -> u32 {
    80
}

fn default_gap() -> u32 {
    10
}

fn default_limit() -> u32 {
    CANDIDATES_MAX
}

/// `org_resolve` node config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgResolveConfig {
    /// Score from which the best candidate is a match (0..=100).
    #[serde(default = "default_threshold")]
    pub threshold: u32,
    /// Lead the best candidate needs over the second one.
    #[serde(default = "default_gap")]
    pub gap: u32,
    /// Candidates returned, 1..=5.
    #[serde(default = "default_limit")]
    pub limit: u32,
}

impl Default for OrgResolveConfig {
    fn default() -> Self {
        Self {
            threshold: default_threshold(),
            gap: default_gap(),
            limit: default_limit(),
        }
    }
}

impl OrgResolveConfig {
    /// Shape errors, shown while editing.
    pub fn check(&self) -> Option<(&'static str, &'static str)> {
        let bad = |m| Some(("org_resolve_config_invalid", m));
        if self.threshold > 100 || self.gap > 100 {
            return bad("threshold and gap are between 0 and 100");
        }
        if !(1..=CANDIDATES_MAX).contains(&self.limit) {
            return bad("limit must be between 1 and 5");
        }
        None
    }
}

/// What is looked up: a company name, where it is if known.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgQuery {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub city: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub postcode: String,
}

/// A register entry as the scoring sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegisterEntry {
    pub siren: String,
    /// Names the company goes by (legal name, trade name, acronym).
    pub names: Vec<String>,
    /// (postcode, city) of the head office and of the matching sites.
    pub places: Vec<(String, String)>,
    pub active: bool,
}

/// One scored candidate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgCandidate {
    pub siren: String,
    pub name: String,
    pub score: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub city: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub postcode: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub naf: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub headcount_band: String,
    pub active: bool,
}

/// Legal forms and filler words that say nothing about which company it is.
const NOISE: &[&str] = &[
    "SA",
    "SAS",
    "SASU",
    "SARL",
    "EURL",
    "SNC",
    "SCI",
    "SCOP",
    "SE",
    "GIE",
    "GROUPE",
    "GROUP",
    "STE",
    "SOCIETE",
    "ET",
    "DE",
    "DES",
    "DU",
    "LA",
    "LE",
    "LES",
    "AND",
    "THE",
    "CIE",
    "COMPAGNIE",
];

/// Upper case, no accents, words only, legal forms dropped.
pub fn tokens(name: &str) -> Vec<String> {
    let folded: String = name
        .chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'À' | 'Â' | 'Ä' => 'A',
            'ç' | 'Ç' => 'C',
            'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'E',
            'î' | 'ï' | 'Î' | 'Ï' => 'I',
            'ô' | 'ö' | 'Ô' | 'Ö' => 'O',
            'ù' | 'û' | 'ü' | 'Ù' | 'Û' | 'Ü' => 'U',
            'ÿ' | 'Ÿ' => 'Y',
            c if c.is_alphanumeric() => c.to_ascii_uppercase(),
            _ => ' ',
        })
        .collect();
    let mut out: Vec<String> = folded
        .split_whitespace()
        // Single letters too: "S.A.S." splits into S, A, S.
        .filter(|w| w.len() > 1 && !NOISE.contains(w))
        .map(str::to_string)
        .collect();
    out.dedup();
    out
}

/// Dice coefficient of two token lists (0..=1).
fn dice(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let common = a.iter().filter(|t| b.contains(t)).count();
    2.0 * common as f64 / (a.len() + b.len()) as f64
}

/// Score of an entry for a query, 0..=100: name likeness (70), same
/// postcode (20) or else same city (15), still active (10). Without a
/// place in the query, at most 80: a name alone is a match only when the
/// threshold allows it.
pub fn score(q: &OrgQuery, e: &RegisterEntry) -> u32 {
    let wanted = tokens(&q.name);
    let name = e
        .names
        .iter()
        .map(|n| dice(&wanted, &tokens(n)))
        .fold(0.0, f64::max);
    let mut s = (name * NAME_POINTS).round() as u32;
    let postcode = q.postcode.trim();
    let city = tokens(&q.city);
    if !postcode.is_empty() && e.places.iter().any(|(p, _)| p == postcode) {
        s += POSTCODE_POINTS;
    } else if !city.is_empty() && e.places.iter().any(|(_, c)| tokens(c) == city) {
        s += CITY_POINTS;
    }
    if e.active {
        s += ACTIVE_POINTS;
    }
    s.min(100)
}

/// Port of a scored, sorted list: a match needs the best above the
/// threshold and ahead of the next one by the gap.
pub fn port_of(c: &OrgResolveConfig, sorted: &[OrgCandidate]) -> usize {
    match sorted {
        [best, rest @ ..]
            if best.score >= c.threshold
                && rest.first().is_none_or(|n| best.score >= n.score + c.gap) =>
        {
            NODE_PORT_MATCHED
        }
        _ => NODE_PORT_REVIEW,
    }
}

/// Runtime → backend: one lookup of a deployed node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrgResolveNodeRequest {
    pub node: crate::ontology::flow::FlowNodeRef,
    pub query: OrgQuery,
}

/// Backend → runtime.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OrgResolveNodeReply {
    pub port: usize,
    #[serde(default)]
    pub candidates: Vec<OrgCandidate>,
    /// SIREN of the match (port 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub siren: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub args: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(siren: &str, name: &str, place: (&str, &str), active: bool) -> RegisterEntry {
        RegisterEntry {
            siren: siren.into(),
            names: vec![name.into()],
            places: vec![(place.0.into(), place.1.into())],
            active,
        }
    }

    fn q(name: &str, city: &str, postcode: &str) -> OrgQuery {
        OrgQuery {
            name: name.into(),
            city: city.into(),
            postcode: postcode.into(),
        }
    }

    #[test]
    fn legal_forms_and_accents_do_not_count() {
        assert_eq!(
            tokens("Mécachrome France S.A.S."),
            vec!["MECACHROME", "FRANCE"]
        );
        assert_eq!(tokens("Société Générale de l'Est"), vec!["GENERALE", "EST"]);
    }

    #[test]
    fn place_and_state_separate_namesakes() {
        let toulouse = entry(
            "329936793",
            "MECACHROME TOULOUSE",
            ("31200", "TOULOUSE"),
            true,
        );
        let france = entry("429853351", "MECACHROME FRANCE", ("37530", "AMBOISE"), true);
        let gone = entry("542093166", "SOC MECACHROME", ("37530", "AMBOISE"), false);
        let query = q("Mecachrome France", "Amboise", "");
        assert_eq!(score(&query, &france), 95);
        assert!(score(&query, &toulouse) < 60);
        assert!(score(&query, &gone) < score(&query, &france));
        // A postcode beats a city name.
        assert_eq!(score(&q("Mecachrome France", "", "37530"), &france), 100);
        // A name alone: at most 80.
        assert_eq!(score(&q("Mecachrome France", "", ""), &france), 80);
    }

    #[test]
    fn a_match_needs_the_threshold_and_a_lead() {
        let c = OrgResolveConfig::default();
        let cand = |s| OrgCandidate {
            score: s,
            ..Default::default()
        };
        assert_eq!(port_of(&c, &[cand(95), cand(60)]), NODE_PORT_MATCHED);
        assert_eq!(port_of(&c, &[cand(85)]), NODE_PORT_MATCHED);
        assert_eq!(port_of(&c, &[cand(85), cand(80)]), NODE_PORT_REVIEW);
        assert_eq!(port_of(&c, &[cand(70)]), NODE_PORT_REVIEW);
        assert_eq!(port_of(&c, &[]), NODE_PORT_REVIEW);
        assert!(OrgResolveConfig {
            limit: 6,
            ..c.clone()
        }
        .check()
        .is_some());
        assert!(c.check().is_none());
    }
}
