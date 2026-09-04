//! Nœud `pnex-device` (Phase 6 ETL) — lit les **dernières valeurs** des pins
//! de un ou plusieurs devices dans OpenObserve (`last_over_time` sur la même
//! série que l'ingestion) et émet un payload objet `clé → valeur`.
//!
//! Garde-fous (mêmes règles que pnex-sql) :
//! - config validée **au build** (lectures non vides, slug device, fenêtre
//!   bornée 1..=3600 s) — un graphe invalide est rejeté au déploiement ;
//! - l'org OpenObserve vient de l'artefact (`pnex_org_id` estampillé au
//!   deploy), les creds de l'env du runtime — jamais de secret dans
//!   flows.json ;
//! - une lecture sans donnée dans la fenêtre = clé **omise** du payload
//!   (jamais de zéro inventé) + warn ; le nœud calc en aval échouera avec
//!   « variable inconnue » si elle lui manque — fail-loud.
//!
//! Sortie : `msg.payload = {"<device>_<pin>": valeur}` — clés calculées par
//! `pnex_core::device_payload_key`, identiques à la prévisualisation de
//! l'éditeur.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

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

fn default_window_secs() -> f64 {
    60.0
}

#[derive(Debug, Deserialize)]
struct DeviceReadCfg {
    device_id: String,
    pin: String,
}

#[derive(Debug, Deserialize)]
struct DeviceNodeConfig {
    reads: Vec<DeviceReadCfg>,
    #[serde(default = "default_window_secs")]
    window_secs: f64,
    /// Estampillé par la projection au deploy (`FlowArtifactMeta.org_id`).
    #[serde(default)]
    pnex_org_id: i64,
}

#[flow_node("pnex-device", red_name = "pnex-device")]
struct PnexDeviceNode {
    base: BaseFlowNodeState,
    config: DeviceNodeConfig,
    o2: O2Client,
}

impl PnexDeviceNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = DeviceNodeConfig::deserialize(&config.rest).map_err(|e| {
            EdgelinkError::BadFlowsJson(format!("pnex-device : config invalide : {e}"))
        })?;

        // Contrat typé au build (miroir validate_graph côté backend/wasm).
        if cfg.reads.is_empty() {
            return Err(EdgelinkError::BadFlowsJson(
                "pnex-device : aucune lecture configurée".into(),
            )
            .into());
        }
        for r in &cfg.reads {
            if !pnex_core::valid_device_label(&r.device_id) {
                return Err(EdgelinkError::BadFlowsJson(format!(
                    "pnex-device : device « {} » invalide (slug requis)",
                    r.device_id
                ))
                .into());
            }
            if r.pin.trim().is_empty() {
                return Err(EdgelinkError::BadFlowsJson(format!(
                    "pnex-device : pin manquant pour « {} »",
                    r.device_id
                ))
                .into());
            }
        }
        if !(cfg.window_secs.is_finite() && (1.0..=3600.0).contains(&cfg.window_secs)) {
            return Err(EdgelinkError::BadFlowsJson(
                "pnex-device : window_secs hors bornes (1..=3600)".into(),
            )
            .into());
        }
        if cfg.pnex_org_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(
                "pnex-device : pnex_org_id absent de l'artefact (redéployer le flow)".into(),
            )
            .into());
        }

        let o2 = O2Client::from_env()?;
        Ok(Box::new(PnexDeviceNode { base: base_node, config: cfg, o2 }))
    }

    /// Un message entrant (déclencheur) → lectures O2 en parallèle → payload
    /// `clé → valeur`. Les lectures passent par la fenêtre de fraîcheur :
    /// une donnée trop vieille est absente, pas zéro.
    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let org = format!("pnex_org_{}", self.config.pnex_org_id);
        let window = Duration::from_secs_f64(self.config.window_secs);
        let deadline = Duration::from_secs_f64(self.config.window_secs + 5.0);

        // Lectures séquentielles bornées globalement (N ≤ quelques dizaines ;
        // la concurrence ferait exploser le budget requêtes O2 à chaque tick).
        let mut values: BTreeMap<String, f64> = BTreeMap::new();
        for read in &self.config.reads {
            let metric = pnex_core::normalize_measurement_name(&read.pin);
            tokio::select! {
                res = tokio::time::timeout(deadline, self.o2.query_last(
                    &org,
                    &metric,
                    &read.device_id,
                    self.config.window_secs,
                )) => match res {
                    Ok(Ok(Some((value, _ts)))) => {
                        values.insert(
                            pnex_core::device_payload_key(&read.device_id, &read.pin),
                            value,
                        );
                    }
                    Ok(Ok(None)) => {
                        log::warn!(
                            "pnex-device [{}] : aucune donnée dans la fenêtre {} s pour {}:{}",
                            self.name(), window.as_secs(), read.device_id, read.pin
                        );
                    }
                    Ok(Err(e)) => {
                        log::warn!(
                            "pnex-device [{}] : lecture {}:{} échouée : {e}",
                            self.name(), read.device_id, read.pin
                        );
                    }
                    Err(_) => {
                        log::warn!(
                            "pnex-device [{}] : lecture {}:{} interrompue (timeout)",
                            self.name(), read.device_id, read.pin
                        );
                    }
                },
                _ = cancel.cancelled() => return Err(EdgelinkError::TaskCancelled.into()),
            }
        }

        log::debug!(
            "pnex-device [{}] : {} lecture(s) résolue(s)",
            self.name(),
            values.len()
        );
        let payload: Variant = serde_json::from_value(serde_json::json!(values)).map_err(|e| {
            EdgelinkError::InvalidOperation(format!("pnex-device : payload non convertible : {e}"))
        })?;
        {
            let mut m = msg.write().await;
            m.set("payload".to_string(), payload);
        }
        self.fan_out_one(Envelope { port: 0, msg }, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for PnexDeviceNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(self.as_ref(), cancel.child_token(), |node: &PnexDeviceNode, msg: MsgHandle| async move {
                match node.execute(msg.clone(), cancel.child_token()).await {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        log::warn!("pnex-device [{}] : message rejeté : {e}", node.name());
                        Err(e)
                    }
                }
            })
            .await;
        }
    }
}
