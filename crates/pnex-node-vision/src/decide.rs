//! `pnex-text-decide` node (media-vision-studio.md D197) — typed questions
//! about the text of each message, answered by the server's Laya model
//! through `/internal/flow/ai/decide` (the model lives once in the backend,
//! not in every org runtime).
//!
//! Output: the message payload with `decisions` = `{question id: {best,
//! probability, probabilities, confidence, expected?}}` and
//! `decide_model`, emitted when the model answered; a message without text
//! or a failed call is dropped with the reason in the node status (D103).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use pnex_core::decide::{DecideRequest, DecideResponse, TextDecideConfig};
use pnex_core::vision::{NodeStatus, NodeStatusLevel};
use pnex_node_camera::status::{publish_status, STATUS_PERIOD};

use crate::caption::post_ai;

const NODE: &str = "pnex-text-decide";

#[derive(Debug, Deserialize)]
struct DecideNodeConfig {
    #[serde(flatten)]
    cfg: TextDecideConfig,
    #[serde(default)]
    pnex_org_id: i64,
    #[serde(default)]
    pnex_node_id: String,
}

/// Text at a dotted path of the payload: a string as is, any other
/// non-null value as JSON; `None` when missing, null or empty.
pub fn text_at(payload: &Value, path: &str) -> Option<String> {
    let v = path.split('.').try_fold(payload, |cur, key| cur.get(key))?;
    match v {
        Value::Null => None,
        Value::String(s) if s.trim().is_empty() => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

#[derive(Default)]
struct Stats {
    received: AtomicU64,
    no_text: AtomicU64,
    failed: AtomicU64,
    emitted: AtomicU64,
    last_ms: AtomicU64,
}

#[flow_node("pnex-text-decide", red_name = "pnex-text-decide")]
struct TextDecideNode {
    base: BaseFlowNodeState,
    config: DecideNodeConfig,
    http: reqwest::Client,
    url: String,
    token: String,
    stats: Stats,
    last_error: std::sync::Mutex<Option<String>>,
}

impl TextDecideNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let config = DecideNodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        if let Some((code, message)) = config.cfg.check() {
            return Err(EdgelinkError::BadFlowsJson(format!("{NODE} [{code}] : {message}")).into());
        }
        if config.pnex_org_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "{NODE} : pnex_org_id missing from the artifact (redeploy the flow)"
            ))
            .into());
        }
        let env = |k: &str| {
            std::env::var(k).map_err(|_| {
                EdgelinkError::InvalidOperation(format!(
                    "{NODE} : {k} is not set in the runtime environment"
                ))
            })
        };
        let url = format!("{}/decide", env("PNEX_FLOW_AI_URL")?.trim_end_matches('/'));
        let token = env("PNEX_FLOW_WRITE_TOKEN")?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .build()
            .map_err(|e| EdgelinkError::InvalidOperation(format!("{NODE} : http client: {e}")))?;
        Ok(Box::new(TextDecideNode {
            base: base_node,
            config,
            http,
            url,
            token,
            stats: Stats::default(),
            last_error: std::sync::Mutex::new(None),
        }))
    }

    fn status(&self) -> NodeStatus {
        let s = &self.stats;
        let base = match self.last_error.lock().expect("decide error").clone() {
            Some(e) => NodeStatus::new(NodeStatusLevel::Error, "ai-call-failed").detail(e),
            None => NodeStatus::new(NodeStatusLevel::Ok, "vision-running"),
        };
        [
            ("received", &s.received),
            ("no_text", &s.no_text),
            ("failed", &s.failed),
            ("emitted", &s.emitted),
            ("last_ms", &s.last_ms),
        ]
        .into_iter()
        .fold(base, |st, (k, v)| st.stat(k, v.load(Ordering::Relaxed)))
    }

    async fn on_msg(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        self.stats.received.fetch_add(1, Ordering::Relaxed);
        let payload = {
            let m = msg.read().await;
            m.get("payload")
                .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
                .unwrap_or(Value::Null)
        };
        let field = self.config.cfg.effective_field();
        let Some(state) = text_at(&payload, field) else {
            self.stats.no_text.fetch_add(1, Ordering::Relaxed);
            log::warn!(
                "{NODE} [{}] : message ignored — no text at payload.{field}",
                self.name()
            );
            return Ok(());
        };
        let req = DecideRequest {
            org_id: self.config.pnex_org_id,
            state,
            questions: self.config.cfg.questions.clone(),
        };
        let res: std::result::Result<DecideResponse, String> =
            post_ai(&self.http, &self.url, &self.token, &req).await;
        let res = match res {
            Ok(r) => r,
            Err(e) => {
                log::warn!("{NODE} [{}] : decision failed: {e}", self.name());
                self.stats.failed.fetch_add(1, Ordering::Relaxed);
                *self.last_error.lock().expect("decide error") = Some(e);
                publish_status(self, &self.config.pnex_node_id, &self.status());
                return Ok(());
            }
        };
        let had_error = self
            .last_error
            .lock()
            .expect("decide error")
            .take()
            .is_some();
        self.stats.last_ms.store(res.took_ms, Ordering::Relaxed);
        let mut out = if payload.is_object() {
            payload
        } else {
            serde_json::json!({ "value": payload })
        };
        out["decisions"] = serde_json::to_value(&res.answers).unwrap_or(Value::Null);
        out["decide_model"] = Value::from(res.model);
        {
            let mut m = msg.write().await;
            m.set("payload".to_string(), Variant::from(out));
        }
        self.fan_out_one(Envelope { port: 0, msg }, cancel).await?;
        self.stats.emitted.fetch_add(1, Ordering::Relaxed);
        if had_error {
            publish_status(self, &self.config.pnex_node_id, &self.status());
        }
        Ok(())
    }
}

#[async_trait]
impl FlowNodeBehavior for TextDecideNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        {
            let node = self.clone();
            let stop = stop_token.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = stop.cancelled() => break,
                        _ = tokio::time::sleep(STATUS_PERIOD) => {}
                    }
                    publish_status(node.as_ref(), &node.config.pnex_node_id, &node.status());
                }
            });
        }
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &TextDecideNode, msg: MsgHandle| async move {
                    node.on_msg(msg, cancel.child_token()).await
                },
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_at_paths() {
        let p = serde_json::json!({"text": "hello", "a": {"b": 3}, "e": "  ", "n": null});
        assert_eq!(text_at(&p, "text").as_deref(), Some("hello"));
        assert_eq!(text_at(&p, "a").as_deref(), Some("{\"b\":3}"));
        assert_eq!(text_at(&p, "a.b").as_deref(), Some("3"));
        assert_eq!(text_at(&p, "e"), None);
        assert_eq!(text_at(&p, "n"), None);
        assert_eq!(text_at(&p, "missing"), None);
    }
}
