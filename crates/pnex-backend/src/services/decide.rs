//! Typed decisions of the `text_decide` flow node (media-vision-studio.md
//! D197): the Laya model runs **here**, once per server process, instead of
//! once per org runtime (≈ 1.3 GB of weights). The bundle is installed by
//! the platform administrator (`settings.decide.model_dir`, env
//! `PNEX_DECIDE_MODEL_DIR` first); none = the node reports
//! `decide-unavailable`.
//!
//! Loaded lazily on the first call (a few seconds), a failed load is
//! retried after [`RELOAD_BACKOFF`]; one inference at a time (CPU bound),
//! callers wait at most [`QUEUE_WAIT`].

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use loco_rs::app::AppContext;
use pnex_core::decide::{
    DecideAnswer, DecideRequest, DecideResponse, TextDecideConfig, DECIDE_STATE_MAX,
};
use pnex_decide::Laya;
use serde::Deserialize;
use tokio::sync::Semaphore;

use crate::services::flow_ai::FlowAiError;

const RELOAD_BACKOFF: Duration = Duration::from_secs(60);
const QUEUE_WAIT: Duration = Duration::from_secs(30);

enum Slot {
    Empty,
    Ready(Arc<Laya>),
    Failed(Instant, String),
}

static MODEL: Mutex<Slot> = Mutex::new(Slot::Empty);
static LOAD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static GATE: Semaphore = Semaphore::const_new(1);

#[derive(Default, Deserialize)]
struct DecidePartial {
    model_dir: Option<String>,
}

/// Bundle directory, `None` = not installed on this server.
pub fn model_dir(ctx: &AppContext) -> Option<PathBuf> {
    std::env::var("PNEX_DECIDE_MODEL_DIR")
        .ok()
        .or_else(|| {
            ctx.config
                .settings
                .as_ref()
                .and_then(|s| s.get("decide"))
                .and_then(|v| serde_json::from_value::<DecidePartial>(v.clone()).ok())
                .and_then(|p| p.model_dir)
        })
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from)
}

fn cached() -> Option<Result<Arc<Laya>, String>> {
    match &*MODEL.lock().expect("decide model") {
        Slot::Empty => None,
        Slot::Ready(m) => Some(Ok(m.clone())),
        Slot::Failed(at, e) if at.elapsed() < RELOAD_BACKOFF => Some(Err(e.clone())),
        Slot::Failed(..) => None,
    }
}

async fn model(ctx: &AppContext) -> Result<(Arc<Laya>, String), FlowAiError> {
    let dir = model_dir(ctx)
        .ok_or_else(|| FlowAiError::Unavailable("no decision model installed".into()))?;
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some(found) = cached() {
        return found.map(|m| (m, name)).map_err(FlowAiError::Unavailable);
    }
    // One loader at a time; the others find the result in the slot.
    let _guard = LOAD.lock().await;
    if let Some(found) = cached() {
        return found.map(|m| (m, name)).map_err(FlowAiError::Unavailable);
    }
    let loaded = tokio::task::spawn_blocking(move || Laya::load(&dir))
        .await
        .map_err(|e| FlowAiError::Unavailable(e.to_string()))?;
    let mut slot = MODEL.lock().expect("decide model");
    match loaded {
        Ok(m) => {
            let m = Arc::new(m);
            *slot = Slot::Ready(m.clone());
            Ok((m, name))
        }
        Err(e) => {
            tracing::error!(error = %e, "decision model load failed");
            *slot = Slot::Failed(Instant::now(), e.to_string());
            Err(FlowAiError::Unavailable(e.to_string()))
        }
    }
}

/// Answers the questions of one `text_decide` message.
pub async fn decide(ctx: &AppContext, req: &DecideRequest) -> Result<DecideResponse, FlowAiError> {
    // The runtime is untrusted: same check as the editor and the deploy.
    let check = TextDecideConfig {
        questions: req.questions.clone(),
        text_field: String::new(),
    };
    if let Some((code, message)) = check.check() {
        return Err(FlowAiError::Invalid(format!("{code}: {message}")));
    }
    let (laya, name) = model(ctx).await?;
    let _permit = tokio::time::timeout(QUEUE_WAIT, GATE.acquire())
        .await
        .map_err(|_| FlowAiError::Unavailable("decision model busy".into()))?
        .map_err(|e| FlowAiError::Unavailable(e.to_string()))?;
    let state = serde_json::Value::String(pnex_core::vision_ai::clip(&req.state, DECIDE_STATE_MAX));
    let questions: Vec<(String, pnex_decide::Question)> = req
        .questions
        .iter()
        .map(|q| (q.id.clone(), q.clone()))
        .collect();
    let (answers, took_ms) = tokio::task::spawn_blocking(move || laya.decide(&state, &questions))
        .await
        .map_err(|e| FlowAiError::Invalid(e.to_string()))?
        .map_err(|e| FlowAiError::Invalid(e.to_string()))?;
    let answers = req
        .questions
        .iter()
        .zip(answers)
        .map(|(q, (id, a))| {
            let label = |i: usize| q.options.get(i).cloned().unwrap_or_default();
            let answer = DecideAnswer {
                best: label(a.best),
                probability: a.probabilities.get(a.best).copied().unwrap_or(0.0),
                probabilities: a
                    .probabilities
                    .iter()
                    .enumerate()
                    .map(|(i, p)| (label(i), *p))
                    .collect(),
                expected: a.expected,
                confidence: a.confidence,
            };
            (id, answer)
        })
        .collect();
    Ok(DecideResponse {
        answers,
        model: name,
        took_ms,
    })
}
