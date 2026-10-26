//! `pnex-mail-received` — event source of the new messages of mailboxes
//! (mail.md D456).
//!
//! The backend stores each new message, then publishes a `MailEvent` on
//! `pnex:mail:v1:{org}`. This node subscribes to the channel of the org
//! stamped by the deploy projection and emits one message per mail of its
//! mailboxes that passes its filters (`payload` = the mail, `topic` = its
//! subject). The deploy checked that whoever deployed reads these
//! mailboxes. No replay: a mail arriving while the flow is stopped stays in
//! the mailbox.

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

use pnex_core::mail::{mail_channel, MailEvent, MailReceivedConfig};

const NODE: &str = "pnex-mail-received";
const BACKOFF_MAX_SECS: u64 = 30;

#[derive(Debug, Deserialize)]
struct NodeConfig {
    // No `serde(flatten)`: it breaks numbers under `arbitrary_precision`.
    #[serde(default)]
    mailboxes: Vec<String>,
    #[serde(default)]
    folder: String,
    #[serde(default)]
    from_contains: String,
    #[serde(default)]
    subject_contains: String,
    #[serde(default = "yes")]
    skip_automatic: bool,
    /// Stamped by the deploy projection.
    #[serde(default)]
    pnex_org_id: i64,
}

fn yes() -> bool {
    true
}

impl NodeConfig {
    fn trigger(&self) -> MailReceivedConfig {
        MailReceivedConfig {
            mailboxes: self.mailboxes.clone(),
            folder: self.folder.clone(),
            from_contains: self.from_contains.clone(),
            subject_contains: self.subject_contains.clone(),
            skip_automatic: self.skip_automatic,
        }
    }
}

#[flow_node("pnex-mail-received", red_name = "pnex-mail-received")]
struct MailReceivedNode {
    base: BaseFlowNodeState,
    config: NodeConfig,
    trigger: MailReceivedConfig,
    client: redis::Client,
}

/// Message body of one mail.
fn body_of(ev: &MailEvent) -> BTreeMap<String, Variant> {
    let mut body = BTreeMap::new();
    body.insert(
        "payload".to_string(),
        Variant::from(serde_json::to_value(&ev.message).unwrap_or_default()),
    );
    body.insert(
        "topic".to_string(),
        Variant::from(ev.message.subject.clone()),
    );
    body
}

impl MailReceivedNode {
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
                    "{NODE} [mail_bus_unavailable] : VALKEY_URL is not set in the runtime environment"
                ))
            })?;
        let client = redis::Client::open(url.as_str()).map_err(|e| {
            EdgelinkError::InvalidOperation(format!("{NODE} : invalid VALKEY_URL: {e}"))
        })?;
        Ok(Box::new(MailReceivedNode {
            base: base_node,
            trigger: cfg.trigger(),
            config: cfg,
            client,
        }))
    }

    /// One subscription session: returns when the connection drops (the
    /// caller reconnects) or the node stops.
    async fn session(&self, stop: &CancellationToken) -> std::result::Result<(), String> {
        let channel = mail_channel(self.config.pnex_org_id);
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
            let Ok(ev) = serde_json::from_str::<MailEvent>(&raw) else {
                continue;
            };
            if !self.trigger.accepts(&ev.message) {
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
impl FlowNodeBehavior for MailReceivedNode {
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
    fn config_reads_the_projection_and_needs_a_mailbox() {
        let id = "8b0c2f4e-1c2d-4e5f-9a0b-1c2d3e4f5a6b";
        let raw = serde_json::json!({
            "type": "pnex-mail-received",
            "mailboxes": [id],
            "from_contains": "acme",
            "pnex_org_id": 3,
            "pnex_node_id": "n1",
        });
        let cfg = NodeConfig::deserialize(&raw).unwrap();
        assert_eq!(cfg.pnex_org_id, 3);
        assert!(cfg.skip_automatic, "loops are cut by default");
        assert!(cfg.trigger().check_deployable().is_none());
        let empty = NodeConfig::deserialize(&serde_json::json!({"pnex_org_id": 3})).unwrap();
        assert!(empty.trigger().check_deployable().is_some());
    }

    #[test]
    fn the_message_carries_the_mail_and_its_subject() {
        let ev: MailEvent = serde_json::from_value(serde_json::json!({"message": {
            "id": "m1", "mailbox_id": "b", "folder": "INBOX", "message_id": "<x@y>",
            "from": {"address": "elodie@acme.fr"}, "subject": "Devis", "text": "Bonjour",
            "truncated": false, "auth": {"spf": "pass", "dkim": "none", "dmarc": "pass"},
            "automatic": false, "from_pnex": false, "received_at": "2026-10-26T10:00:00Z"
        }}))
        .unwrap();
        let body = body_of(&ev);
        let payload = serde_json::to_value(&body["payload"]).unwrap();
        assert_eq!(payload["from"]["address"], "elodie@acme.fr");
        assert_eq!(payload["auth"]["dmarc"], "pass");
        assert_eq!(serde_json::to_value(&body["topic"]).unwrap(), "Devis");
    }
}
