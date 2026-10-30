//! Mail send node (mail.md D456, D518): `pnex-mail-send`. The runtime
//! holds no mailbox and no password: each message calls
//! `POST /internal/flow/mail/send` naming the deployed node; the backend
//! reads the mailbox from the deployed config and writes the mail (a draft
//! in its outbox, or sent when the mailbox sends on its own). Port 0
//! written, port 1 refused.

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

use pnex_core::mail::{MailSendNodeReply, MailSendNodeRequest, SEND_PORT_REFUSED};
use pnex_core::ontology::flow::FlowNodeRef;

const NAME: &str = "pnex-mail-send";
const FLOW_TOKEN_HEADER: &str = "x-pnex-flow-token";
/// The backend gives SMTP 30 s.
const TIMEOUT: Duration = Duration::from_secs(45);
const UNAVAILABLE: &str = pnex_core::err_codes::MAIL_UNREACHABLE;

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

#[flow_node("pnex-mail-send", red_name = "pnex-mail-send")]
struct MailSendNode {
    base: BaseFlowNodeState,
    node: FlowNodeRef,
    http: reqwest::Client,
    url: String,
    token: String,
}

/// `to` as a list or one address; the other fields as text.
fn request(node: &FlowNodeRef, p: &Value) -> MailSendNodeRequest {
    let text = |k: &str| p[k].as_str().map(str::to_string);
    let to = match &p["to"] {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    };
    MailSendNodeRequest {
        node: node.clone(),
        to,
        subject: text("subject").unwrap_or_default(),
        text: text("text").unwrap_or_default(),
        in_reply_to: text("in_reply_to").filter(|s| !s.is_empty()),
        key: text("key").filter(|s| !s.is_empty()),
    }
}

impl MailSendNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let c = Common::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NAME} : invalid config : {e}")))?;
        if c.pnex_org_id <= 0 || c.pnex_flow_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "{NAME} : pnex_org_id / pnex_flow_id missing from the artifact (redeploy the flow)"
            ))
            .into());
        }
        let base = std::env::var("PNEX_FLOW_MAIL_URL").map_err(|_| {
            EdgelinkError::InvalidOperation(format!(
                "{NAME} : PNEX_FLOW_MAIL_URL is not set in the runtime environment"
            ))
        })?;
        let token = std::env::var("PNEX_FLOW_WRITE_TOKEN").map_err(|_| {
            EdgelinkError::InvalidOperation(format!(
                "{NAME} : PNEX_FLOW_WRITE_TOKEN is not set in the runtime environment"
            ))
        })?;
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| EdgelinkError::InvalidOperation(format!("{NAME} : http client: {e}")))?;
        Ok(Box::new(MailSendNode {
            base: base_node,
            node: FlowNodeRef {
                org_id: c.pnex_org_id,
                flow_id: c.pnex_flow_id,
                version: c.pnex_version,
                node_id: c.pnex_node_id,
            },
            http,
            url: format!("{}/send", base.trim_end_matches('/')),
            token,
        }))
    }

    /// The backend's answer, or a refusal when it cannot be had.
    async fn call(&self, req: &MailSendNodeRequest) -> MailSendNodeReply {
        let refused = |code: String| MailSendNodeReply {
            port: SEND_PORT_REFUSED,
            code: Some(code),
            ..Default::default()
        };
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
            Ok(r) if r.status().is_success() => {
                r.json::<MailSendNodeReply>().await.unwrap_or_else(|e| {
                    log::warn!("{NAME} [{}] : unreadable answer: {e}", self.name());
                    refused(UNAVAILABLE.into())
                })
            }
            Ok(r) => {
                let status = r.status();
                let body: Value = r.json().await.unwrap_or(Value::Null);
                log::warn!("{NAME} [{}] : refused ({status}): {body}", self.name());
                refused(body["code"].as_str().unwrap_or(UNAVAILABLE).to_string())
            }
            Err(e) => {
                log::warn!("{NAME} [{}] : call failed: {e}", self.name());
                refused(UNAVAILABLE.into())
            }
        }
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let req = {
            let m = msg.read().await;
            let payload = m
                .get("payload")
                .and_then(|v| serde_json::to_value(v).ok())
                .unwrap_or(Value::Null);
            request(&self.node, &payload)
        };
        let reply = self.call(&req).await;
        {
            let mut m = msg.write().await;
            if let Some(mail) = &reply.mail {
                let v = serde_json::to_value(mail).unwrap_or(Value::Null);
                m.set("mail".into(), Variant::from(v));
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

#[async_trait]
impl FlowNodeBehavior for MailSendNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &MailSendNode, msg: MsgHandle| async move {
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
    fn the_payload_becomes_one_mail() {
        let node = FlowNodeRef {
            org_id: 2,
            flow_id: 7,
            version: 3,
            node_id: "n".into(),
        };
        let r = request(
            &node,
            &serde_json::json!({
                "to": "ann@acme.fr", "subject": "Hi", "text": "Body", "in_reply_to": "", "key": "k1"
            }),
        );
        assert_eq!(r.to, vec!["ann@acme.fr".to_string()]);
        assert_eq!((r.in_reply_to, r.key.as_deref()), (None, Some("k1")));
        let r = request(&node, &serde_json::json!({"to": ["a@b.fr", 3, "c@d.fr"]}));
        assert_eq!(r.to.len(), 2, "only addresses are kept");
    }
}
