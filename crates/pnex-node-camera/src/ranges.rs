//! `pnex-range-upsert` — writes `msg.payload` as a time range of the
//! node's scope (media-ingest.md D169) through the backend internal route
//! `/internal/flow/time-range`: the runtime has no database access, the
//! backend re-checks the scope against the stamped org and the fields.
//!
//! Output (one port): on success the message goes on with `msg.range_id`
//! and `msg.range_created`; a refused or failed write is logged and the
//! message dropped (downstream only sees stored ranges).

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

use pnex_core::time_range::{
    RangeUpsertAck, RangeUpsertConfig, RangeUpsertRequest, TimeRangeInput,
};

const NODE: &str = "pnex-range-upsert";
const FLOW_TOKEN_HEADER: &str = "x-pnex-flow-token";

/// Explicit fields, no `flatten` (arbitrary_precision, see topics.rs).
#[derive(Debug, Deserialize)]
struct NodeConfig {
    #[serde(default)]
    scope_kind: String,
    #[serde(default)]
    scope_id: String,
    #[serde(default)]
    origin: String,
    #[serde(default)]
    pnex_node_id: String,
    #[serde(default)]
    pnex_flow_id: i64,
    #[serde(default)]
    pnex_version: i64,
    #[serde(default)]
    pnex_org_id: i64,
}

#[flow_node("pnex-range-upsert", red_name = "pnex-range-upsert")]
struct RangeUpsertNode {
    base: BaseFlowNodeState,
    config: NodeConfig,
    http: reqwest::Client,
    url: String,
    token: String,
}

/// Request body for an incoming payload: `None` when it is not an object
/// of range fields.
fn request_of(payload: serde_json::Value, c: &NodeConfig) -> Option<RangeUpsertRequest> {
    if !payload.is_object() {
        return None;
    }
    let mut range: TimeRangeInput = serde_json::from_value(payload).ok()?;
    // The scope is the node's, never the message's.
    range.scope_kind = None;
    range.scope_id = None;
    Some(RangeUpsertRequest {
        org_id: c.pnex_org_id,
        flow_id: c.pnex_flow_id,
        version: c.pnex_version,
        node_id: c.pnex_node_id.clone(),
        scope_kind: c.scope_kind.clone(),
        scope_id: c.scope_id.trim().to_string(),
        origin: c.origin.clone(),
        range,
    })
}

impl RangeUpsertNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = NodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        let check = RangeUpsertConfig {
            scope_kind: cfg.scope_kind.clone(),
            scope_id: cfg.scope_id.clone(),
            origin: cfg.origin.clone(),
        };
        if let Some((code, message)) = check.check() {
            return Err(EdgelinkError::BadFlowsJson(format!("{NODE} [{code}] : {message}")).into());
        }
        if cfg.pnex_org_id <= 0 || cfg.pnex_flow_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "{NODE} : pnex_org_id / pnex_flow_id missing from the artifact (redeploy the flow)"
            ))
            .into());
        }
        let url = std::env::var("PNEX_FLOW_TIME_RANGE_URL").map_err(|_| {
            EdgelinkError::InvalidOperation(format!(
                "{NODE} : PNEX_FLOW_TIME_RANGE_URL is not set in the runtime environment \
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
        Ok(Box::new(RangeUpsertNode {
            base: base_node,
            config: cfg,
            http,
            url,
            token,
        }))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let payload = {
            let m = msg.read().await;
            m.get("payload")
                .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
                .unwrap_or(serde_json::Value::Null)
        };
        let Some(req) = request_of(payload, &self.config) else {
            log::warn!(
                "{NODE} [{}] : message dropped — payload is not an object of range fields",
                self.name()
            );
            return Ok(());
        };
        let res = self
            .http
            .post(&self.url)
            .header(FLOW_TOKEN_HEADER, &self.token)
            .header(
                pnex_core::FLOW_WORKER_HEADER,
                pnex_core::flow_worker_fence().unwrap_or_default(),
            )
            .json(&req)
            .send()
            .await;
        let ack = match res {
            Ok(r) if r.status().is_success() => r.json::<RangeUpsertAck>().await.ok(),
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                log::warn!(
                    "{NODE} [{}] : range refused ({status}): {body}",
                    self.name()
                );
                None
            }
            Err(e) => {
                log::warn!("{NODE} [{}] : range write failed: {e}", self.name());
                None
            }
        };
        let Some(ack) = ack else {
            return Ok(());
        };
        {
            let mut m = msg.write().await;
            m.set("range_id".to_string(), Variant::from(ack.id));
            m.set("range_created".to_string(), Variant::from(ack.created));
        }
        self.fan_out_one(Envelope { port: 0, msg }, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for RangeUpsertNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &RangeUpsertNode, msg: MsgHandle| async move {
                    node.execute(msg, cancel.child_token()).await
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
    fn request_keeps_fields_and_forces_the_node_scope() {
        let c = NodeConfig {
            scope_kind: "stream".into(),
            scope_id: " s-1 ".into(),
            origin: "grid".into(),
            pnex_node_id: "n1".into(),
            pnex_flow_id: 4,
            pnex_version: 2,
            pnex_org_id: 9,
        };
        let p = serde_json::json!({
            "external_id": "7-9", "label": "Le 7/9",
            "planned_start": "2026-10-12T07:00:00+02:00",
            "planned_end": "2026-10-12T09:00:00+02:00",
            "attrs": {"host": "A"},
            "scope_kind": "org", "scope_id": "999",
            "unknown": 1
        });
        let r = request_of(p, &c).expect("request");
        assert_eq!((r.org_id, r.flow_id, r.version), (9, 4, 2));
        assert_eq!(r.scope_kind, "stream");
        assert_eq!(r.scope_id, "s-1");
        assert_eq!(r.range.scope_kind, None);
        assert_eq!(r.range.scope_id, None);
        assert_eq!(r.range.external_id.as_deref(), Some("7-9"));
        assert_eq!(r.range.attrs, Some(serde_json::json!({"host": "A"})));
        assert!(request_of(serde_json::json!("x"), &c).is_none());
        // A field of the wrong type is not a range.
        assert!(request_of(serde_json::json!({"label": 3}), &c).is_none());
    }
}
