//! Agent run node (agent-environments.md D713): `pnex-agent-run`. Each message reads the
//! artifact ids, the chain id and the subject at the paths of the deployed config, then
//! calls `POST /internal/flow/agent-run`; the backend reads the agent and the chain bounds
//! from the deployed graph and answers the run id at once. The flow never waits for the
//! run: `agent-run-finished` emits its end. Port 0 started or joined, port 1 refused.

use std::collections::BTreeMap;
use std::time::Duration;

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

use crate::proxy::{proxy_node_behavior, BackendProxy};
use pnex_core::agent_chains::{
    msg_string, AgentRunNodeReply, AgentRunNodeRequest, NODE_PORT_REFUSED,
};

const NAME: &str = "pnex-agent-run";
/// The backend only queues the run.
const TIMEOUT: Duration = Duration::from_secs(30);
const UNAVAILABLE: &str = pnex_core::err_codes::FLOW_RUNTIME;

#[derive(Debug, Default, Deserialize)]
struct Paths {
    #[serde(default)]
    inputs: BTreeMap<String, String>,
    #[serde(default)]
    subject_from: String,
}

#[flow_node("pnex-agent-run", red_name = "pnex-agent-run")]
struct AgentRunNode {
    base: BaseFlowNodeState,
    proxy: BackendProxy,
    paths: Paths,
}

/// The request for one message, read as a whole JSON object.
fn request_of(
    paths: &Paths,
    msg: &Value,
    proxy_node: &pnex_core::ontology::flow::FlowNodeRef,
) -> AgentRunNodeRequest {
    AgentRunNodeRequest {
        node: proxy_node.clone(),
        inputs: paths
            .inputs
            .iter()
            .filter_map(|(name, path)| Some((name.clone(), msg_string(msg, path)?)))
            .collect(),
        chain_id: msg_string(msg, "chain_id"),
        subject: (!paths.subject_from.is_empty())
            .then(|| msg_string(msg, &paths.subject_from))
            .flatten(),
    }
}

impl AgentRunNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let paths = Paths::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NAME} : invalid config : {e}")))?;
        Ok(Box::new(AgentRunNode {
            base: base_node,
            proxy: BackendProxy::from_config(NAME, config, "PNEX_FLOW_AGENT_RUN_URL", TIMEOUT)?,
            paths,
        }))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let whole = {
            let m = msg.read().await;
            serde_json::to_value(&*m).unwrap_or(Value::Null)
        };
        let req = request_of(&self.paths, &whole, &self.proxy.node);
        let reply: AgentRunNodeReply = self
            .proxy
            .call(self.name(), &req, UNAVAILABLE, |code| AgentRunNodeReply {
                port: NODE_PORT_REFUSED,
                code: Some(code),
                ..Default::default()
            })
            .await;
        {
            let mut m = msg.write().await;
            let run = serde_json::json!({
                "run_id": reply.run_id,
                "chain_id": reply.chain_id,
                "joined": reply.joined,
            });
            m.set("run".into(), Variant::from(run));
            if let Some(chain) = &reply.chain_id {
                m.set("chain_id".into(), Variant::from(chain.clone()));
            }
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

proxy_node_behavior!(AgentRunNode);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_reads_the_configured_paths_only() {
        let paths = Paths {
            inputs: BTreeMap::from([
                ("ticket".into(), "payload.ticket".into()),
                ("patch".into(), "run.outputs.patch".into()),
            ]),
            subject_from: "payload.key".into(),
        };
        let node = pnex_core::ontology::flow::FlowNodeRef {
            org_id: 1,
            flow_id: 2,
            version: 3,
            node_id: "n".into(),
        };
        let msg = serde_json::json!({
            "payload": { "ticket": "a1", "key": "repo#7" },
            "chain_id": "c1",
        });
        let req = request_of(&paths, &msg, &node);
        assert_eq!(req.inputs.len(), 1);
        assert_eq!(req.inputs["ticket"], "a1");
        assert_eq!(req.chain_id.as_deref(), Some("c1"));
        assert_eq!(req.subject.as_deref(), Some("repo#7"));
    }
}
