//! Nœud `pnex-metric` (Phase 6 ETL) — écrit le résultat du pipeline dans
//! OpenObserve **comme une métrique au même titre que les capteurs** :
//! remote-write sur la même pipeline d'ingestion, labels identiques
//! (`device_id` = device virtuel `flow_{id}`, `pred_dev="virtual_device"`,
//! `source_type="etl"`), nom auto-préfixé `etl_`. La série apparaît ainsi
//! d'elle-même dans le catalogue Visualisation (découverte dynamique des
//! streams metrics).
//!
//! Garde-fous : nom requis au build ; `pnex_flow_id`/`pnex_org_id`
//! estampillés par la projection (traçabilité + org O2) ; creds racine via
//! l'env du runtime — jamais de secret dans flows.json. Entrée : valeur
//! numérique (sortie d'un nœud `calc`), bool → 1/0, sinon rejet typé.
//! Sortie : payload inchangé (le debug peut être branché en aval).

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

use crate::o2::O2Client;

#[derive(Debug, Deserialize)]
struct MetricNodeConfig {
    metric_name: String,
    #[serde(default)]
    pnex_flow_id: i64,
    #[serde(default)]
    pnex_org_id: i64,
    /// Identifiant O2 **réel** (openobserve_orgs.o2_org), estampillé par la
    /// projection. Vide = org non provisionnée au deploy — l'écriture est
    /// sautée (warn) sans casser le pipeline, et une reprojection (deploy
    /// suivant ou auto après provisioning) comble le champ.
    #[serde(default)]
    pnex_o2_org: String,
}

#[flow_node("pnex-metric", red_name = "pnex-metric")]
struct PnexMetricNode {
    base: BaseFlowNodeState,
    config: MetricNodeConfig,
    o2: O2Client,
}

impl PnexMetricNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = MetricNodeConfig::deserialize(&config.rest).map_err(|e| {
            EdgelinkError::BadFlowsJson(format!("pnex-metric : config invalide : {e}"))
        })?;
        if cfg.metric_name.trim().is_empty() {
            return Err(EdgelinkError::BadFlowsJson(
                "pnex-metric : le nom de la métrique est requis".into(),
            )
            .into());
        }
        if cfg.pnex_flow_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(
                "pnex-metric : pnex_flow_id absent de l'artefact (redéployer le flow)".into(),
            )
            .into());
        }
        if cfg.pnex_org_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(
                "pnex-metric : pnex_org_id absent de l'artefact (redéployer le flow)".into(),
            )
            .into());
        }
        // org O2 vide : PAS de fail-loud au build — l'absence d'org O2 est un
        // état transitoire (provisioning pas encore passé), pas une config
        // invalide. L'exécution dégrade (écriture sautée, pipeline vivant).
        let o2 = O2Client::from_env()?;
        Ok(Box::new(PnexMetricNode {
            base: base_node,
            config: cfg,
            o2,
        }))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        // 1) Frontière d'entrée : une valeur numérique.
        let value: f64 = {
            let m = msg.read().await;
            let payload_json = match m.get("payload").cloned() {
                Some(v) => Some(serde_json::to_value(&v).map_err(|e| {
                    EdgelinkError::InvalidOperation(format!(
                        "pnex-metric : payload non sérialisable : {e}"
                    ))
                })?),
                None => None,
            };
            pnex_core::metric_value_from_payload(payload_json.as_ref()).map_err(|v| {
                EdgelinkError::InvalidOperation(format!(
                    "pnex-metric [{}] : {}",
                    self.name(),
                    v.message
                ))
            })?
        };

        // 2) Remote-write borné (timeout 10 s côté client) et annulable.
        // Org O2 vide (provisioning pas encore passé) : écriture sautée,
        // pipeline vivant — la reprojection post-provisioning comblera.
        if self.config.pnex_o2_org.trim().is_empty() {
            log::warn!(
                "pnex-metric [{}] : org O2 non provisionnée — {} = {value} NON écrit",
                self.name(),
                pnex_core::etl_metric_name(&self.config.metric_name)
            );
            self.fan_out_one(Envelope { port: 0, msg }, cancel).await
        } else {
            let org = self.config.pnex_o2_org.clone();
            let metric = pnex_core::etl_metric_name(&self.config.metric_name);
            let virtual_device = format!("flow_{}", self.config.pnex_flow_id);
            let ts_ms = chrono::Utc::now().timestamp_millis();
            let series = vec![crate::o2::etl_series(metric, virtual_device, value, ts_ms)];
            tokio::select! {
                res = self.o2.write(&org, series) => {
                    if let Err(e) = res {
                        return Err(EdgelinkError::InvalidOperation(format!(
                            "pnex-metric [{}] : écriture refusée : {e}", self.name()
                        )).into());
                    }
                }
                _ = cancel.cancelled() => return Err(EdgelinkError::TaskCancelled.into()),
            }

            log::debug!(
                "pnex-metric [{}] : {} = {value} écrit dans {org}",
                self.name(),
                pnex_core::etl_metric_name(&self.config.metric_name)
            );

            // 3) Passthrough : le payload sort inchangé (debug aval possible).
            self.fan_out_one(Envelope { port: 0, msg }, cancel).await
        }
    }
}

#[async_trait]
impl FlowNodeBehavior for PnexMetricNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &PnexMetricNode, msg: MsgHandle| async move {
                    match node.execute(msg.clone(), cancel.child_token()).await {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            log::warn!("pnex-metric [{}] : message rejeté : {e}", node.name());
                            Err(e)
                        }
                    }
                },
            )
            .await;
        }
    }
}
