//! `pnex-event-log` node (camera-video.md D84) — writes `msg.payload` (any
//! JSON) as one event in the org's OpenObserve logs stream, through the
//! backend internal route `/internal/flow/event` (the backend owns the O2
//! org provisioning). Lenient: a failed write is logged, the message always
//! passes through on port 0 (chainable into notify, debug, …).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use pnex_core::events::{EventInput, EventLevel};

const NODE: &str = "pnex-event-log";
const FLOW_TOKEN_HEADER: &str = "x-pnex-flow-token";

/// Anti-stripping anchor (see the runtime binary).
pub fn registered() {}

#[derive(Debug, Deserialize)]
struct EventLogNodeConfig {
    #[serde(default)]
    stream: String,
    #[serde(default)]
    level: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    pnex_node_id: String,
    #[serde(default)]
    pnex_flow_id: i64,
    #[serde(default)]
    pnex_org_id: i64,
}

#[flow_node("pnex-event-log", red_name = "pnex-event-log")]
struct EventLogNode {
    base: BaseFlowNodeState,
    config: EventLogNodeConfig,
    stream: String,
    level: EventLevel,
    http: reqwest::Client,
    url: String,
    token: String,
}

impl EventLogNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = EventLogNodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        let level = if cfg.level.is_empty() {
            EventLevel::default()
        } else {
            EventLevel::from_wire(&cfg.level).ok_or_else(|| {
                EdgelinkError::BadFlowsJson(format!("{NODE} : unknown level `{}`", cfg.level))
            })?
        };
        let check = pnex_core::events::EventLogConfig {
            stream: cfg.stream.clone(),
            level,
            message: cfg.message.clone(),
        };
        if let Some((code, message)) = check.check() {
            return Err(EdgelinkError::BadFlowsJson(format!("{NODE} [{code}] : {message}")).into());
        }
        let stream = pnex_core::events::event_stream_name(&cfg.stream).expect("checked above");
        if cfg.pnex_org_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "{NODE} : pnex_org_id missing from the artifact (redeploy the flow)"
            ))
            .into());
        }
        let url = std::env::var("PNEX_FLOW_EVENT_URL").map_err(|_| {
            EdgelinkError::InvalidOperation(format!(
                "{NODE} : PNEX_FLOW_EVENT_URL is not set in the runtime environment \
                 (runtime_token not configured on the server?)"
            ))
        })?;
        let token = std::env::var("PNEX_FLOW_WRITE_TOKEN").map_err(|_| {
            EdgelinkError::InvalidOperation(format!(
                "{NODE} : PNEX_FLOW_WRITE_TOKEN is not set in the runtime environment"
            ))
        })?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| EdgelinkError::InvalidOperation(format!("{NODE} : http client: {e}")))?;
        Ok(Box::new(EventLogNode {
            base: base_node,
            config: cfg,
            stream,
            level,
            http,
            url,
            token,
        }))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let (payload, topic) = {
            let m = msg.read().await;
            let payload = m
                .get("payload")
                .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
                .unwrap_or(serde_json::Value::Null);
            let topic = m.get("topic").and_then(|t| t.as_str().map(str::to_string));
            (payload, topic)
        };
        let ev = EventInput {
            org_id: self.config.pnex_org_id,
            stream: self.stream.clone(),
            level: self.level,
            message: self.config.message.clone(),
            topic,
            flow_id: (self.config.pnex_flow_id > 0).then_some(self.config.pnex_flow_id),
            node_id: self.config.pnex_node_id.clone(),
            payload,
        };
        match self
            .http
            .post(&self.url)
            .header(FLOW_TOKEN_HEADER, &self.token)
            .header(
                pnex_core::FLOW_WORKER_HEADER,
                pnex_core::flow_worker_fence().unwrap_or_default(),
            )
            .json(&ev)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                log::warn!(
                    "{NODE} [{}] : event refused ({status}): {body}",
                    self.name()
                );
            }
            Err(e) => log::warn!("{NODE} [{}] : event write failed: {e}", self.name()),
        }
        self.fan_out_one(Envelope { port: 0, msg }, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for EventLogNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &EventLogNode, msg: MsgHandle| async move {
                    node.execute(msg, cancel.child_token()).await
                },
            )
            .await;
        }
    }
}
