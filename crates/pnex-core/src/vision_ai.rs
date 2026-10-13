//! Video understanding contract (media-vision-studio.md D194–D196): the
//! `vision_caption` and `media_fuse` flow nodes and the internal routes
//! that call the organization's own model for them (D119: no platform
//! LLM). Pure, wasm-safe.

use serde::{Deserialize, Serialize};

/// Longest caption prompt, chars.
pub const CAPTION_PROMPT_MAX: usize = 500;
/// Longest side of an image sent to the model, px.
pub const CAPTION_MAX_SIDE: u32 = 1024;
/// Longest caption kept, chars.
pub const CAPTION_MAX_CHARS: usize = 1000;
/// Items of one fusion window.
pub const FUSE_ITEMS_MAX: usize = 200;
/// Text of one fusion item, chars (longer texts are cut).
pub const FUSE_ITEM_TEXT_MAX: usize = 2000;
/// Model calls per organization and per UTC day across the caption and
/// fusion nodes (§4.5 cap), unless `settings.ai.flow_daily_calls` says
/// otherwise.
pub const FLOW_AI_DAILY_CALLS: u64 = 2000;

/// Default caption prompt.
pub const DEFAULT_CAPTION_PROMPT: &str =
    "Describe factually what is visible: people, objects, actions, readable text. One or two sentences.";

/// Config of the `vision_caption` flow node (D195).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisionCaptionConfig {
    /// What to ask about the image (empty = [`DEFAULT_CAPTION_PROMPT`]).
    #[serde(default)]
    pub prompt: String,
    /// At most one caption per source in this interval, s.
    #[serde(default = "default_caption_interval")]
    pub min_interval_secs: f64,
    /// Describe the best detection of the message (crop) instead of the
    /// whole frame, when the message carries detections or tracks.
    #[serde(default)]
    pub crop_best: bool,
}

fn default_caption_interval() -> f64 {
    10.0
}

impl Default for VisionCaptionConfig {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            min_interval_secs: default_caption_interval(),
            crop_best: false,
        }
    }
}

impl VisionCaptionConfig {
    pub fn effective_prompt(&self) -> &str {
        let p = self.prompt.trim();
        if p.is_empty() {
            DEFAULT_CAPTION_PROMPT
        } else {
            p
        }
    }

    pub fn check(&self) -> Option<(&'static str, String)> {
        if self.prompt.chars().count() > CAPTION_PROMPT_MAX
            || !self.min_interval_secs.is_finite()
            || !(1.0..=86_400.0).contains(&self.min_interval_secs)
        {
            return Some((
                "vision_caption_invalid",
                format!("prompt up to {CAPTION_PROMPT_MAX} chars, interval 1..86400 s"),
            ));
        }
        None
    }
}

/// Config of the `media_fuse` flow node (D196).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaFuseConfig {
    /// Window length, s: everything received in a window is described
    /// together.
    #[serde(default = "default_fuse_window")]
    pub window_secs: f64,
    /// Language of the description (`fr`, `en`…; empty = the language of
    /// the transcripts).
    #[serde(default)]
    pub language: String,
}

fn default_fuse_window() -> f64 {
    30.0
}

impl Default for MediaFuseConfig {
    fn default() -> Self {
        Self {
            window_secs: default_fuse_window(),
            language: String::new(),
        }
    }
}

impl MediaFuseConfig {
    pub fn check(&self) -> Option<(&'static str, String)> {
        let lang_ok = self.language.len() <= 16
            && self
                .language
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !self.window_secs.is_finite() || !(5.0..=3600.0).contains(&self.window_secs) || !lang_ok
        {
            return Some((
                "media_fuse_invalid",
                "window 5..3600 s, language as a short code (fr, en-US)".into(),
            ));
        }
        None
    }
}

/// Longest `llm_text` instruction, chars.
pub const LLM_PROMPT_MAX: usize = 4000;
/// Data sent with one `llm_text` call (serialized JSON), chars.
pub const LLM_DATA_MAX: usize = 60_000;
/// Bounds of the `llm_text` answer length, chars.
pub const LLM_OUTPUT_CHARS: (u32, u32) = (100, 20_000);

/// Config of the `llm_text` flow node (D208): an instruction written by
/// the flow author and the incoming data → a text of the org's LLM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmTextConfig {
    /// What to write (trusted: the author of the flow).
    #[serde(default)]
    pub prompt: String,
    /// Dotted path of the data inside `msg.payload` (empty = the whole
    /// payload). The data is untrusted and passed as data only.
    #[serde(default)]
    pub data_field: String,
    /// Answer language (`fr`, `en`…; empty = the language of the data).
    #[serde(default)]
    pub language: String,
    /// Longest answer, chars.
    #[serde(default = "default_llm_max_chars")]
    pub max_chars: u32,
}

fn default_llm_max_chars() -> u32 {
    4000
}

impl Default for LlmTextConfig {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            data_field: String::new(),
            language: String::new(),
            max_chars: default_llm_max_chars(),
        }
    }
}

fn language_ok(l: &str) -> bool {
    l.len() <= 16 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

impl LlmTextConfig {
    pub fn check(&self) -> Option<(&'static str, String)> {
        let n = self.prompt.trim().chars().count();
        if n == 0 || n > LLM_PROMPT_MAX {
            return Some((
                "llm_text_prompt_invalid",
                format!("write an instruction of 1 to {LLM_PROMPT_MAX} characters"),
            ));
        }
        let (lo, hi) = LLM_OUTPUT_CHARS;
        let path_ok = self.data_field.len() <= 128
            && self
                .data_field
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
        if !language_ok(&self.language) || !(lo..=hi).contains(&self.max_chars) || !path_ok {
            return Some((
                "llm_text_options_invalid",
                format!("language as a short code, answer of {lo} to {hi} characters, data path made of letters, digits, _ - ."),
            ));
        }
        None
    }
}

/// `POST /internal/flow/ai/text` (D208).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmTextRequest {
    pub org_id: i64,
    pub prompt: String,
    /// Untrusted data the instruction works on.
    #[serde(default)]
    pub data: serde_json::Value,
    #[serde(default)]
    pub language: String,
    pub max_chars: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmTextResponse {
    pub text: String,
    pub model: String,
    pub took_ms: u64,
    /// The data was cut to [`LLM_DATA_MAX`] chars before the call.
    #[serde(default)]
    pub data_truncated: bool,
}

/// `POST /internal/flow/ai/caption` — the backend reads the frame itself
/// (org-prefixed key), never trusts image bytes from the runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionRequest {
    pub org_id: i64,
    pub frame_key: String,
    pub prompt: String,
    /// `[x, y, w, h]` box to describe, source pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<[f32; 4]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionResponse {
    pub caption: String,
    /// Model name of the org provider.
    pub model: String,
    pub took_ms: u64,
}

/// Kind of a fusion input, in decreasing order of trust (§4.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FuseKind {
    /// Detection, tracking or zone event: a fact.
    Detection,
    /// Transcribed speech: reliable for what was said.
    Transcript,
    /// Vision-language caption: indicative only.
    Caption,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuseItem {
    /// Stable id the sentences cite (segment id, caption id, event id).
    pub id: String,
    pub kind: FuseKind,
    pub ts_ms: i64,
    /// Camera, stream or other source name.
    #[serde(default)]
    pub source: String,
    pub text: String,
}

/// `POST /internal/flow/ai/fuse`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuseRequest {
    pub org_id: i64,
    pub t0_ms: i64,
    pub t1_ms: i64,
    #[serde(default)]
    pub language: String,
    pub items: Vec<FuseItem>,
}

/// One generated sentence and the item ids it rests on (D184 provenance).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuseSentence {
    pub text: String,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuseResponse {
    /// Kept sentences: every one cites at least one item of the window.
    pub sentences: Vec<FuseSentence>,
    /// Sentences dropped for citing no valid source.
    pub rejected: usize,
    pub model: String,
    pub took_ms: u64,
}

/// Keeps the sentences whose sources are all items of the window (and at
/// least one); returns them with the number dropped.
pub fn keep_sourced(
    sentences: Vec<FuseSentence>,
    items: &[FuseItem],
) -> (Vec<FuseSentence>, usize) {
    let total = sentences.len();
    let kept: Vec<FuseSentence> = sentences
        .into_iter()
        .filter(|s| {
            !s.text.trim().is_empty()
                && !s.sources.is_empty()
                && s.sources.iter().all(|id| items.iter().any(|i| &i.id == id))
        })
        .collect();
    let rejected = total - kept.len();
    (kept, rejected)
}

/// Cuts `text` to `max` chars.
pub fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_text_check() {
        let mut c = LlmTextConfig::default();
        assert_eq!(c.check().unwrap().0, "llm_text_prompt_invalid");
        c.prompt = "Summarize the week".into();
        assert!(c.check().is_none());
        c.data_field = "a b".into();
        assert_eq!(c.check().unwrap().0, "llm_text_options_invalid");
        c.data_field = "items".into();
        c.max_chars = 10;
        assert_eq!(c.check().unwrap().0, "llm_text_options_invalid");
    }

    #[test]
    fn configs_check() {
        assert!(VisionCaptionConfig::default().check().is_none());
        let bad = VisionCaptionConfig {
            min_interval_secs: 0.0,
            ..Default::default()
        };
        assert_eq!(bad.check().unwrap().0, "vision_caption_invalid");
        assert_eq!(
            VisionCaptionConfig::default().effective_prompt(),
            DEFAULT_CAPTION_PROMPT
        );
        assert!(MediaFuseConfig::default().check().is_none());
        let bad = MediaFuseConfig {
            language: "fr; drop".into(),
            ..Default::default()
        };
        assert_eq!(bad.check().unwrap().0, "media_fuse_invalid");
    }

    #[test]
    fn unsourced_sentences_are_dropped() {
        let item = |id: &str| FuseItem {
            id: id.into(),
            kind: FuseKind::Detection,
            ts_ms: 0,
            source: String::new(),
            text: "x".into(),
        };
        let s = |text: &str, sources: &[&str]| FuseSentence {
            text: text.into(),
            sources: sources.iter().map(|x| x.to_string()).collect(),
        };
        let (kept, rejected) = keep_sourced(
            vec![
                s("ok", &["a"]),
                s("none", &[]),
                s("ghost", &["a", "zz"]),
                s("", &["a"]),
            ],
            &[item("a"), item("b")],
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(rejected, 3);
    }
}
