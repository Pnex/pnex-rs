//! Shared plumbing of the nodes that hand each message to a backend
//! `/internal/flow/*` route (web search, web visit, site crawl, site recipe,
//! org resolve): deployed-node identity, HTTP client, the call and its
//! refusals, and the message loop.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use edgelink_core::runtime::model::json::RedFlowNodeConfig;
use edgelink_core::{EdgelinkError, Result};

use pnex_core::flow::FLOW_TOKEN_HEADER;
use pnex_core::ontology::flow::FlowNodeRef;

/// Deployed-node identity stamped into the artifact by the backend.
#[derive(Debug, Deserialize)]
struct Common {
    #[serde(default)]
    pnex_node_id: String,
    #[serde(default)]
    pnex_flow_id: i64,
    #[serde(default)]
    pnex_version: i64,
    #[serde(default)]
    pnex_org_id: i64,
}

/// Client of one backend route on behalf of one deployed node.
pub(crate) struct BackendProxy {
    kind: &'static str,
    pub(crate) node: FlowNodeRef,
    http: reqwest::Client,
    url: String,
    token: String,
}

impl BackendProxy {
    /// From the node config; `url_env` names the route URL variable.
    pub(crate) fn from_config(
        kind: &'static str,
        config: &RedFlowNodeConfig,
        url_env: &str,
        timeout: Duration,
    ) -> Result<Self> {
        let c = Common::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{kind} : invalid config : {e}")))?;
        if c.pnex_org_id <= 0 || c.pnex_flow_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "{kind} : pnex_org_id / pnex_flow_id missing from the artifact (redeploy the flow)"
            ))
            .into());
        }
        let env = |k: &str| {
            std::env::var(k).map_err(|_| {
                EdgelinkError::InvalidOperation(format!(
                    "{kind} : {k} is not set in the runtime environment"
                ))
            })
        };
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| EdgelinkError::InvalidOperation(format!("{kind} : http client: {e}")))?;
        Ok(Self {
            kind,
            node: FlowNodeRef {
                org_id: c.pnex_org_id,
                flow_id: c.pnex_flow_id,
                version: c.pnex_version,
                node_id: c.pnex_node_id,
            },
            http,
            url: env(url_env)?,
            token: env("PNEX_FLOW_WRITE_TOKEN")?,
        })
    }

    /// The backend's answer, or `refused(code)` when it cannot be had: the
    /// backend's refusal code, else `unavailable`. `name` labels the logs.
    pub(crate) async fn call<Req: Serialize, Rep: DeserializeOwned>(
        &self,
        name: &str,
        req: &Req,
        unavailable: &str,
        refused: impl Fn(String) -> Rep,
    ) -> Rep {
        let kind = self.kind;
        let res = self
            .http
            .post(&self.url)
            .header(FLOW_TOKEN_HEADER, &self.token)
            .header(
                pnex_core::FLOW_WORKER_HEADER,
                pnex_core::flow_worker_fence().unwrap_or_default(),
            )
            .json(req)
            .send()
            .await;
        match res {
            Ok(r) if r.status().is_success() => r.json::<Rep>().await.unwrap_or_else(|e| {
                log::warn!("{kind} [{name}] : unreadable answer: {e}");
                refused(unavailable.into())
            }),
            Ok(r) => {
                let status = r.status();
                let body: Value = r.json().await.unwrap_or(Value::Null);
                log::warn!("{kind} [{name}] : refused ({status}): {body}");
                refused(body["code"].as_str().unwrap_or(unavailable).to_string())
            }
            Err(e) => {
                log::warn!("{kind} [{name}] : call failed: {e}");
                refused(unavailable.into())
            }
        }
    }
}

/// `FlowNodeBehavior` of a proxy node: each message through its
/// `execute(msg, cancel)`.
macro_rules! proxy_node_behavior {
    ($node:ty) => {
        #[async_trait::async_trait]
        impl FlowNodeBehavior for $node {
            fn get_base(&self) -> &BaseFlowNodeState {
                &self.base
            }

            async fn run(self: std::sync::Arc<Self>, stop_token: CancellationToken) {
                while !stop_token.is_cancelled() {
                    let cancel = stop_token.child_token();
                    with_uow(
                        self.as_ref(),
                        cancel.child_token(),
                        |node: &$node, msg: MsgHandle| async move {
                            node.execute(msg, cancel.child_token()).await
                        },
                    )
                    .await;
                }
            }
        }
    };
}
pub(crate) use proxy_node_behavior;
