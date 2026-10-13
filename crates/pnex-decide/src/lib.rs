//! PNeX typed decisions (media-vision-studio.md §4.5bis, D197) — Laya, a
//! non-generative decision model (Convai Innovations, Apache-2.0 weights):
//! one forward pass answers typed questions about a state with a
//! probability per option. Pure Rust: tract for the ONNX graph, the Hugging
//! Face tokenizer for the ids; no Python or Node at runtime.
//!
//! Bundle layout (the output of `receptron/laya` `export_onnx.py`):
//! `laya.onnx` (+ `laya.onnx.data`), `laya_config.json`, `tokenizer/
//! tokenizer.json`. Graph inputs: `input_ids [B,L] i64`, `attention_mask
//! [B,L] i64`, `marker_pos [B,K] i64`, `marker_mask [B,K] bool`, `qtype
//! [B] i64`; output 0 = uncalibrated logits `[B,K]` (masked = −1e4).

pub mod sequence;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tract_onnx::prelude::*;

use sequence::{build_sequence, confidence, softmax, temp_bucket, SpecialIds};
pub use sequence::{Question, QuestionKind};

#[derive(Debug, thiserror::Error)]
pub enum DecideError {
    #[error("bundle file unreadable: {0}")]
    Bundle(String),
    #[error("tokenizer: {0}")]
    Tokenizer(String),
    #[error("model load failed: {0}")]
    Load(String),
    #[error("inference failed: {0}")]
    Run(String),
    #[error("question {0}: options do not fit the head (at most 20 short options)")]
    TooManyOptions(String),
    #[error("question {0}: needs 2 to 20 options")]
    Options(String),
}

/// `laya_config.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct LayaConfig {
    pub max_len: usize,
    pub head_max_len: usize,
    /// Per question kind (choice, score, noul).
    pub temperature: Vec<f32>,
    #[serde(default)]
    pub temperature_by_options: HashMap<String, f32>,
}

/// Answer to one question: calibrated probabilities in option order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// Best option (choice) or nearest level (score).
    pub best: usize,
    pub probabilities: Vec<f32>,
    /// Score only: expected level Σ i·p(i).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<f32>,
    /// 1 − normalized entropy.
    pub confidence: f32,
}

type Plan = Arc<TypedRunnableModel>;

pub struct Laya {
    plan: Plan,
    tokenizer: tokenizers::Tokenizer,
    config: LayaConfig,
    ids: SpecialIds,
    /// Literal mask token, scrubbed from user text.
    mask_token: String,
}

/// Most options per question (the model card: precision drops beyond).
pub const MAX_OPTIONS: usize = 20;

fn round4(x: f32) -> f32 {
    (x * 1e4).round() / 1e4
}

impl Laya {
    /// Loads a bundle directory (graph optimized once with symbolic batch,
    /// sequence and option dimensions).
    pub fn load(dir: &Path) -> Result<Self, DecideError> {
        let read = |f: &str| {
            std::fs::read_to_string(dir.join(f))
                .map_err(|e| DecideError::Bundle(format!("{f}: {e}")))
        };
        let config: LayaConfig = serde_json::from_str(&read("laya_config.json")?)
            .map_err(|e| DecideError::Bundle(format!("laya_config.json: {e}")))?;
        let tokenizer = tokenizers::Tokenizer::from_file(dir.join("tokenizer/tokenizer.json"))
            .map_err(|e| DecideError::Tokenizer(e.to_string()))?;
        // Special tokens come from tokenizer_config.json like the Python
        // reference (`tok.cls_token_id`…): `[CLS]` for the English
        // checkpoint, `<bos>`/`<eos>`/`<mask>`/`<pad>` for the multilingual one.
        let tcfg: serde_json::Value =
            serde_json::from_str(&read("tokenizer/tokenizer_config.json")?)
                .map_err(|e| DecideError::Bundle(format!("tokenizer_config.json: {e}")))?;
        let id = |field: &str| {
            let t = tcfg[field].as_str().unwrap_or_default();
            tokenizer
                .token_to_id(t)
                .map(|id| (id, t.to_string()))
                .ok_or_else(|| DecideError::Tokenizer(format!("special token {field} missing")))
        };
        let (mask, mask_token) = id("mask_token")?;
        let ids = SpecialIds {
            cls: id("cls_token")?.0,
            sep: id("sep_token")?.0,
            mask,
            pad: id("pad_token")?.0,
        };
        let plan =
            load_plan(&dir.join("laya.onnx")).map_err(|e| DecideError::Load(format!("{e:#}")))?;
        Ok(Self {
            plan,
            tokenizer,
            config,
            ids,
            mask_token,
        })
    }

    pub fn config(&self) -> &LayaConfig {
        &self.config
    }

    fn encode(&self, text: &str) -> Vec<u32> {
        self.tokenizer
            .encode(text, false)
            .map(|e| e.get_ids().to_vec())
            .unwrap_or_default()
    }

    /// Token ids and marker positions of one question (exposed for the
    /// parity test against the reference implementation).
    pub fn sequence(&self, state: &serde_json::Value, q: &Question) -> (Vec<u32>, Vec<usize>) {
        build_sequence(
            &|t| self.encode(t),
            self.ids,
            &self.mask_token,
            state,
            q,
            self.config.max_len,
            self.config.head_max_len,
        )
    }

    /// Raw logits of pre-built rows (one row per question).
    pub fn logits(
        &self,
        rows: &[(Vec<u32>, Vec<usize>, QuestionKind)],
    ) -> Result<Vec<Vec<f32>>, DecideError> {
        let n = rows.len();
        let l = rows.iter().map(|r| r.0.len()).max().unwrap_or(1);
        let k = rows.iter().map(|r| r.1.len()).max().unwrap_or(1);
        let mut input_ids =
            tract_ndarray::Array2::<i64>::from_elem((n, l), i64::from(self.ids.pad));
        let mut attention = tract_ndarray::Array2::<i64>::zeros((n, l));
        let mut marker_pos = tract_ndarray::Array2::<i64>::zeros((n, k));
        let mut marker_mask = tract_ndarray::Array2::<bool>::from_elem((n, k), false);
        let mut qtype = tract_ndarray::Array1::<i64>::zeros(n);
        for (i, (seq, markers, kind)) in rows.iter().enumerate() {
            for (j, v) in seq.iter().enumerate() {
                input_ids[[i, j]] = i64::from(*v);
                attention[[i, j]] = 1;
            }
            for (j, m) in markers.iter().enumerate() {
                marker_pos[[i, j]] = *m as i64;
                marker_mask[[i, j]] = true;
            }
            qtype[i] = *kind as i64;
        }
        let out = self
            .plan
            .run(tvec!(
                input_ids.into_tensor().into(),
                attention.into_tensor().into(),
                marker_pos.into_tensor().into(),
                marker_mask.into_tensor().into(),
                qtype.into_tensor().into()
            ))
            .map_err(|e| DecideError::Run(format!("{e:#}")))?;
        let logits = out[0]
            .to_plain_array_view::<f32>()
            .map_err(|e| DecideError::Run(format!("{e:#}")))?;
        Ok(rows
            .iter()
            .enumerate()
            .map(|(i, r)| (0..r.1.len()).map(|j| logits[[i, j]]).collect())
            .collect())
    }

    /// Answers every question about `state` in one forward pass, with the
    /// checkpoint's per-cardinality temperature (rounded like the
    /// reference: 4 decimals).
    pub fn decide(
        &self,
        state: &serde_json::Value,
        questions: &[(String, Question)],
    ) -> Result<(Vec<(String, Answer)>, u64), DecideError> {
        let started = Instant::now();
        let mut rows = Vec::with_capacity(questions.len());
        for (qid, q) in questions {
            let expected = sequence::render_options(q).len();
            if q.kind != QuestionKind::Noul && !(2..=MAX_OPTIONS).contains(&expected) {
                return Err(DecideError::Options(qid.clone()));
            }
            let (seq, markers) = self.sequence(state, q);
            if markers.len() != expected {
                return Err(DecideError::TooManyOptions(qid.clone()));
            }
            rows.push((seq, markers, q.kind));
        }
        let logits = self.logits(&rows)?;
        let answers = questions
            .iter()
            .zip(logits)
            .map(|((qid, q), z)| {
                let temp = self
                    .config
                    .temperature_by_options
                    .get(&temp_bucket(q.kind, z.len()))
                    .copied()
                    .or_else(|| self.config.temperature.get(q.kind as usize).copied())
                    .unwrap_or(1.0);
                let p = softmax(&z.iter().map(|v| v / temp).collect::<Vec<_>>());
                let best = p
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(b.1))
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                let expected = (q.kind == QuestionKind::Score)
                    .then(|| round4(p.iter().enumerate().map(|(i, v)| i as f32 * v).sum()));
                let answer = Answer {
                    best: expected.map(|e| e.round() as usize).unwrap_or(best),
                    confidence: round4(confidence(&p)),
                    probabilities: p.into_iter().map(round4).collect(),
                    expected,
                };
                (qid.clone(), answer)
            })
            .collect();
        Ok((answers, started.elapsed().as_millis() as u64))
    }
}

/// Graph with its exported symbolic dims (`batch`, `seq`, `options`), optimized once.
fn load_plan(path: &Path) -> TractResult<Plan> {
    let mut model = tract_onnx::onnx().model_for_path(path)?;
    let b = model.symbols.sym("batch").to_dim();
    let l = model.symbols.sym("seq").to_dim();
    let k = model.symbols.sym("options").to_dim();
    model.set_input_fact(0, i64::fact([b.clone(), l.clone()]).into())?;
    model.set_input_fact(1, i64::fact([b.clone(), l]).into())?;
    model.set_input_fact(2, i64::fact([b.clone(), k.clone()]).into())?;
    model.set_input_fact(3, bool::fact([b.clone(), k]).into())?;
    model.set_input_fact(4, i64::fact([b]).into())?;
    model.into_optimized()?.into_runnable()
}
