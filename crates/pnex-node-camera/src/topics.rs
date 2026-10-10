//! `pnex-topic-classify` — tags the text of each message with the topics
//! of one pinned taxonomy version (media-ingest.md D168). Keyword matching
//! only. The topics and the `<name>@<version>` label are stamped into the
//! artifact by the backend at projection: the runtime reads no database.
//!
//! Output (one port, always emitted): the incoming message with
//! `payload.topics` (matched ids, possibly empty) and
//! `payload.taxonomy_version`; `topic` is kept.

use std::sync::Arc;

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

use pnex_core::taxonomy::{classify_keywords, Topic, TopicClassifyConfig};

const NODE: &str = "pnex-topic-classify";

/// Explicit fields, no `flatten`: with arbitrary_precision (unified in the
/// workspace) a flattened integer is buffered as a magic map and fails.
#[derive(Debug, Deserialize)]
struct NodeConfig {
    #[serde(default)]
    taxonomy_id: String,
    #[serde(default)]
    version: i32,
    #[serde(default)]
    text_field: String,
    /// Stamped by the backend projection.
    #[serde(default)]
    topics: Vec<Topic>,
    #[serde(default)]
    taxonomy_version: String,
}

#[flow_node("pnex-topic-classify", red_name = "pnex-topic-classify")]
struct TopicClassifyNode {
    base: BaseFlowNodeState,
    config: NodeConfig,
}

/// New payload for an incoming one: `None` when it is not an object or
/// carries no text at `text_field`.
pub(crate) fn classified(
    payload: serde_json::Value,
    text_field: &str,
    topics: &[Topic],
    label: &str,
) -> Option<serde_json::Value> {
    let text = pnex_core::resolve_msg_path(&payload, text_field)?
        .as_str()?
        .to_string();
    let serde_json::Value::Object(mut map) = payload else {
        return None;
    };
    map.insert("topics".into(), classify_keywords(&text, topics).into());
    map.insert("taxonomy_version".into(), label.into());
    Some(serde_json::Value::Object(map))
}

impl TopicClassifyNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = NodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        let check = TopicClassifyConfig {
            taxonomy_id: cfg.taxonomy_id.clone(),
            version: cfg.version,
            text_field: cfg.text_field.clone(),
        };
        if let Some((code, message)) = check.check() {
            return Err(EdgelinkError::BadFlowsJson(format!("{NODE} [{code}] : {message}")).into());
        }
        if cfg.topics.is_empty() {
            log::warn!(
                "{NODE} : no topic stamped for taxonomy {} v{} (deleted?) — nothing will match",
                cfg.taxonomy_id,
                cfg.version
            );
        }
        Ok(Box::new(TopicClassifyNode {
            base: base_node,
            config: cfg,
        }))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let payload = {
            let m = msg.read().await;
            m.get("payload")
                .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
                .unwrap_or(serde_json::Value::Null)
        };
        let Some(out) = classified(
            payload,
            self.config.text_field.trim(),
            &self.config.topics,
            &self.config.taxonomy_version,
        ) else {
            log::warn!(
                "{NODE} [{}] : message dropped — payload is not an object with text at \"{}\"",
                self.name(),
                self.config.text_field
            );
            return Ok(());
        };
        msg.write()
            .await
            .set("payload".to_string(), crate::variant_of(out));
        self.fan_out_one(Envelope { port: 0, msg }, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for TopicClassifyNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &TopicClassifyNode, msg: MsgHandle| async move {
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
    fn payload_gets_topics_and_label() {
        let topics = vec![Topic {
            id: "economie".into(),
            label: "Économie".into(),
            definition: String::new(),
            keywords: vec!["pouvoir d'achat".into()],
        }];
        let p = serde_json::json!({"stream": "inter", "seg": {"text": "Le pouvoir d’achat"}});
        let out = classified(p, "seg.text", &topics, "Actu@2").expect("classified");
        assert_eq!(out["topics"], serde_json::json!(["economie"]));
        assert_eq!(out["taxonomy_version"], "Actu@2");
        assert_eq!(out["stream"], "inter");
        // No match still emits, with an empty list.
        let none = classified(
            serde_json::json!({"text": "météo"}),
            "text",
            &topics,
            "Actu@2",
        )
        .expect("emitted");
        assert_eq!(none["topics"], serde_json::json!([]));
        // Missing text or non-object payloads are dropped.
        assert!(classified(serde_json::json!({"x": 1}), "text", &topics, "").is_none());
        assert!(classified(serde_json::json!("text"), "text", &topics, "").is_none());
    }
}
