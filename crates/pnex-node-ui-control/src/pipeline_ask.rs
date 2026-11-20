//! Pipeline ask node (pipelines.md D999): `pnex-pipeline-ask`. Each message gives
//! its `payload` (one object or an array of objects) as rows to
//! `POST /internal/flow/pipeline-ask`; the backend reads the pipeline and the daily
//! ceiling from the deployed graph and answers at once. The flow never waits for the
//! collection: follow its results with an ontology trigger. Port 0 asked, port 1 refused.

use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::Result;
use edgelink_macro::*;

use crate::proxy::{proxy_node_behavior, BackendProxy};
use pnex_core::pipeline::ask::{rows_of, PipelineAskReply, PipelineAskRequest, NODE_PORT_REFUSED};

const NAME: &str = "pnex-pipeline-ask";
/// The backend asks the rows, it never waits for the collection.
const TIMEOUT: Duration = Duration::from_secs(30);
const UNAVAILABLE: &str = pnex_core::err_codes::FLOW_RUNTIME;

#[flow_node("pnex-pipeline-ask", red_name = "pnex-pipeline-ask")]
struct PipelineAskNode {
    base: BaseFlowNodeState,
    proxy: BackendProxy,
}

impl PipelineAskNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        Ok(Box::new(PipelineAskNode {
            base: base_node,
            proxy: BackendProxy::from_config(NAME, config, "PNEX_FLOW_PIPELINE_ASK_URL", TIMEOUT)?,
        }))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let payload = {
            let m = msg.read().await;
            m.get("payload")
                .and_then(|v| serde_json::to_value(v).ok())
                .unwrap_or(Value::Null)
        };
        let req = PipelineAskRequest {
            node: self.proxy.node.clone(),
            rows: rows_of(&payload),
        };
        let reply: PipelineAskReply = self
            .proxy
            .call(self.name(), &req, UNAVAILABLE, |code| PipelineAskReply {
                port: NODE_PORT_REFUSED,
                code: Some(code),
                ..Default::default()
            })
            .await;
        {
            let mut m = msg.write().await;
            m.set("asked".into(), Variant::from(reply.asked as i64));
            if let Some(code) = &reply.code {
                let error = serde_json::json!({ "code": code, "args": reply.args });
                m.set("error".into(), Variant::from(error));
            }
        }
        self.fan_out_one(
            Envelope {
                port: reply.port,
                msg,
            },
            cancel,
        )
        .await
    }
}

proxy_node_behavior!(PipelineAskNode);
