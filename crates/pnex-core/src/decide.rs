//! Typed decisions contract (media-vision-studio.md §4.5bis, D197): the
//! `text_decide` flow node asks typed questions about a text and gets a
//! probability per option from the server's Laya model (non-generative,
//! Apache-2.0 weights). Pure, wasm-safe.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Question kind; the value is the `qtype` input of the Laya graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// One option among N.
    #[default]
    Choice = 0,
    /// Expected level on an ordered rubric.
    Score = 1,
    /// P(true) of a statement — biased per the model card: refused by the
    /// node in V1 (use a two-option choice).
    Noul = 2,
}

/// One typed question. `options`: choice = labels, score = rubric levels
/// in order. `descriptions` (choice only, same order) are shown to the
/// model after each label.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DecideQuestion {
    /// Key of the answer in the output (`[a-z0-9_]{1,32}`).
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub kind: QuestionKind,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub descriptions: Vec<String>,
}

pub const DECIDE_MAX_QUESTIONS: usize = 8;
pub const DECIDE_MAX_OPTIONS: usize = 20;
pub const DECIDE_INSTRUCTIONS_MAX: usize = 300;
pub const DECIDE_OPTION_MAX: usize = 120;
/// Longest state text sent to the model, chars (the model reads ≤ 1024
/// tokens anyway).
pub const DECIDE_STATE_MAX: usize = 8000;
/// Decisions per organization and per UTC day: the model is shared by the
/// whole server, one org must not monopolize it.
pub const DECIDE_DAILY_CALLS: u64 = 20_000;

/// Config of the `text_decide` flow node.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TextDecideConfig {
    #[serde(default)]
    pub questions: Vec<DecideQuestion>,
    /// Dotted path of the text in `msg.payload` (default `text`, what
    /// `media_fuse` and `media_source` emit); a non-string value is sent as
    /// JSON.
    #[serde(default)]
    pub text_field: String,
}

impl TextDecideConfig {
    pub fn effective_field(&self) -> &str {
        let f = self.text_field.trim();
        if f.is_empty() {
            "text"
        } else {
            f
        }
    }

    pub fn check(&self) -> Option<(&'static str, String)> {
        if self.questions.is_empty() {
            return Some(("text_decide_empty", "add at least one question".into()));
        }
        if self.questions.len() > DECIDE_MAX_QUESTIONS {
            return Some((
                "text_decide_invalid",
                format!("at most {DECIDE_MAX_QUESTIONS} questions"),
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for q in &self.questions {
            let id_ok = (1..=32).contains(&q.id.len())
                && q.id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            if !id_ok || !ids.insert(q.id.as_str()) {
                return Some((
                    "text_decide_invalid",
                    "question ids: unique, a-z 0-9 _ (1..32)".into(),
                ));
            }
            if q.kind == QuestionKind::Noul {
                return Some((
                    "text_decide_invalid",
                    format!("question {}: use a two-option choice instead of noul", q.id),
                ));
            }
            let n = q.instructions.trim().chars().count();
            if n == 0 || n > DECIDE_INSTRUCTIONS_MAX {
                return Some((
                    "text_decide_invalid",
                    format!(
                        "question {}: instructions 1..{DECIDE_INSTRUCTIONS_MAX} chars",
                        q.id
                    ),
                ));
            }
            let mut seen = std::collections::BTreeSet::new();
            let opts_ok = (2..=DECIDE_MAX_OPTIONS).contains(&q.options.len())
                && q.options.iter().all(|o| {
                    let t = o.trim();
                    !t.is_empty() && t.chars().count() <= DECIDE_OPTION_MAX && seen.insert(t)
                })
                && q.descriptions.len() <= q.options.len()
                && q.descriptions
                    .iter()
                    .all(|d| d.chars().count() <= DECIDE_OPTION_MAX);
            if !opts_ok {
                return Some((
                    "text_decide_invalid",
                    format!(
                        "question {}: 2..{DECIDE_MAX_OPTIONS} unique options of 1..{DECIDE_OPTION_MAX} chars",
                        q.id
                    ),
                ));
            }
        }
        None
    }
}

/// `POST /internal/flow/ai/decide`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecideRequest {
    pub org_id: i64,
    pub state: String,
    pub questions: Vec<DecideQuestion>,
}

/// Answer to one question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecideAnswer {
    /// Best option (choice) or nearest level (score), as its label.
    pub best: String,
    /// Probability of the best option.
    pub probability: f32,
    /// Label → probability, in option order.
    pub probabilities: Vec<(String, f32)>,
    /// Score only: expected level Σ i·p(i) (0 = first level).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<f32>,
    /// 1 − normalized entropy (0 = undecided, 1 = certain).
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecideResponse {
    pub answers: BTreeMap<String, DecideAnswer>,
    /// Model bundle name (directory name on the server).
    pub model: String,
    pub took_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(id: &str, options: &[&str]) -> DecideQuestion {
        DecideQuestion {
            id: id.into(),
            kind: QuestionKind::Choice,
            instructions: "Is it an incident?".into(),
            options: options.iter().map(|s| s.to_string()).collect(),
            descriptions: vec![],
        }
    }

    #[test]
    fn config_check() {
        let mut c = TextDecideConfig::default();
        assert_eq!(c.check().unwrap().0, "text_decide_empty");
        c.questions = vec![q("incident", &["yes", "no"])];
        assert!(c.check().is_none());
        assert_eq!(c.effective_field(), "text");
        c.questions.push(q("incident", &["a", "b"]));
        assert_eq!(c.check().unwrap().0, "text_decide_invalid", "duplicate id");
        c.questions = vec![q("Bad Id", &["a", "b"])];
        assert!(c.check().is_some());
        c.questions = vec![q("x", &["a"])];
        assert!(c.check().is_some(), "one option");
        c.questions = vec![q("x", &["a", "a"])];
        assert!(c.check().is_some(), "duplicate option");
        c.questions = vec![DecideQuestion {
            kind: QuestionKind::Noul,
            ..q("x", &["a", "b"])
        }];
        assert!(c.check().is_some(), "noul refused in V1");
    }
}
