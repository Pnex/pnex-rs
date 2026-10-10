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

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    /// Free labels (D171): label name → source, checked at build.
    #[serde(default)]
    labels: BTreeMap<String, String>,
}

/// Cardinality bound of one node's label tuples (D171): known tuples are
/// always admitted, new ones only below the cap.
#[derive(Default)]
struct LabelTuples {
    seen: Mutex<HashSet<Vec<String>>>,
    last_warn: Mutex<Option<Instant>>,
}

impl LabelTuples {
    /// `true` = write this tuple. A refusal warns at most once a minute.
    fn admit(&self, labels: &BTreeMap<String, String>, node: &str) -> bool {
        let tuple: Vec<String> = labels.values().cloned().collect();
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        if seen.contains(&tuple) {
            return true;
        }
        if seen.len() < pnex_core::METRIC_LABEL_TUPLES_MAX {
            seen.insert(tuple);
            return true;
        }
        drop(seen);
        let mut last = self.last_warn.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
            *last = Some(Instant::now());
            log::warn!(
                "pnex-metric [{node}]: more than {} distinct label values, new combinations are not written",
                pnex_core::METRIC_LABEL_TUPLES_MAX
            );
        }
        false
    }
}

#[flow_node("pnex-metric", red_name = "pnex-metric")]
struct PnexMetricNode {
    base: BaseFlowNodeState,
    config: MetricNodeConfig,
    o2: O2Client,
    tuples: LabelTuples,
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
        let check = pnex_core::MetricConfig {
            metric_name: cfg.metric_name.clone(),
            labels: cfg.labels.clone(),
        }
        .check();
        if let Some((_, message)) = check {
            return Err(EdgelinkError::BadFlowsJson(format!("pnex-metric: {message}")).into());
        }
        // org O2 vide : PAS de fail-loud au build — l'absence d'org O2 est un
        // état transitoire (provisioning pas encore passé), pas une config
        // invalide. L'exécution dégrade (écriture sautée, pipeline vivant).
        let o2 = O2Client::from_env()?;
        Ok(Box::new(PnexMetricNode {
            base: base_node,
            config: cfg,
            o2,
            tuples: LabelTuples::default(),
        }))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        // 1) Input boundary: a number, a boolean or an object of numeric
        // fields (one series per field, D140).
        let (values, labels): (Vec<(Option<String>, f64)>, BTreeMap<String, String>) = {
            let m = msg.read().await;
            let payload_json = match m.get("payload").cloned() {
                Some(v) => Some(serde_json::to_value(&v).map_err(|e| {
                    EdgelinkError::InvalidOperation(format!(
                        "pnex-metric : payload non sérialisable : {e}"
                    ))
                })?),
                None => None,
            };
            let values =
                pnex_core::metric_values_from_payload(payload_json.as_ref()).map_err(|v| {
                    EdgelinkError::InvalidOperation(format!(
                        "pnex-metric [{}] : {}",
                        self.name(),
                        v.message
                    ))
                })?;
            // Label values never reject the message: they are sanitized.
            let topic = m.get("topic").and_then(|t| serde_json::to_value(t).ok());
            let labels = self
                .config
                .labels
                .iter()
                .map(|(name, source)| {
                    let value = pnex_core::MetricLabelSource::parse(source)
                        .map(|s| s.resolve(topic.as_ref(), payload_json.as_ref()))
                        .unwrap_or_else(|| "unknown".into());
                    (name.clone(), value)
                })
                .collect();
            (values, labels)
        };
        let series_name = |field: &Option<String>| match field {
            Some(f) => pnex_core::etl_metric_name(&format!("{}_{f}", self.config.metric_name)),
            None => pnex_core::etl_metric_name(&self.config.metric_name),
        };

        // 2) Remote-write borné (timeout 10 s côté client) et annulable.
        // Org O2 vide (provisioning pas encore passé) : écriture sautée,
        // pipeline vivant — la reprojection post-provisioning comblera.
        if !self.tuples.admit(&labels, self.name()) {
            self.fan_out_one(Envelope { port: 0, msg }, cancel).await
        } else if self.config.pnex_o2_org.trim().is_empty() {
            log::warn!(
                "pnex-metric [{}] : O2 org not provisioned — {} series NOT written",
                self.name(),
                values.len()
            );
            self.fan_out_one(Envelope { port: 0, msg }, cancel).await
        } else {
            let org = self.config.pnex_o2_org.clone();
            let virtual_device = format!("flow_{}", self.config.pnex_flow_id);
            let ts_ms = chrono::Utc::now().timestamp_millis();
            let series = values
                .iter()
                .map(|(field, value)| {
                    crate::o2::etl_series_labelled(
                        series_name(field),
                        virtual_device.clone(),
                        *value,
                        ts_ms,
                        &labels,
                    )
                })
                .collect::<Vec<_>>();
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
                "pnex-metric [{}] : {} series written to {org}",
                self.name(),
                values.len()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_tuples_are_capped_but_known_ones_pass() {
        let tuples = LabelTuples::default();
        let set = |v: usize| BTreeMap::from([("entity".to_string(), v.to_string())]);
        for i in 0..pnex_core::METRIC_LABEL_TUPLES_MAX {
            assert!(tuples.admit(&set(i), "n"));
        }
        assert!(!tuples.admit(&set(9_999), "n"));
        assert!(tuples.admit(&set(3), "n"));
        assert!(!tuples.admit(&set(10_000), "n"));
    }
}
