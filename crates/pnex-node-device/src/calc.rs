//! Nœud `pnex-calc` (Phase 6 ETL) — évalue l'expression sur les clés du
//! payload device via l'évaluateur partagé `pnex_core::eval_calc` (même
//! fonction que la validation live de l'éditeur : ce qui est validé à la
//! sauvegarde est exactement ce qui est exécuté).
//!
//! Contrat d'entrée : payload objet `clé → numérique` (sortie d'un nœud
//! `pnex-device`), booléens convertis 1/0 — rejet typé sinon. Sortie :
//! `msg.payload = <f64>`.

use std::collections::HashMap;
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
struct CalcNodeConfig {
    expression: String,
}

#[derive(Debug)]
#[flow_node("pnex-calc", red_name = "pnex-calc")]
struct PnexCalcNode {
    base: BaseFlowNodeState,
    config: CalcNodeConfig,
}

impl PnexCalcNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = CalcNodeConfig::deserialize(&config.rest).map_err(|e| {
            EdgelinkError::BadFlowsJson(format!("pnex-calc : config invalide : {e}"))
        })?;
        if let Some(e) = pnex_core::validate_calc(&cfg.expression).first() {
            return Err(EdgelinkError::BadFlowsJson(format!("pnex-calc : {e}")).into());
        }
        Ok(Box::new(PnexCalcNode { base: base_node, config: cfg }))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let vars: HashMap<String, f64> = {
            let m = msg.read().await;
            let payload_json = match m.get("payload").cloned() {
                Some(v) => Some(serde_json::to_value(&v).map_err(|e| {
                    EdgelinkError::InvalidOperation(format!("pnex-calc : payload non sérialisable : {e}"))
                })?),
                None => None,
            };
            pnex_core::numeric_map_from_payload(payload_json.as_ref(), "calc").map_err(|v| {
                EdgelinkError::InvalidOperation(format!("pnex-calc [{}] : {}", self.name(), v.message))
            })?
        };

        let value = pnex_core::eval_calc(&self.config.expression, &vars).map_err(|e| {
            EdgelinkError::InvalidOperation(format!("pnex-calc [{}] : {e}", self.name()))
        })?;

        log::debug!("pnex-calc [{}] : {expression} = {value}", self.name(), expression = self.config.expression);
        let payload: Variant = serde_json::from_value(serde_json::json!(value)).map_err(|e| {
            EdgelinkError::InvalidOperation(format!("pnex-calc : résultat non convertible : {e}"))
        })?;
        {
            let mut m = msg.write().await;
            m.set("payload".to_string(), payload);
        }
        self.fan_out_one(Envelope { port: 0, msg }, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for PnexCalcNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(self.as_ref(), cancel.child_token(), |node: &PnexCalcNode, msg: MsgHandle| async move {
                match node.execute(msg.clone(), cancel.child_token()).await {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        log::warn!("pnex-calc [{}] : message rejeté : {e}", node.name());
                        Err(e)
                    }
                }
            })
            .await;
        }
    }
}
