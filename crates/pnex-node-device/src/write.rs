//! Nœud `pnex-device-write` — écrit les pins output d'un seul device
//! (digital 1/0, pwm duty 0..=100). Le runtime n'a jamais d'accès direct au
//! WS device : le downlink transite par la route interne backend
//! `/internal/flow/device-write` (école `/internal/notify/deliver`), token
//! partagé injecté dans l'env du runtime par le superviseur.
//!
//! - build fail-loud si les coordonnées d'écriture sont absentes de l'env
//!   (`PNEX_FLOW_WRITE_URL`/`_TOKEN`) — le préflight `--check` du deploy
//!   échoue explicitement (école pnex-notify) ;
//! - incoming payload = a `{pin: value}` map (batch — only the configured
//!   pins present in the map are sent), or a **scalar** routed by
//!   `msg.topic` when the wire was drawn on a pin's input anchor (the deploy
//!   projection stamps the topic; one unit write per message), or a
//!   **scalar** on a single-pin node (direct wiring from a device-read
//!   per-pin port); anything else is dropped with a warn (passthrough).
//! - écriture lenient : un pin en erreur = warn + continue (jamais de
//!   crash-loop) ; le message passe en sortie (passthrough).
//! - Sorties : port 0 = message entrant (passthrough), dernier port =
//!   « nom du device » (payload = slug du device) — nommage explicite du
//!   device pour l'aval, sans convention de payload.

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
struct DeviceWriteNodeConfig {
    device_id: String,
    #[serde(default)]
    pins: Vec<String>,
    /// Estampillé par la projection au deploy (`FlowArtifactMeta.org_id`) —
    /// passé tel quel à la route interne (elle résout le device dans l'org).
    #[serde(default)]
    pnex_org_id: i64,
}

#[flow_node("pnex-device-write", red_name = "pnex-device-write")]
struct DeviceWriteNode {
    base: BaseFlowNodeState,
    config: DeviceWriteNodeConfig,
    http: reqwest::Client,
    url: String,
    token: String,
    org_id: i64,
}

impl DeviceWriteNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = DeviceWriteNodeConfig::deserialize(&config.rest).map_err(|e| {
            EdgelinkError::BadFlowsJson(format!("pnex-device-write : config invalide : {e}"))
        })?;
        if !pnex_core::valid_device_label(&cfg.device_id) {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "pnex-device-write : device « {} » invalide (slug requis)",
                cfg.device_id
            ))
            .into());
        }
        if cfg.pins.is_empty() {
            return Err(EdgelinkError::BadFlowsJson(
                "pnex-device-write : aucune pin sélectionnée".into(),
            )
            .into());
        }
        for pin in &cfg.pins {
            if pin.trim().is_empty() {
                return Err(EdgelinkError::BadFlowsJson(format!(
                    "pnex-device-write : pin vide pour « {} »",
                    cfg.device_id
                ))
                .into());
            }
        }
        if cfg.pnex_org_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(
                "pnex-device-write : pnex_org_id absent de l'artefact (redéployer le flow)".into(),
            )
            .into());
        }
        // Coordonnées injectées par le superviseur (FlowSettings.device_write
        // → apply_runtime_env). Jamais en flows.json — absent ⇒ fail-loud au
        // build : le préflight du deploy échoue explicitement.
        let url = std::env::var("PNEX_FLOW_WRITE_URL").map_err(|_| {
            EdgelinkError::InvalidOperation(
                "pnex-device-write : PNEX_FLOW_WRITE_URL absente de l'environnement \
                 du runtime (runtime_token non configuré côté serveur ?)"
                    .into(),
            )
        })?;
        let token = std::env::var("PNEX_FLOW_WRITE_TOKEN").map_err(|_| {
            EdgelinkError::InvalidOperation(
                "pnex-device-write : PNEX_FLOW_WRITE_TOKEN absente de l'environnement \
                 du runtime"
                    .into(),
            )
        })?;
        let http = reqwest::Client::new();
        Ok(Box::new(DeviceWriteNode {
            base: base_node,
            org_id: cfg.pnex_org_id,
            config: cfg,
            http,
            url,
            token,
        }))
    }
}

impl DeviceWriteNode {
    /// Passthrough fan-out used by every exit path: the incoming message on
    /// port 0 + the configured device slug on the device-name port (last) —
    /// explicit naming for downstream consumers, no payload convention.
    async fn fan_out_done(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let name_branch = msg.deep_clone(false).await;
        {
            let mut m = name_branch.write().await;
            m.set(
                "payload".to_string(),
                Variant::from(self.config.device_id.clone()),
            );
        }
        self.fan_out_one(
            Envelope {
                port: 1,
                msg: name_branch,
            },
            cancel.clone(),
        )
        .await?;
        self.fan_out_one(Envelope { port: 0, msg }, cancel).await
    }

    /// Payload = `{pin: value}` map (batch), or a scalar routed by
    /// `msg.topic` to the configured pin it names (wired input anchor), or a
    /// scalar on a single-pin node — direct wiring from a device-read
    /// per-pin port. Values are filtered on the configured pins and POSTed
    /// to the internal route. Lenient result: pins in error are warned, the
    /// message passes through.
    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        // Payload read (cloned) inside the guard — never a reference beyond
        // this block; the topic travels with it (wired input anchors).
        let (payload_value, topic): (Variant, Option<String>) = {
            let m = msg.read().await;
            let topic = m.get("topic").and_then(|v| v.as_str()).map(str::to_string);
            (m.get("payload").cloned().unwrap_or(Variant::Null), topic)
        };
        // Object payload → batch write (the map wins even when a topic is
        // present). Anything else falls back to the scalar path:
        // - a scalar whose `topic` matches one of the configured pins is
        //   written to that pin — the deploy projection inserts a tagger on
        //   every wire drawn on a declared input anchor, stamping
        //   `msg.topic` with the pin name (one unit write per message);
        // - without a topic, a scalar is acceptable **only with a single
        //   configured pin** (the target is then unambiguous, `{pin: value}`)
        //   — direct wiring from a per-pin port of a device-read.
        // Multi-pin + scalar without topic = ambiguous, nothing is written
        // (warn — a map stays the normal path).
        if payload_value.as_object().is_none() {
            let scalar = match &payload_value {
                Variant::Number(n) => serde_json::Value::Number(n.clone()),
                Variant::String(s) => serde_json::Value::String(s.clone()),
                Variant::Bool(b) => serde_json::Value::Bool(*b),
                Variant::Null => {
                    return self.fan_out_done(msg, cancel).await;
                }
                _ => serde_json::Value::Null,
            };
            if !scalar.is_null() {
                if let Some(topic) = topic.as_deref() {
                    if let Some(pin) = self.config.pins.iter().find(|p| p.as_str() == topic) {
                        let mut single = serde_json::Map::new();
                        single.insert(pin.clone(), scalar);
                        return self.write_values(msg, cancel, single).await;
                    }
                    log::warn!(
                        "pnex-device-write [{}] : payload topic \"{topic}\" matches no \
                         configured pin ({}) — nothing to write",
                        self.name(),
                        self.config.pins.join(", ")
                    );
                    return self.fan_out_done(msg, cancel).await;
                }
                if self.config.pins.len() == 1 {
                    let mut single = serde_json::Map::new();
                    single.insert(self.config.pins[0].clone(), scalar);
                    return self.write_values(msg, cancel, single).await;
                }
            }
            log::warn!(
                "pnex-device-write [{}] : payload non objet{} — rien à écrire",
                self.name(),
                if self.config.pins.len() > 1 {
                    " (N pins configurées : passez une map {pin: valeur})"
                } else {
                    ""
                }
            );
            return self.fan_out_done(msg, cancel).await;
        }
        // Map Variant → JSON une seule fois ; le filtrage sur les pins
        // configurées vit dans `write_values`.
        let obj: serde_json::Map<String, serde_json::Value> =
            match serde_json::to_value(&payload_value) {
                Ok(v) => v.as_object().cloned().unwrap_or_default(),
                Err(e) => {
                    return Err(EdgelinkError::InvalidOperation(format!(
                        "pnex-device-write : payload non sérialisable : {e}"
                    ))
                    .into());
                }
            };
        self.write_values(msg, cancel, obj).await
    }

    /// Un POST batch vers la route interne avec les valeurs filtrées sur les
    /// pins configurées. Résultat lenient : les pins en erreur sont
    /// warnées, le message passe (passthrough).
    async fn write_values(
        &self,
        msg: MsgHandle,
        cancel: CancellationToken,
        map: serde_json::Map<String, serde_json::Value>,
    ) -> Result<()> {
        let mut values = serde_json::Map::new();
        for pin in &self.config.pins {
            let Some(v) = map.get(pin.as_str()) else {
                continue;
            };
            // Conversion Variant → JSON pour la route (une seule fois).
            let json_v = serde_json::to_value(v).map_err(|e| {
                EdgelinkError::InvalidOperation(format!(
                    "pnex-device-write : payload non sérialisable : {e}"
                ))
            })?;
            values.insert(pin.clone(), json_v);
        }
        if values.is_empty() {
            log::warn!(
                "pnex-device-write [{}] : aucune pin configurée présente dans le payload",
                self.name()
            );
            return self.fan_out_done(msg, cancel).await;
        }
        let body = serde_json::json!({
            "org_id": self.org_id,
            "device_id": self.config.device_id,
            "values": values,
        });
        let request = self
            .http
            .post(&self.url)
            .header("x-pnex-flow-token", &self.token)
            // Fencing identity of this worker (D106).
            .header(
                pnex_core::FLOW_WORKER_HEADER,
                pnex_core::flow_worker_fence().unwrap_or_default(),
            )
            .json(&body);
        tokio::select! {
            res = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                request.send(),
            ) => match res {
                Ok(Ok(resp)) => {
                    let status = resp.status();
                    if !status.is_success() {
                        let detail = resp.text().await.unwrap_or_default();
                        log::warn!(
                            "pnex-device-write [{}] : écriture {}:{} échouée : {status} {detail}",
                            self.name(), self.config.device_id,
                            values.keys().cloned().collect::<Vec<_>>().join(",")
                        );
                    } else {
                        match resp.json::<serde_json::Value>().await {
                            Ok(out) => {
                                for r in out.get("results").and_then(|x| x.as_array()).unwrap_or(&vec![]) {
                                    let ok = r.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
                                    let pin = r.get("pin").and_then(|x| x.as_str()).unwrap_or("?");
                                    if !ok {
                                        let err = r.get("err").and_then(|x| x.as_str()).unwrap_or("?");
                                        log::warn!(
                                            "pnex-device-write [{}] : pin {} : {err}",
                                            self.name(), pin
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                log::warn!(
                                    "pnex-device-write [{}] : réponse illisible : {e}",
                                    self.name()
                                );
                            }
                        }
                    }
                }
                Ok(Err(e)) => {
                    log::warn!(
                        "pnex-device-write [{}] : requête échouée : {e}",
                        self.name()
                    );
                }
                Err(_) => {
                    log::warn!(
                        "pnex-device-write [{}] : écriture interrompue (timeout)",
                        self.name()
                    );
                }
            },
            _ = cancel.cancelled() => return Err(EdgelinkError::TaskCancelled.into()),
        }
        self.fan_out_done(msg, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for DeviceWriteNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &DeviceWriteNode, msg: MsgHandle| async move {
                    match node.execute(msg.clone(), cancel.child_token()).await {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            log::warn!(
                                "pnex-device-write [{}] : message rejeté : {e}",
                                node.name()
                            );
                            Err(e)
                        }
                    }
                },
            )
            .await;
        }
    }
}
