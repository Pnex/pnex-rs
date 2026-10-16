//! File analyses (media-import-pipeline.md D213): steps proposed on an
//! uploaded file, their visible state, and the start request.

use serde::{Deserialize, Serialize};

/// One analysis step of a media version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisStep {
    /// Text extraction and search index (doc-search.md); its state lives in
    /// `media_text_index`, not in `media_analyses`.
    Index,
    /// Summary and key points written to a page (D214), LLM `extraction`.
    Summarize,
}

impl AnalysisStep {
    pub const ALL: [Self; 2] = [Self::Index, Self::Summarize];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Index => "index",
            Self::Summarize => "summarize",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|step| step.as_str() == s)
    }

    /// Whether the step calls the org's LLM.
    pub fn needs_llm(self) -> bool {
        matches!(self, Self::Summarize)
    }
}

/// Steps proposed for a media kind (D213 §4), LLM steps included.
pub fn steps_for_kind(kind: &str) -> &'static [AnalysisStep] {
    match kind {
        "document" | "table" => &[AnalysisStep::Index, AnalysisStep::Summarize],
        _ => &[],
    }
}

pub const STATE_QUEUED: &str = "queued";
pub const STATE_RUNNING: &str = "running";
pub const STATE_DONE: &str = "done";
pub const STATE_FAILED: &str = "failed";

/// A step that can be launched now, or why not (`blocked` = error code,
/// e.g. `media-analysis-no-llm`: shown greyed with a link to fix it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepOffer {
    pub step: AnalysisStep,
    #[serde(default)]
    pub blocked: Option<String>,
    /// Estimated LLM calls of the step, once the file text is known.
    #[serde(default)]
    pub calls: Option<u32>,
}

/// Above this many estimated LLM calls a step must be confirmed
/// (`StartAnalyses::confirm_large`): cost and time become noticeable.
pub const LARGE_CALLS: u32 = 20;

/// Visible state of one step of the current version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaAnalysis {
    pub id: uuid::Uuid,
    pub step: AnalysisStep,
    /// `queued | running | done | failed`.
    pub state: String,
    /// 0–100.
    pub progress: i16,
    #[serde(default)]
    pub error_code: Option<String>,
    /// Page written by a summary (D214).
    #[serde(default)]
    pub page_id: Option<uuid::Uuid>,
    pub updated_at: String,
}

/// `POST /api/v1/media/{id}/analyses`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StartAnalyses {
    pub steps: Vec<AnalysisStep>,
    /// UI locale (`fr-FR`, `en-US`): language of the page title.
    #[serde(default)]
    pub locale: String,
    /// The user accepted a step over [`LARGE_CALLS`] calls.
    #[serde(default)]
    pub confirm_large: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_round_trip_and_kinds() {
        for s in AnalysisStep::ALL {
            assert_eq!(AnalysisStep::parse(s.as_str()), Some(s));
        }
        assert!(steps_for_kind("document").contains(&AnalysisStep::Summarize));
        assert!(steps_for_kind("splat").is_empty());
    }
}
