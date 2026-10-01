//! Nœud custom EdgeLinkd `pnex-starlark` — exécuteur Starlark sandboxé pour
//! les fonctions du registre « Fonctions » (langage `starlark`).
//!
//! Rôle dans l'architecture :
//! - `executor.rs` = lib d'exécution **partagée** par le nœud de flow et par
//!   le CLI de test `pnex-flow-runtime --test-function` → parité test ≡
//!   runtime in-flow garantie par construction ;
//! - **compile/eval au build du nœud** (syntaxe + `handle` défini) → le
//!   pré-flight `--check` du deploy attrape les erreurs (400 `engine_load`),
//!   là où le nœud `function` vendor n'évalue qu'au 1er message ;
//! - limits Evaluator (ticks, heap, callstack, deadline wall-time) —
//!   Starlark est sans I/O par design ; l'isolation des fonctions «
//!   bizarres » reste un sujet d'infra (1 tenant = 1 process).

pub mod executor;

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

// Imports en wildcards comme le plugin de référence edgelink-nodes-dummy :
// la macro `#[flow_node]` développe du code (MetaNode, NodeFactory,
// FlowsElement, ElementId, Context…) résolu dans le scope du module.
use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

/// Point d'ancrage référencé par le binaire `pnex-flow-runtime` : garantit que
/// l'édition de liens conserve les soumissions `inventory` de ce crate.
pub fn registered() {}

fn default_timeout_ms() -> u64 {
    1000
}

#[derive(Debug, Deserialize)]
pub(crate) struct StarlarkNodeConfig {
    /// Code Starlark inline par la projection (version épinglée).
    code: String,
    /// L'interface déclarée (`inputs` présente dans l'artefact, informative)
    /// n'est PAS déclarée ici : l'exécuteur reparsé les directives du code —
    /// source de vérité ; chaque entrée lit implicitement `payload.<nom>`.
    /// Deadline wall-time par message (borne anti boucle infinie).
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
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
            EdgelinkError::BadFlowsJson(format!("pnex-starlark : config invalide : {e}"))
        })?;

        // Compile + évalue le module (defs uniquement) et exige `handle` :
        // erreur = rejet au BUILD → pré-flight `--check` échoue (400
        // engine_load) — l'éditeur voit l'erreur moteur réelle au deploy.
        executor::compile_check(&cfg.code)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("pnex-starlark : {e}")))?;

        Ok(Box::new(StarlarkNode {
            base: base_node,
            config: cfg,
        }))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        // 1) Lecture du msg entrant en objet JSON (frontière sérialisable).
        let incoming: serde_json::Value = {
            let m = msg.read().await;
            serde_json::to_value(&*m).map_err(|e| {
                EdgelinkError::InvalidOperation(format!(
                    "pnex-starlark [{}] : msg non sérialisable : {e}",
                    self.name()
                ))
            })?
        };

        // 2) Exécution sandboxée hors de la boucle async (spawn_blocking) et
        //    annulable (arrêt du flow). Le runtime d'exécution est reconstruit
        //    par message (école du nœud function vendor : contexte frais).
        let code = self.config.code.clone();
        let incoming_exec = incoming.clone();
        let timeout_ms = self.config.timeout_ms;
        let exec = tokio::task::spawn_blocking(move || {
            executor::execute(&code, &incoming_exec, timeout_ms)
        });
        let outcome = tokio::select! {
            res = exec => res.map_err(|e| {
                EdgelinkError::InvalidOperation(format!(
                    "pnex-starlark [{}] : tâche d'exécution : {e}",
                    self.name()
                ))
            })?,
            _ = cancel.cancelled() => return Err(EdgelinkError::TaskCancelled.into()),
        };
        let outcome = outcome.map_err(|e| {
            EdgelinkError::InvalidOperation(format!("pnex-starlark [{}] : {e}", self.name()))
        })?;

        // 3) Mapping du retour (même contrat que le wrapper JS) → fan-out.
        let outs = executor::map_return(&outcome.return_json, &outcome.outputs, &incoming);
        let mut envelopes: smallvec::SmallVec<[Envelope; 4]> = smallvec::SmallVec::new();
        for (port, out) in outs.into_iter().enumerate() {
            if let Some(v) = out {
                let variant: Variant = serde_json::from_value(v).map_err(|e| {
                    EdgelinkError::InvalidOperation(format!(
                        "pnex-starlark [{}] : sortie non convertible : {e}",
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
                    // `with_uow` route les erreurs vers `flow.handle_error` sans
                    // les logger : on journalise ici pour l'exploitation.
                    match node.execute(msg.clone(), cancel.child_token()).await {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            log::warn!("pnex-starlark [{}] : message rejeté : {e}", node.name());
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
            .expect("nœud pnex-starlark absent du registre");
        assert_eq!(meta.type_, "pnex-starlark");
    }

    #[test]
    fn registered_ne_panique_pas() {
        registered();
    }
}
