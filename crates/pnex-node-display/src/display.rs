//! Nœud `pnex-display` (sonde) — passthrough + publication au canal debug du
//! moteur avec l'id canvas brut. Le backend collecte ces événements (stdout
//! du runtime → anneau mémoire) pour le panneau de debug et le badge live
//! sous le nœud dans l'éditeur.
//!
//! Garde-fous :
//! - `pnex_node_id` **requis** au build (estampillé par la projection) — un
//!   artefact qui ne le porte pas est rejeté au déploiement (fail-loud) ;
//! - passthrough intact : le message poursuit sa route inchangé.

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

#[derive(Debug, Deserialize)]
struct DisplayNodeConfig {
    /// Estampillé par la projection = id canvas du nœud (`"n3"`) — la clé de
    /// rattachement du panneau et du badge. Jamais saisi par l'utilisateur.
    /// (Les autres stamps de traçabilité — pnex_flow_id/version/org_id — sont
    /// présents dans l'artefact mais ignorés ici : le nœud n'en a pas besoin.)
    pnex_node_id: String,
}

#[flow_node("pnex-display", red_name = "pnex-display")]
struct PnexDisplayNode {
    base: BaseFlowNodeState,
    config: DisplayNodeConfig,
}

impl PnexDisplayNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = DisplayNodeConfig::deserialize(&config.rest).map_err(|e| {
            EdgelinkError::BadFlowsJson(format!("pnex-display : config invalide : {e}"))
        })?;
        if cfg.pnex_node_id.trim().is_empty() {
            return Err(EdgelinkError::BadFlowsJson(
                "pnex-display : pnex_node_id absent de l'artefact (redéployer le flow)".into(),
            )
            .into());
        }
        Ok(Box::new(PnexDisplayNode { base: base_node, config: cfg }))
    }

    /// Passthrough + publication : la valeur (payload si présent, sinon le
    /// message entier) part sur le canal debug avec l'id canvas brut, puis
    /// le message original poursuit sa route inchangé.
    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let (captured, topic, msgid) = {
            let m = msg.read().await;
            let captured: serde_json::Value = match m.get("payload") {
                Some(v) => serde_json::to_value(v).unwrap_or(serde_json::Value::Null),
                None => serde_json::to_value(&*m).unwrap_or(serde_json::Value::Null),
            };
            let topic = m.get("topic").and_then(|t| t.as_str()).map(str::to_string);
            let msgid = m.get("_msgid").and_then(|v| v.as_str()).map(str::to_string);
            (captured, topic, msgid)
        };

        if let Some(engine) = self.engine() {
            let path = self.flow().map(|f| f.get_path()).unwrap_or_else(|| "global".to_string());
            let name = self.name();
            engine.debug_channel().send(edgelink_core::runtime::debug_channel::DebugMessage {
                // Id canvas BRUT (pas le hash moteur) : matching direct avec
                // le nœud de l'éditeur pour le badge live.
                id: self.config.pnex_node_id.clone(),
                name: if name.is_empty() { None } else { Some(name.to_string()) },
                msg: captured,
                property: Some("payload".into()),
                format: Some("pnex-display".into()),
                path,
                topic,
                timestamp: Some(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0),
                ),
                msgid,
            });
        } else {
            log::warn!("pnex-display [{}] : moteur inaccessible, pas de publication", self.name());
        }

        // Passthrough intact.
        self.fan_out_one(Envelope { port: 0, msg }, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for PnexDisplayNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(self.as_ref(), cancel.child_token(), |node: &PnexDisplayNode, msg: MsgHandle| async move {
                match node.execute(msg.clone(), cancel.child_token()).await {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        log::warn!("pnex-display [{}] : message rejeté : {e}", node.name());
                        Err(e)
                    }
                }
            })
            .await;
        }
    }
}
