//! `pnex-agent-run-finished` — event source of the ended agent runs (agent-environments.md
//! D713).
//!
//! When a run ends, the backend publishes its summary once on `pnex:agentrun:v1:{org}`.
//! This node subscribes to the org channel and emits one message per ended run matching
//! its filter (`payload` = `msg.run` = summary, `msg.chain_id`, `topic` = job state). No
//! replay: a run that ends while the flow is stopped is not emitted.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use pnex_core::agent_chains::{finished_channel, AgentRunFinishedConfig, AgentRunFinishedEvent};

const NODE: &str = "pnex-agent-run-finished";
const BACKOFF_MAX_SECS: u64 = 30;

#[derive(Debug, Deserialize)]
struct NodeConfig {
    // No `serde(flatten)`: it breaks numbers under `arbitrary_precision`.
    #[serde(default)]
    agent_id: String,
    #[serde(default)]
    chained_only: bool,
    /// Stamped by the deploy projection.
    #[serde(default)]
    pnex_org_id: i64,
}

#[flow_node("pnex-agent-run-finished", red_name = "pnex-agent-run-finished")]
struct AgentRunFinishedNode {
    base: BaseFlowNodeState,
    config: NodeConfig,
    trigger: AgentRunFinishedConfig,
    client: redis::Client,
}

impl NodeConfig {
    fn trigger(&self) -> AgentRunFinishedConfig {
        AgentRunFinishedConfig {
            agent_id: self.agent_id.clone(),
            chained_only: self.chained_only,
        }
    }
}

/// Message body of one ended run.
fn body_of(ev: &AgentRunFinishedEvent) -> BTreeMap<String, Variant> {
    let run = serde_json::to_value(ev).unwrap_or_default();
    let mut body = BTreeMap::new();
    body.insert("payload".to_string(), Variant::from(run.clone()));
    body.insert("run".to_string(), Variant::from(run));
    body.insert("topic".to_string(), Variant::from(ev.state.clone()));
    if let Some(chain) = &ev.chain_id {
        body.insert("chain_id".to_string(), Variant::from(chain.clone()));
    }
    body
}

impl AgentRunFinishedNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = NodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        if let Some((code, message)) = cfg.trigger().check() {
            return Err(EdgelinkError::BadFlowsJson(format!("{NODE} [{code}] : {message}")).into());
        }
        if cfg.pnex_org_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "{NODE} : pnex_org_id missing from the artifact (redeploy the flow)"
            ))
            .into());
        }
        let url = std::env::var("VALKEY_URL")
            .ok()
            .filter(|u| !u.trim().is_empty())
            .ok_or_else(|| {
                EdgelinkError::InvalidOperation(format!(
                    "{NODE} [agent_run_bus_unavailable] : VALKEY_URL is not set in the runtime environment"
                ))
            })?;
        let client = redis::Client::open(url.as_str()).map_err(|e| {
            EdgelinkError::InvalidOperation(format!("{NODE} : invalid VALKEY_URL: {e}"))
        })?;
        Ok(Box::new(AgentRunFinishedNode {
            base: base_node,
            trigger: cfg.trigger(),
            config: cfg,
            client,
        }))
    }

    /// One subscription session: returns when the connection drops (the
    /// caller reconnects) or the node stops.
    async fn session(&self, stop: &CancellationToken) -> std::result::Result<(), String> {
        let channel = finished_channel(self.config.pnex_org_id);
        let mut pubsub = self
            .client
            .get_async_pubsub()
            .await
            .map_err(|e| format!("valkey connect failed: {e}"))?;
        pubsub
            .subscribe(&channel)
            .await
            .map_err(|e| format!("valkey subscribe failed: {e}"))?;
        log::info!("{NODE} [{}] : subscribed to {channel}", self.name());
        let mut stream = pubsub.on_message();
        loop {
            let msg = tokio::select! {
                _ = stop.cancelled() => return Ok(()),
                m = stream.next() => match m {
                    Some(m) => m,
                    None => return Err("valkey subscription closed".into()),
                },
            };
            let Ok(raw) = msg.get_payload::<String>() else {
                continue;
            };
            let Ok(ev) = serde_json::from_str::<AgentRunFinishedEvent>(&raw) else {
                continue;
            };
            if !ev.matches(&self.trigger) {
                continue;
            }
            let envelope = Envelope {
                port: 0,
                msg: MsgHandle::with_properties(body_of(&ev)),
            };
            if let Err(e) = self.fan_out_one(envelope, stop.child_token()).await {
                log::warn!("{NODE} [{}] : fan-out failed: {e}", self.name());
            }
        }
    }
}

#[async_trait]
impl FlowNodeBehavior for AgentRunFinishedNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        let mut backoff = 1u64;
        while !stop_token.is_cancelled() {
            match self.session(&stop_token).await {
                Ok(()) => break,
                Err(e) => {
                    log::warn!("{NODE} [{}] : {e} — retry in {backoff} s", self.name());
                    tokio::select! {
                        _ = stop_token.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
                    }
                    backoff = (backoff * 2).min(BACKOFF_MAX_SECS);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_carries_the_run_and_its_chain() {
        let ev = AgentRunFinishedEvent {
            run_id: "r1".into(),
            agent_id: Some("a1".into()),
            chain_id: Some("c1".into()),
            subject: None,
            state: "succeeded".into(),
            outcome: Some("succeeded".into()),
            exit_code: Some(0),
            error: None,
            llm_calls: 2,
            outputs: BTreeMap::from([("patch".into(), "art1".into())]),
            ts_ms: 1,
        };
        let body = body_of(&ev);
        let run = serde_json::to_value(&body["run"]).unwrap();
        assert_eq!(run["outputs"]["patch"], "art1");
        assert_eq!(serde_json::to_value(&body["chain_id"]).unwrap(), "c1");
        assert_eq!(serde_json::to_value(&body["topic"]).unwrap(), "succeeded");
    }
}
