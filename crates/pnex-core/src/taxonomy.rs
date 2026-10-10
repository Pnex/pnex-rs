//! Topic taxonomies (media-ingest.md D168): versioned lists of topics an
//! org classifies texts into, and the `topic_classify` flow node config.
//! Pure, wasm-safe.
//!
//! - DTOs: [`Taxonomy`], [`TaxonomyVersion`], [`Topic`] and their inputs;
//! - [`check_topics`]: bounds shared by the HTTP API and the assistant tool;
//! - [`classify_keywords`]: case- and accent-insensitive whole-word match;
//! - [`TopicClassifyConfig`]: the flow node, pinned on one version.

use serde::{Deserialize, Serialize};

use crate::err_codes;

/// Topics per version.
pub const TOPICS_MAX: usize = 100;
/// Max length of a topic id (`[a-z0-9_]`).
pub const TOPIC_ID_MAX: usize = 48;
pub const TOPIC_LABEL_MAX: usize = 120;
pub const TOPIC_DEFINITION_MAX: usize = 1000;
/// Keywords per topic.
pub const KEYWORDS_MAX: usize = 50;
pub const KEYWORD_MAX: usize = 80;
pub const NAME_MAX: usize = 200;
pub const DESCRIPTION_MAX: usize = 2000;
pub const NOTE_MAX: usize = 500;
/// Max length of the `text_field` path of the node.
pub const TEXT_FIELD_MAX: usize = 128;

/// One topic of a taxonomy version.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topic {
    /// Stable slug `[a-z0-9_]{1,48}`: what flows and series carry.
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub definition: String,
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// One append-only version of a taxonomy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaxonomyVersion {
    pub version: i32,
    pub topics: Vec<Topic>,
    #[serde(default)]
    pub note: String,
    pub created_at: String,
}

/// A taxonomy with its current version (absent before the first one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Taxonomy {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// 0 = no version yet.
    pub current_version: i32,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<TaxonomyVersion>,
}

/// Create / update body of a taxonomy; absent fields keep their value.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaxonomyInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Body of a new version: written on top of `expected_version` (optimistic
/// concurrency, 409 `taxonomy-version-conflict` otherwise).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaxonomyVersionInput {
    pub topics: Vec<Topic>,
    #[serde(default)]
    pub note: String,
    pub expected_version: i32,
}

/// Label value of the series derived from a version (D168): changing the
/// taxonomy creates new series, never rewrites history.
pub fn version_label(name: &str, version: i32) -> String {
    format!("{name}@{version}")
}

pub fn is_valid_topic_id(id: &str) -> bool {
    (1..=TOPIC_ID_MAX).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn too_long(max: usize) -> String {
    format!("{}:{max}", err_codes::FIELD_MAX_LENGTH)
}

/// Trims and checks the topics of a new version. `Err((field, token))`:
/// field = `topics` or `topics.<index>.<name>`, token = machine token.
pub fn check_topics(topics: &[Topic]) -> Result<Vec<Topic>, (String, String)> {
    let bad = |i: usize, f: &str, token: String| Err((format!("topics.{i}.{f}"), token));
    if topics.is_empty() {
        return Err(("topics".into(), err_codes::FIELD_REQUIRED.into()));
    }
    if topics.len() > TOPICS_MAX {
        return Err(("topics".into(), too_long(TOPICS_MAX)));
    }
    let mut out = Vec::with_capacity(topics.len());
    let mut seen = std::collections::BTreeSet::new();
    for (i, t) in topics.iter().enumerate() {
        let id = t.id.trim().to_string();
        if !is_valid_topic_id(&id) || !seen.insert(id.clone()) {
            return bad(i, "id", err_codes::FIELD_INVALID.into());
        }
        let label = t.label.trim().to_string();
        if label.is_empty() {
            return bad(i, "label", err_codes::FIELD_REQUIRED.into());
        }
        if label.chars().count() > TOPIC_LABEL_MAX {
            return bad(i, "label", too_long(TOPIC_LABEL_MAX));
        }
        let definition = t.definition.trim().to_string();
        if definition.chars().count() > TOPIC_DEFINITION_MAX {
            return bad(i, "definition", too_long(TOPIC_DEFINITION_MAX));
        }
        if t.keywords.len() > KEYWORDS_MAX {
            return bad(i, "keywords", too_long(KEYWORDS_MAX));
        }
        let mut keywords = Vec::with_capacity(t.keywords.len());
        for k in &t.keywords {
            let k = k.trim();
            // A keyword without a letter or digit could never match.
            if fold_words(k).is_empty() {
                return bad(i, "keywords", err_codes::FIELD_INVALID.into());
            }
            if k.chars().count() > KEYWORD_MAX {
                return bad(i, "keywords", too_long(KEYWORD_MAX));
            }
            keywords.push(k.to_string());
        }
        out.push(Topic {
            id,
            label,
            definition,
            keywords,
        });
    }
    Ok(out)
}

/// Lowercase ASCII form of a Latin letter (`é` → `e`, `œ` → `oe`); other
/// characters are returned lowercased, unchanged.
fn fold_char(c: char, out: &mut String) {
    for l in c.to_lowercase() {
        let ascii = match l {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
            'æ' => "ae",
            'ç' | 'ć' | 'č' => "c",
            'ď' | 'đ' => "d",
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e",
            'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' => "i",
            'ł' => "l",
            'ñ' | 'ń' | 'ň' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
            'œ' => "oe",
            'ř' => "r",
            'ś' | 'š' | 'ş' => "s",
            'ß' => "ss",
            'ť' | 'ţ' => "t",
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => "u",
            'ý' | 'ÿ' => "y",
            'ź' | 'ż' | 'ž' => "z",
            other => {
                out.push(other);
                continue;
            }
        };
        out.push_str(ascii);
    }
}

/// Folded words of a text: lowercase, Latin accents folded, split on every
/// non-alphanumeric character (apostrophes and hyphens included).
pub fn fold_words(text: &str) -> Vec<String> {
    let mut folded = String::with_capacity(text.len());
    for c in text.chars() {
        fold_char(c, &mut folded);
    }
    folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Ids of the topics whose keywords appear in `text`, in taxonomy order.
/// Whole words or whole phrases only (`bourse` never matches `boursier`),
/// case- and accent-insensitive.
pub fn classify_keywords(text: &str, topics: &[Topic]) -> Vec<String> {
    let words = fold_words(text);
    topics
        .iter()
        .filter(|t| {
            t.keywords.iter().any(|k| {
                let phrase = fold_words(k);
                !phrase.is_empty() && words.windows(phrase.len()).any(|w| w == phrase.as_slice())
            })
        })
        .map(|t| t.id.clone())
        .collect()
}

/// Configuration of the `topic_classify` flow node (D168). The topics are
/// not in the config: the deploy stamps those of the pinned version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopicClassifyConfig {
    /// Taxonomy of the org (checked at deploy).
    #[serde(default)]
    pub taxonomy_id: String,
    /// Pinned version, chosen in the editor; never follows the latest.
    #[serde(default)]
    pub version: i32,
    /// Dotted path of the text inside `msg.payload`.
    #[serde(default = "default_text_field")]
    pub text_field: String,
}

fn default_text_field() -> String {
    "text".into()
}

impl Default for TopicClassifyConfig {
    fn default() -> Self {
        Self {
            taxonomy_id: String::new(),
            version: 0,
            text_field: default_text_field(),
        }
    }
}

impl TopicClassifyConfig {
    /// Structural check shared by the save validation and the runtime build.
    pub fn check(&self) -> Option<(&'static str, String)> {
        if uuid::Uuid::parse_str(self.taxonomy_id.trim()).is_err() {
            return Some(("topic_classify_no_taxonomy", "select a taxonomy".into()));
        }
        if self.version < 1 {
            return Some((
                "topic_classify_version_invalid",
                "pin a version of the taxonomy".into(),
            ));
        }
        let path = self.text_field.trim();
        let segments_ok = path.split('.').all(|s| !s.is_empty());
        if path.is_empty() || path.len() > TEXT_FIELD_MAX || !segments_ok {
            return Some((
                "topic_classify_text_field_invalid",
                "the text field must be a dotted path inside msg.payload".into(),
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topic(id: &str, keywords: &[&str]) -> Topic {
        Topic {
            id: id.into(),
            label: id.into(),
            definition: String::new(),
            keywords: keywords.iter().map(|k| k.to_string()).collect(),
        }
    }

    #[test]
    fn keywords_match_whole_words_without_case_or_accents() {
        let topics = vec![
            topic("economie", &["économie", "pouvoir d'achat"]),
            topic("sante", &["hôpital", "SANTÉ PUBLIQUE"]),
            topic("bourse", &["bourse"]),
            topic("sport", &["football"]),
        ];
        let text = "Le POUVOIR D’ACHAT et l'Économie : les hopitaux et la santé publique, \
                    marché boursier en baisse.";
        // Typographic apostrophe splits like a plain one; `hopitaux` is not
        // `hôpital`, `boursier` is not `bourse`.
        assert_eq!(
            classify_keywords(text, &topics),
            vec!["economie".to_string(), "sante".to_string()]
        );
        assert_eq!(
            classify_keywords("À l’hôpital", &topics),
            vec!["sante".to_string()]
        );
        assert!(classify_keywords("", &topics).is_empty());
        // Order is the taxonomy order, not the text order.
        assert_eq!(
            classify_keywords("football puis bourse", &topics),
            vec!["bourse".to_string(), "sport".to_string()]
        );
    }

    #[test]
    fn folding_handles_ligatures_and_separators() {
        assert_eq!(
            fold_words("Cœur-de-Bœuf, ÇA!"),
            ["coeur", "de", "boeuf", "ca"]
        );
        assert!(fold_words(" -- ").is_empty());
    }

    #[test]
    fn topics_are_checked_and_trimmed() {
        let ok = check_topics(&[Topic {
            id: " eco ".into(),
            label: " Économie ".into(),
            definition: "x".into(),
            keywords: vec![" bourse ".into()],
        }])
        .expect("valid");
        assert_eq!(ok[0].id, "eco");
        assert_eq!(ok[0].label, "Économie");
        assert_eq!(ok[0].keywords, vec!["bourse"]);

        let err = |t: Vec<Topic>| check_topics(&t).unwrap_err();
        assert_eq!(err(vec![]), ("topics".into(), "required".into()));
        assert_eq!(err(vec![topic("a", &[]), topic("a", &[])]).0, "topics.1.id");
        assert_eq!(err(vec![topic("A b", &[])]).0, "topics.0.id");
        assert_eq!(err(vec![topic("x", &["--"])]).0, "topics.0.keywords");
        let many: Vec<Topic> = (0..=TOPICS_MAX)
            .map(|i| topic(&format!("t{i}"), &[]))
            .collect();
        assert_eq!(err(many).1, "max_length:100");
        let mut long = topic("x", &[]);
        long.label = "l".repeat(TOPIC_LABEL_MAX + 1);
        assert_eq!(
            err(vec![long]),
            ("topics.0.label".into(), "max_length:120".into())
        );
    }

    #[test]
    fn node_config_check() {
        let mut c = TopicClassifyConfig {
            taxonomy_id: "6f1c8a52-3b8e-4c1a-9d0e-7a2b5c4d3e21".into(),
            version: 2,
            ..Default::default()
        };
        assert_eq!(c.text_field, "text");
        assert!(c.check().is_none());
        c.text_field = "a..b".into();
        assert_eq!(c.check().unwrap().0, "topic_classify_text_field_invalid");
        c.text_field = "segment.text".into();
        c.version = 0;
        assert_eq!(c.check().unwrap().0, "topic_classify_version_invalid");
        c.taxonomy_id = String::new();
        assert_eq!(c.check().unwrap().0, "topic_classify_no_taxonomy");
    }
}
