//! `pnex-llm-text` node (media-vision-studio.md D208) — for each message,
//! sends the author's instruction and the message data (whole payload or
//! `data_field`) to the organization's LLM through the backend
//! (`/internal/flow/ai/text`) and emits the answer.
//!
//! The data travels as data, never as instructions (the backend prompt
//! says so); calls count in the org's daily AI quota. Output (port 0):
//! `payload = {text, model, took_ms, data_truncated, ai_generated: true}`,
//! `topic` kept. A failed call is logged and shown on the node status
//! (D103), nothing is emitted.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use pnex_core::vision::{NodeStatus, NodeStatusLevel};
use pnex_core::vision_ai::{LlmTextConfig, LlmTextRequest, LlmTextResponse};
use pnex_node_camera::status::publish_status;

use crate::caption::post_ai;

const NODE: &str = "pnex-llm-text";

/// Explicit fields, no `flatten`: with arbitrary_precision (unified in the
/// workspace) a flattened integer is buffered as a magic map and fails.
#[derive(Debug, Deserialize)]
struct NodeConfig {
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    data_field: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    max_chars: u32,
    #[serde(default)]
    pnex_org_id: i64,
    #[serde(default)]
    pnex_node_id: String,
}

/// Data of a message: the whole payload, or the value at `field`.
pub fn data_of(payload: &Value, field: &str) -> Option<Value> {
    let field = field.trim();
    if field.is_empty() {
        return Some(payload.clone());
    }
    pnex_core::resolve_msg_path(payload, field).cloned()
}

#[derive(Default)]
struct Stats {
    received: AtomicU64,
    answered: AtomicU64,
    failed: AtomicU64,
}

#[flow_node("pnex-llm-text", red_name = "pnex-llm-text")]
struct LlmTextNode {
    base: BaseFlowNodeState,
    cfg: LlmTextConfig,
    org_id: i64,
    node_id: String,
    http: reqwest::Client,
    url: String,
    token: String,
    stats: Stats,
    last_error: std::sync::Mutex<Option<String>>,
}

impl LlmTextNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let raw = NodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        let cfg = LlmTextConfig {
            prompt: raw.prompt,
            data_field: raw.data_field,
            language: raw.language,
            max_chars: raw.max_chars,
        };
        if let Some((code, message)) = cfg.check() {
            return Err(EdgelinkError::BadFlowsJson(format!("{NODE} [{code}] : {message}")).into());
        }
        if raw.pnex_org_id <= 0 {
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
        let url = format!("{}/text", env("PNEX_FLOW_AI_URL")?.trim_end_matches('/'));
        let token = env("PNEX_FLOW_WRITE_TOKEN")?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|e| EdgelinkError::InvalidOperation(format!("{NODE} : http client: {e}")))?;
        Ok(Box::new(LlmTextNode {
            base: base_node,
            cfg,
            org_id: raw.pnex_org_id,
            node_id: raw.pnex_node_id,
            http,
            url,
            token,
            stats: Stats::default(),
            last_error: std::sync::Mutex::new(None),
        }))
    }

    fn status(&self) -> NodeStatus {
        let s = &self.stats;
        let base = match self.last_error.lock().expect("llm error").clone() {
            Some(e) => NodeStatus::new(NodeStatusLevel::Error, "ai-call-failed").detail(e),
            None => NodeStatus::new(NodeStatusLevel::Ok, "vision-running"),
        };
        [
            ("received", &s.received),
            ("answered", &s.answered),
            ("failed", &s.failed),
        ]
        .into_iter()
        .fold(base, |st, (k, v)| st.stat(k, v.load(Ordering::Relaxed)))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        self.stats.received.fetch_add(1, Ordering::Relaxed);
        let payload = {
            let m = msg.read().await;
            m.get("payload")
                .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
                .unwrap_or(Value::Null)
        };
        let Some(data) = data_of(&payload, &self.cfg.data_field) else {
            log::warn!(
                "{NODE} [{}] : message ignored — no data at \"{}\"",
                self.name(),
                self.cfg.data_field
            );
            return Ok(());
        };
        let req = LlmTextRequest {
            org_id: self.org_id,
            prompt: self.cfg.prompt.clone(),
            data,
            language: self.cfg.language.clone(),
            max_chars: self.cfg.max_chars,
        };
        let res: std::result::Result<LlmTextResponse, String> =
            post_ai(&self.http, &self.url, &self.token, &req).await;
        let res = match res {
            Ok(r) => {
                *self.last_error.lock().expect("llm error") = None;
                self.stats.answered.fetch_add(1, Ordering::Relaxed);
                r
            }
            Err(e) => {
                log::warn!("{NODE} [{}] : call failed: {e}", self.name());
                self.stats.failed.fetch_add(1, Ordering::Relaxed);
                *self.last_error.lock().expect("llm error") = Some(e);
                publish_status(self, &self.node_id, &self.status());
                return Ok(());
            }
        };
        publish_status(self, &self.node_id, &self.status());
        let out = json!({
            "text": res.text,
            "model": res.model,
            "took_ms": res.took_ms,
            "data_truncated": res.data_truncated,
            "ai_generated": true,
        });
        msg.write()
            .await
            .set("payload".to_string(), Variant::from(out));
        self.fan_out_one(Envelope { port: 0, msg }, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for LlmTextNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &LlmTextNode, msg: MsgHandle| async move {
                    let res = node.execute(msg, cancel.child_token()).await;
                    if let Err(e) = &res {
                        log::warn!("{NODE} [{}] : message rejected : {e}", node.name());
                    }
                    res
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
    fn data_is_the_payload_or_a_field() {
        let p = json!({"week": {"events": [1, 2]}, "other": true});
        assert_eq!(data_of(&p, ""), Some(p.clone()));
        assert_eq!(data_of(&p, "week.events"), Some(json!([1, 2])));
        assert_eq!(data_of(&p, "missing"), None);
    }
}
