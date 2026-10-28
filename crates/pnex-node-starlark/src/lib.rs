//! Custom EdgeLinkd node `pnex-starlark` — sandboxed Starlark executor for
//! the functions of the "Functions" registry (`starlark` language).
//!
//! Role in the architecture:
//! - `executor.rs` = execution lib **shared** by the flow node and by
//!   the `pnex-flow-runtime --test-function` test CLI → test ≡
//!   in-flow runtime parity guaranteed by construction;
//! - **compile/eval at node build** (syntax + `handle` defined) → the
//!   deploy's `--check` pre-flight catches errors (400 `engine_load`),
//!   whereas the vendor `function` node only evaluates on the 1st message;
//! - Evaluator limits (ticks, heap, callstack, wall-time deadline) —
//!   Starlark has no I/O by design; isolating "weird" functions
//!   remains an infra topic (1 tenant = 1 process).

pub mod executor;

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

// Wildcard imports like the reference plugin edgelink-nodes-dummy:
// the `#[flow_node]` macro expands code (MetaNode, NodeFactory,
// FlowsElement, ElementId, Context…) resolved in the module scope.
use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

/// Anchor referenced by the `pnex-flow-runtime` binary: guarantees that
/// linking keeps this crate's `inventory` submissions.
pub fn registered() {}

fn default_timeout_ms() -> u64 {
    1000
}

#[derive(Debug, Deserialize)]
pub(crate) struct StarlarkNodeConfig {
    /// Starlark code inlined by the projection (pinned version).
    code: String,
    /// The declared interface (`inputs` present in the artefact, informative)
    /// is NOT declared here: the executor reparses the code's directives —
    /// source of truth; each input implicitly reads `payload.<name>`.
    /// Wall-time deadline per message (bound against infinite loops).
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    /// The org's pack settings (D368), stamped at deploy.
    #[serde(default)]
    settings: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug)]
#[flow_node("pnex-starlark", red_name = "pnex-starlark")]
struct StarlarkNode {
    base: BaseFlowNodeState,
    config: StarlarkNodeConfig,
}

impl StarlarkNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = StarlarkNodeConfig::deserialize(&config.rest).map_err(|e| {
            EdgelinkError::BadFlowsJson(format!("pnex-starlark: invalid config: {e}"))
        })?;

        // Compiles + evaluates the module (defs only) and requires `handle`:
        // error = rejection at BUILD → `--check` pre-flight fails (400
        // engine_load) — the editor sees the real engine error at deploy.
        executor::compile_check(&cfg.code)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("pnex-starlark : {e}")))?;

        Ok(Box::new(StarlarkNode {
            base: base_node,
            config: cfg,
        }))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        // 1) Reads the incoming msg as a JSON object (serializable boundary).
        let incoming: serde_json::Value = {
            let m = msg.read().await;
            serde_json::to_value(&*m).map_err(|e| {
                EdgelinkError::InvalidOperation(format!(
                    "pnex-starlark [{}]: msg not serializable: {e}",
                    self.name()
                ))
            })?
        };

        // 2) Sandboxed execution outside the async loop (spawn_blocking) and
        //    cancellable (flow stop). The execution runtime is rebuilt
        //    per message (vendor function node pattern: fresh context).
        let code = self.config.code.clone();
        let incoming_exec = incoming.clone();
        let timeout_ms = self.config.timeout_ms;
        let settings = self.config.settings.clone();
        let exec = tokio::task::spawn_blocking(move || {
            executor::execute_with(&code, &incoming_exec, timeout_ms, &settings)
        });
        let outcome = tokio::select! {
            res = exec => res.map_err(|e| {
                EdgelinkError::InvalidOperation(format!(
                    "pnex-starlark [{}]: execution task: {e}",
                    self.name()
                ))
            })?,
            _ = cancel.cancelled() => return Err(EdgelinkError::TaskCancelled.into()),
        };
        let outcome = outcome.map_err(|e| {
            EdgelinkError::InvalidOperation(format!("pnex-starlark [{}] : {e}", self.name()))
        })?;

        // 3) Return mapping (same contract as the JS wrapper) → fan-out.
        let outs = executor::map_return(&outcome.return_json, &outcome.outputs, &incoming);
        let mut envelopes: smallvec::SmallVec<[Envelope; 4]> = smallvec::SmallVec::new();
        for (port, out) in outs.into_iter().enumerate() {
            if let Some(v) = out {
                let variant: Variant = serde_json::from_value(v).map_err(|e| {
                    EdgelinkError::InvalidOperation(format!(
                        "pnex-starlark [{}]: output not convertible: {e}",
                        self.name()
                    ))
                })?;
                let mut m = Msg::default();
                *m.as_variant_mut() = variant;
                envelopes.push(Envelope {
                    port,
                    msg: MsgHandle::new(m),
                });
            }
        }
        self.fan_out_many(envelopes, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for StarlarkNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &StarlarkNode, msg: MsgHandle| async move {
                    // `with_uow` routes errors to `flow.handle_error` without
                    // logging them: we log here for operations.
                    match node.execute(msg.clone(), cancel.child_token()).await {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            log::warn!("pnex-starlark [{}]: message rejected: {e}", node.name());
                            Err(e)
                        }
                    }
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
    fn node_enregistre_dans_le_registre() {
        let reg = edgelink_core::runtime::registry::RegistryBuilder::default()
            .build()
            .expect("registre");
        let meta = reg
            .get("pnex-starlark")
            .expect("pnex-starlark node missing from the registry");
        assert_eq!(meta.type_, "pnex-starlark");
    }

    #[test]
    fn registered_ne_panique_pas() {
        registered();
    }
}
