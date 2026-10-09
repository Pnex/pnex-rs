//! Nœud `pnex-device-read` — lit les **dernières valeurs** des pins d'**un
//! seul device** dans OpenObserve (`last_over_time` sur la même série que
//! l'ingestion).
//!
//! Read path: when the supervisor injects `VALKEY_URL`, values come from the
//! live last-value cache (single `MGET`, freshness = exact age of the last
//! received sample) — the OpenObserve loop below is the fallback when the
//! cache is disabled or failing.
//!
//! Garde-fous (école `pnex-device` historique) :
//! - config validée **au build** (slug device, pins non vides, fenêtre
//!   bornée 1..=3600 s, org estampillée) — un graphe invalide est rejeté
//!   au déploiement ;
//! - l'org OpenObserve vient de l'artefact (`pnex_o2_org`), les creds de
//!   l'env du runtime — jamais de secret dans flows.json ;
//! - a read with no fresh sample within the window emits **nothing** on
//!   the pin port (never a `null`, never an invented zero) + warn: an
//!   offline device must not produce readings downstream (O14).
//!
//! Sorties : un port par pin (**valeur brute**, only when fresh — no
//! message otherwise) + un port final « nom du
//! device » (payload = slug du device) — nommage explicite du device pour
//! l'aval (templates, labels), sans convention de payload.

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
struct DeviceReadNodeConfig {
    device_id: String,
    #[serde(default)]
    pins: Vec<String>,
    #[serde(default = "default_window_secs")]
    window_secs: f64,
    /// Estampillé par la projection au deploy (`FlowArtifactMeta.org_id`).
    #[serde(default)]
    pnex_org_id: i64,
    /// Identifiant O2 **réel** (`openobserve_orgs.o2_org`), estampillé par la
    /// projection. Vide = org pas encore provisionnée — les lectures sont
    /// sautées (warn, payload vide) sans casser le pipeline ; la reprojection
    /// qui suit le provisioning comble le champ.
    #[serde(default)]
    pnex_o2_org: String,
}

#[flow_node("pnex-device-read", red_name = "pnex-device-read")]
struct DeviceReadNode {
    base: BaseFlowNodeState,
    config: DeviceReadNodeConfig,
    o2: O2Client,
    /// Live last-value cache reader — `None` when `VALKEY_URL` is absent
    /// (legacy OpenObserve-only path).
    valkey: Option<crate::valkey::ValkeyLastClient>,
}

impl DeviceReadNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = DeviceReadNodeConfig::deserialize(&config.rest).map_err(|e| {
            EdgelinkError::BadFlowsJson(format!("pnex-device-read : config invalide : {e}"))
        })?;

        // Contrat typé au build (miroir validate_graph côté backend/wasm).
        if !pnex_core::valid_device_label(&cfg.device_id) {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "pnex-device-read : device « {} » invalide (slug requis)",
                cfg.device_id
            ))
            .into());
        }
        if cfg.pins.is_empty() {
            return Err(EdgelinkError::BadFlowsJson(
                "pnex-device-read : aucune pin sélectionnée".into(),
            )
            .into());
        }
        for pin in &cfg.pins {
            if pin.trim().is_empty() {
                return Err(EdgelinkError::BadFlowsJson(format!(
                    "pnex-device-read : pin vide pour « {} »",
                    cfg.device_id
                ))
                .into());
            }
        }
        if !(cfg.window_secs.is_finite() && (1.0..=3600.0).contains(&cfg.window_secs)) {
            return Err(EdgelinkError::BadFlowsJson(
                "pnex-device-read : window_secs hors bornes (1..=3600)".into(),
            )
            .into());
        }
        if cfg.pnex_org_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(
                "pnex-device-read : pnex_org_id absent de l'artefact (redéployer le flow)".into(),
            )
            .into());
        }
        // org O2 vide : PAS de fail-loud au build — état transitoire (avant
        // provisioning), pas une config invalide. Dégradation à l'exécution.

        let o2 = O2Client::from_env()?;
        let valkey = crate::valkey::ValkeyLastClient::from_env_opt()?;
        Ok(Box::new(DeviceReadNode {
            base: base_node,
            config: cfg,
            o2,
            valkey,
        }))
    }

    /// Un message entrant (déclencheur) → lectures O2 séquentielles →
    /// un message par pin sur son port + le port « tout » combiné.
    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let window = Duration::from_secs_f64(self.config.window_secs);
        let deadline = Duration::from_secs_f64(self.config.window_secs + 5.0);

        let mut values: BTreeMap<String, f64> = BTreeMap::new();
        // Live last-value cache first (single MGET, bounded at 1 s): the
        // freshness window applies to the exact age of the last received
        // sample, not to the data age in OpenObserve (ingestion batcher
        // latency). Cache absent or failing -> OpenObserve loop below.
        let mut cache_served = false;
        if let Some(vk) = &self.valkey {
            let now_ms = chrono::Utc::now().timestamp_millis();
            tokio::select! {
                res = vk.mget_last(
                    self.config.pnex_org_id,
                    &self.config.device_id,
                    &self.config.pins,
                ) => match res {
                    Ok(samples) => {
                        cache_served = true;
                        for (pin, sample) in self.config.pins.iter().zip(samples) {
                            match pnex_core::last_cache::resolve(sample, now_ms, self.config.window_secs) {
                                Some(v) => {
                                    values.insert(
                                        pnex_core::device_payload_key(&self.config.device_id, pin),
                                        v,
                                    );
                                }
                                None => {
                                    log::warn!(
                                        "pnex-device-read [{}] : no fresh cached value for {}:{} within {} s",
                                        self.name(), self.config.device_id, pin, self.config.window_secs
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "pnex-device-read [{}] : valkey cache read failed ({e}) — falling back to OpenObserve",
                            self.name()
                        );
                    }
                },
                _ = cancel.cancelled() => return Err(EdgelinkError::TaskCancelled.into()),
            }
        }
        if !cache_served && self.config.pnex_o2_org.trim().is_empty() {
            log::warn!(
                "pnex-device-read [{}] : org O2 non provisionnée — lecture(s) sautée(s)",
                self.name()
            );
        } else {
            for pin in &self.config.pins {
                let metric = pnex_core::normalize_measurement_name(pin);
                tokio::select! {
                    res = tokio::time::timeout(deadline, self.o2.query_last(
                        &self.config.pnex_o2_org,
                        &metric,
                        &self.config.device_id,
                        self.config.window_secs,
                    )) => match res {
                        Ok(Ok(Some(sample))) => {
                            // Same freshness contract as the cache path: a
                            // sample older than the window is never emitted.
                            let now_ms = chrono::Utc::now().timestamp_millis();
                            match fresh_o2_value(sample, now_ms, self.config.window_secs) {
                                Some(value) => {
                                    values.insert(
                                        pnex_core::device_payload_key(&self.config.device_id, pin),
                                        value,
                                    );
                                }
                                None => {
                                    log::warn!(
                                        "pnex-device-read [{}] : stale OpenObserve sample for {}:{} (older than {} s) — dropped",
                                        self.name(), self.config.device_id, pin, self.config.window_secs
                                    );
                                }
                            }
                        }
                        Ok(Ok(None)) => {
                            log::warn!(
                                "pnex-device-read [{}] : aucune donnée dans la fenêtre {} s pour {}:{}",
                                self.name(), window.as_secs(), self.config.device_id, pin
                            );
                        }
                        Ok(Err(e)) => {
                            log::warn!(
                                "pnex-device-read [{}] : lecture {}:{} échouée : {e}",
                                self.name(), self.config.device_id, pin
                            );
                        }
                        Err(_) => {
                            log::warn!(
                                "pnex-device-read [{}] : lecture {}:{} interrompue (timeout)",
                                self.name(), self.config.device_id, pin
                            );
                        }
                    },
                    _ = cancel.cancelled() => return Err(EdgelinkError::TaskCancelled.into()),
                }
            }
        }
        log::debug!(
            "pnex-device-read [{}] : {} lecture(s) résolue(s)",
            self.name(),
            values.len()
        );

        let n = self.config.pins.len();
        // One port per pin, carrying the raw value — ONLY when a fresh value
        // exists. A missing or stale pin (device offline, no sample within
        // the window) emits NOTHING on its port: a `null` payload used to be
        // sent, which downstream code coerced to falsy (`not None` in
        // Starlark = True) and turned an offline device into phantom
        // readings (O14: "Button pressed" notifications with the device
        // offline). Each port gets a deep clone of the message: the original
        // handle is shared by every fan-out branch.
        for (port, value) in fresh_pin_outputs(&self.config.device_id, &self.config.pins, &values) {
            let branch = msg.deep_clone(false).await;
            {
                let mut m = branch.write().await;
                m.set("payload".to_string(), Variant::from(value));
            }
            self.fan_out_one(Envelope { port, msg: branch }, cancel.clone())
                .await?;
        }

        // Port final « nom du device » : le slug du device en payload —
        // nommage explicite (templates, labels), sans convention de payload.
        {
            let mut m = msg.write().await;
            m.set(
                "payload".to_string(),
                Variant::from(self.config.device_id.clone()),
            );
        }
        self.fan_out_one(Envelope { port: n, msg }, cancel).await
    }
}

/// Freshness gate of the OpenObserve fallback, mirroring the cache path
/// (`pnex_core::last_cache::resolve`, inclusive window, clock skew = fresh).
/// `last_over_time(...[w s])` already bounds the range, this is the
/// defensive second check on the returned sample timestamp.
fn fresh_o2_value(sample: crate::o2::LastSample, now_ms: i64, window_secs: f64) -> Option<f64> {
    let (v, ts_ms) = sample;
    pnex_core::last_cache::resolve(
        Some(pnex_core::CachedSample { v, ts_ms }),
        now_ms,
        window_secs,
    )
}

/// `(port, value)` pairs to emit: one per pin that has a fresh value, in
/// pin order (port index = pin index, stable). Pins without a fresh value
/// are skipped — never a `null`, never an invented zero.
fn fresh_pin_outputs(
    device_id: &str,
    pins: &[String],
    values: &BTreeMap<String, f64>,
) -> Vec<(usize, f64)> {
    pins.iter()
        .enumerate()
        .filter_map(|(i, pin)| {
            values
                .get(&pnex_core::device_payload_key(device_id, pin))
                .map(|v| (i, *v))
        })
        .collect()
}

#[async_trait]
impl FlowNodeBehavior for DeviceReadNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &DeviceReadNode, msg: MsgHandle| async move {
                    match node.execute(msg.clone(), cancel.child_token()).await {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            log::warn!("pnex-device-read [{}] : message rejeté : {e}", node.name());
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

    const NOW_MS: i64 = 1_789_000_000_000;

    #[test]
    fn o2_sample_within_window_is_fresh() {
        assert_eq!(
            fresh_o2_value((1.0, NOW_MS - 4_000), NOW_MS, 5.0),
            Some(1.0)
        );
    }

    #[test]
    fn o2_sample_older_than_window_is_dropped() {
        assert_eq!(fresh_o2_value((0.0, NOW_MS - 600_000), NOW_MS, 5.0), None);
    }

    #[test]
    fn o2_window_boundary_is_inclusive() {
        assert_eq!(
            fresh_o2_value((0.0, NOW_MS - 5_000), NOW_MS, 5.0),
            Some(0.0)
        );
    }

    #[test]
    fn stale_cached_sample_is_never_resolved() {
        // O14: device offline for minutes, last cached D3 = 0 (pressed,
        // active-low) still inside the 3900 s GC TTL — must NOT be emitted.
        let stale = pnex_core::CachedSample {
            v: 0.0,
            ts_ms: NOW_MS - 600_000,
        };
        assert_eq!(
            pnex_core::last_cache::resolve(Some(stale), NOW_MS, 5.0),
            None
        );
    }

    #[test]
    fn offline_device_emits_nothing_on_pin_ports() {
        let pins = vec!["D3".to_string()];
        let values = BTreeMap::new();
        assert!(fresh_pin_outputs("proud-ibex", &pins, &values).is_empty());
    }

    #[test]
    fn only_fresh_pins_are_emitted_on_their_own_port() {
        let pins = vec!["D1".to_string(), "D3".to_string(), "A0".to_string()];
        let mut values = BTreeMap::new();
        values.insert(pnex_core::device_payload_key("dev", "D3"), 0.0);
        values.insert(pnex_core::device_payload_key("dev", "A0"), 512.0);
        assert_eq!(
            fresh_pin_outputs("dev", &pins, &values),
            vec![(1, 0.0), (2, 512.0)]
        );
    }
}
