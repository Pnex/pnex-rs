//! `pnex-value` node — replaces `msg.payload` with a fixed JSON value
//! (mode `static`) or a uniform random number in [min, max] (mode `random`).
//!
//! The node is a **transformer**, not an autonomous source: the trigger stays
//! upstream (inject). The config + structural check come from
//! `pnex_core` (single source of truth — what the editor flags is exactly
//! what fails at deploy).

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

/// Anti-stripping anchor: without a reference to the crate the linker may
/// drop the `inventory` submissions (same guard as the runtime binary calls).
pub fn registered() {}

#[derive(Debug, Deserialize)]
struct ValueNodeConfig {
    #[serde(flatten)]
    config: pnex_core::ValueConfig,
}

#[flow_node("pnex-value", red_name = "pnex-value")]
struct PnexValueNode {
    base: BaseFlowNodeState,
    config: ValueNodeConfig,
}

impl PnexValueNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = ValueNodeConfig::deserialize(&config.rest).map_err(|e| {
            EdgelinkError::BadFlowsJson(format!("pnex-value : invalid config : {e}"))
        })?;
        // Same rule as the save validation (pnex_core::ValueConfig::check):
        // a config the editor flags must fail loud at deploy, never silently
        // emit a null payload or an inverted range.
        if let Some((code, message)) = cfg.config.check() {
            return Err(
                EdgelinkError::BadFlowsJson(format!("pnex-value [{code}] : {message}")).into(),
            );
        }
        Ok(Box::new(PnexValueNode {
            base: base_node,
            config: cfg,
        }))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let json = match self.config.config.mode {
            pnex_core::ValueMode::Static => self.config.config.value.clone(),
            pnex_core::ValueMode::Random => {
                use rand::RngExt;
                let mut rng = rand::rng();
                serde_json::json!(rng.random_range(self.config.config.min..=self.config.config.max))
            }
        };
        let payload: Variant = serde_json::from_value(json).map_err(|e| {
            EdgelinkError::InvalidOperation(format!("pnex-value : non-convertible payload : {e}"))
        })?;
        {
            let mut m = msg.write().await;
            m.set("payload".to_string(), payload);
        }
        self.fan_out_one(Envelope { port: 0, msg }, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for PnexValueNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &PnexValueNode, msg: MsgHandle| async move {
                    match node.execute(msg.clone(), cancel.child_token()).await {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            log::warn!("pnex-value [{}] : message rejected : {e}", node.name());
                            Err(e)
                        }
                    }
                },
            )
            .await;
        }
    }
}
