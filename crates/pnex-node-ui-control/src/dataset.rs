//! `pnex-dataset-trigger`: event source of one dataset (data-pipelines.md D578).
//!
//! The server publishes `dataset.committed` and `dataset.synced` on
//! `pnex:ds:v1:{org}`. This node subscribes to the org channel and emits one message
//! per event of its dataset. The message names the version, never its rows: a flow
//! reacts to a load, it does not carry it. No replay: a frame missed while the flow
//! is stopped is gone.

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

use pnex_core::pipelines::{dataset_channel, DatasetEvent, DatasetTriggerConfig};

const NODE: &str = "pnex-dataset-trigger";
const BACKOFF_MAX_SECS: u64 = 30;

#[derive(Debug, Deserialize)]
struct NodeConfig {
    // No `serde(flatten)`: it breaks numbers under `arbitrary_precision`.
    #[serde(default)]
    dataset: String,
    #[serde(default)]
    on: String,
    /// Stamped by the deploy projection.
    #[serde(default)]
    pnex_org_id: i64,
}

impl NodeConfig {
    fn trigger(&self) -> DatasetTriggerConfig {
        DatasetTriggerConfig {
            dataset: self.dataset.clone(),
            on: self.on.clone(),
        }
    }
}

#[flow_node("pnex-dataset-trigger", red_name = "pnex-dataset-trigger")]
struct DatasetTriggerNode {
    base: BaseFlowNodeState,
    config: NodeConfig,
    trigger: DatasetTriggerConfig,
    client: redis::Client,
}

/// Message body of one event.
fn body_of(ev: &DatasetEvent) -> BTreeMap<String, Variant> {
    let mut body = BTreeMap::new();
    body.insert(
        "payload".to_string(),
        Variant::from(serde_json::json!({
            "dataset_id": ev.dataset_id,
            "dataset": ev.dataset,
            "reference": ev.reference,
            "counts": ev.counts,
            "changed": ev.changed,
            "ts_ms": ev.ts_ms,
        })),
    );
    body.insert("topic".to_string(), Variant::from(ev.event.clone()));
    body
}

impl DatasetTriggerNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = NodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        if let Some((code, message)) = cfg.trigger().check_deployable() {
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
                    "{NODE} : VALKEY_URL is not set in the runtime environment"
                ))
            })?;
        let client = redis::Client::open(url.as_str()).map_err(|e| {
            EdgelinkError::InvalidOperation(format!("{NODE} : invalid VALKEY_URL: {e}"))
        })?;
        Ok(Box::new(DatasetTriggerNode {
            base: base_node,
            trigger: cfg.trigger(),
            config: cfg,
            client,
        }))
    }

    /// One subscription session: returns when the connection drops (the caller
    /// reconnects) or the node stops.
    async fn session(&self, stop: &CancellationToken) -> std::result::Result<(), String> {
        let channel = dataset_channel(self.config.pnex_org_id);
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
            let Ok(ev) = serde_json::from_str::<DatasetEvent>(&raw) else {
                continue;
            };
            if !self.trigger.matches(&ev) {
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
impl FlowNodeBehavior for DatasetTriggerNode {
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
    fn message_carries_the_version_and_the_event_as_topic() {
        let ev = DatasetEvent {
            event: pnex_core::pipelines::DATASET_COMMITTED.into(),
            dataset_id: "d1".into(),
            dataset: "clean".into(),
            reference: "t1".into(),
            counts: serde_json::json!({ "rows": 3 }),
            changed: vec![],
            ts_ms: 5,
        };
        let body = body_of(&ev);
        let payload = serde_json::to_value(&body["payload"]).unwrap();
        assert_eq!(payload["dataset"], "clean");
        assert_eq!(payload["counts"]["rows"], 3);
        assert_eq!(
            serde_json::to_value(&body["topic"]).unwrap(),
            "dataset.committed"
        );
    }

    #[test]
    fn config_reads_the_projection() {
        let raw = serde_json::json!({
            "type": "pnex-dataset-trigger",
            "dataset": "clean",
            "on": "synced",
            "pnex_org_id": 3,
        });
        let cfg = NodeConfig::deserialize(&raw).unwrap();
        assert_eq!(cfg.pnex_org_id, 3);
        assert!(cfg.trigger().check_deployable().is_none());
    }
}
