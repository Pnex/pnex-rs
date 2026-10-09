//! Nœud custom EdgeLinkd `pnex-coolprop` — propriétés thermophysiques
//! in-process via [`pnex_coolprop`] (CoolProp v8.0.0 liée statiquement).
//!
//! Garde-fous :
//! - la config est validée **au build du nœud** (spec non vide, entrées et
//!   sorties résolues par noms CoolProp) — un graphe invalide échoue au
//!   pré-flight `--check` avant tout deploy ;
//! - le `msg` entrant suit le contrat device/calc (objet `clé → numérique`,
//!   [`pnex_core::numeric_map_from_payload`]) — rejet sans panic ;
//! - aucun secret, aucune lecture d'env : tout vient de la config du graphe
//!   (la spec du mélange est figée à l'édition — D6).
//!
//! État process-global CoolProp : chaque exécution fait des appels sous le
//! mutex global du wrapper — sérialisation assumée, coût négligeable au
//! rythme des flows ETL (pas de handle AbstractState statique).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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

/// Point d'ancrage référencé par le binaire `pnex-flow-runtime` : garantit que
/// l'édition de liens conserve les soumissions `inventory` de ce crate.
pub fn registered() {}

#[derive(Debug, Deserialize)]
struct PnexCoolPropNodeConfig {
    fluid_spec: String,
    input1: String,
    input2: String,
    v1_key: String,
    v2_key: String,
    outputs: Vec<String>,
    #[serde(default)]
    include_phase: bool,
    /// Input units (catalogue ids, empty = SI — legacy artifacts).
    #[serde(default)]
    unit1: String,
    #[serde(default)]
    unit2: String,
    /// Unit per output id (missing = SI).
    #[serde(default)]
    output_units: HashMap<String, String>,
}

/// Last SI value received on each input (named anchors deliver the two
/// inputs in separate messages).
#[derive(Debug, Default)]
struct Latch {
    v1: Option<f64>,
    v2: Option<f64>,
}

#[derive(Debug)]
#[flow_node("pnex-coolprop", red_name = "pnex-coolprop")]
struct PnexCoolPropNode {
    base: BaseFlowNodeState,
    config: PnexCoolPropNodeConfig,
    latch: Mutex<Latch>,
}

/// Numeric value of a scalar payload (booleans → 1/0, same contract as
/// `numeric_map_from_payload`).
fn scalar_of(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

impl PnexCoolPropNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = PnexCoolPropNodeConfig::deserialize(&config.rest).map_err(|e| {
            EdgelinkError::BadFlowsJson(format!("pnex-coolprop : config invalide : {e}"))
        })?;

        // Contrats au build : noms CoolProp résolus (fail-fast au --check).
        if cfg.fluid_spec.trim().is_empty() {
            return Err(
                EdgelinkError::BadFlowsJson("pnex-coolprop : fluid_spec requis".into()).into(),
            );
        }
        for f in [&cfg.input1, &cfg.input2] {
            if pnex_coolprop::get_param_index(f).is_err() {
                return Err(EdgelinkError::BadFlowsJson(format!(
                    "pnex-coolprop : propriété d'entrée inconnue : {f:?}"
                ))
                .into());
            }
        }
        for o in &cfg.outputs {
            // Derived catalogue quantities are computed by the node itself.
            let derived = pnex_core::thermo_quantity(o)
                .is_some_and(|q| q.derivation != pnex_core::ThermoDerivation::Direct);
            if !derived && pnex_coolprop::get_param_index(o).is_err() {
                return Err(EdgelinkError::BadFlowsJson(format!(
                    "pnex-coolprop : sortie inconnue : {o:?}"
                ))
                .into());
            }
        }

        Ok(Box::new(PnexCoolPropNode {
            base: base_node,
            config: cfg,
            latch: Mutex::new(Latch::default()),
        }))
    }

    fn reject(&self, what: impl std::fmt::Display) -> EdgelinkError {
        EdgelinkError::InvalidOperation(format!("pnex-coolprop [{}] : {what}", self.name()))
    }

    /// Latches the input values carried by one message: a scalar on a named
    /// anchor (`topic` = input key, stamped at deploy) or an object payload
    /// holding one or both keys. Values are stored in SI.
    fn latch_inputs(
        &self,
        topic: Option<&str>,
        payload: Option<&serde_json::Value>,
    ) -> Result<Option<(f64, f64)>> {
        let c = &self.config;
        let mut got: Vec<(bool, f64)> = Vec::new();
        match (topic, payload) {
            (Some(t), Some(p)) if (t == c.v1_key || t == c.v2_key) && scalar_of(p).is_some() => {
                got.push((t == c.v1_key, scalar_of(p).unwrap_or(f64::NAN)));
            }
            (_, Some(serde_json::Value::Object(map))) => {
                for (first, key) in [(true, &c.v1_key), (false, &c.v2_key)] {
                    if let Some(v) = map.get(key) {
                        let v = scalar_of(v).ok_or_else(|| {
                            self.reject(format!("non-numeric value for key \"{key}\""))
                        })?;
                        got.push((first, v));
                    }
                }
                if got.is_empty() {
                    return Err(self
                        .reject(format!(
                            "payload has neither \"{}\" nor \"{}\"",
                            c.v1_key, c.v2_key
                        ))
                        .into());
                }
            }
            _ => {
                return Err(self
                    .reject(format!(
                        "expected a number on input \"{}\" / \"{}\" or an object holding them",
                        c.v1_key, c.v2_key
                    ))
                    .into());
            }
        }
        let mut latch = self.latch.lock().expect("coolprop latch");
        for (first, v) in got {
            if first {
                latch.v1 = Some(pnex_core::thermo_to_si(&c.input1, &c.unit1, v));
            } else {
                latch.v2 = Some(pnex_core::thermo_to_si(&c.input2, &c.unit2, v));
            }
        }
        Ok(latch.v1.zip(latch.v2))
    }

    fn props(&self, output: &str, v1: f64, v2: f64) -> std::result::Result<f64, String> {
        let c = &self.config;
        pnex_coolprop::props_si(output, &c.input1, v1, &c.input2, v2, &c.fluid_spec)
            .map_err(|e| e.0)
    }

    /// One output in SI: plain PropsSI, or a saturation-derived quantity
    /// evaluated at the state pressure.
    fn output_si(&self, id: &str, v1: f64, v2: f64) -> std::result::Result<f64, String> {
        use pnex_core::ThermoDerivation as D;
        let derivation = pnex_core::thermo_quantity(id)
            .map(|q| q.derivation)
            .unwrap_or(D::Direct);
        if derivation == D::Direct {
            return self.props(id, v1, v2);
        }
        let spec = &self.config.fluid_spec;
        let p = self.props("P", v1, v2)?;
        let sat = |quality: f64| {
            pnex_coolprop::props_si("T", "P", p, "Q", quality, spec).map_err(|e| e.0)
        };
        match derivation {
            D::DewTemperature => sat(1.0),
            D::BubbleTemperature => sat(0.0),
            D::Superheat => Ok(self.props("T", v1, v2)? - sat(1.0)?),
            D::Subcooling => Ok(sat(0.0)? - self.props("T", v1, v2)?),
            D::Direct => unreachable!(),
        }
    }

    /// One incoming message → latch the inputs → compute the outputs once
    /// both inputs are known. Port 0 = object {output: value, …} (+ "phase"),
    /// then one scalar port per output, then the phase port.
    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        log::debug!("pnex-coolprop [{}] : réception d'un message", self.name());

        // 1) Boundary: topic + payload, outside any lock.
        let (topic, payload) = {
            let m = msg.read().await;
            let payload = match m.get("payload").cloned() {
                Some(v) => Some(serde_json::to_value(&v).map_err(|e| {
                    EdgelinkError::InvalidOperation(format!(
                        "pnex-coolprop : payload non sérialisable : {e}"
                    ))
                })?),
                None => None,
            };
            let topic = m.get("topic").and_then(|v| v.as_str()).map(str::to_string);
            (topic, payload)
        };
        let Some((v1, v2)) = self.latch_inputs(topic.as_deref(), payload.as_ref())? else {
            log::debug!(
                "pnex-coolprop [{}] : waiting for the other input",
                self.name()
            );
            return Ok(());
        };

        // 2) Outputs. A single failing output (e.g. transport property not
        // available for a mixture) yields null; all failing rejects the msg.
        let mut payload_obj = serde_json::Map::new();
        let mut scalars: Vec<Option<f64>> = Vec::with_capacity(self.config.outputs.len());
        let mut last_err = None;
        for name in &self.config.outputs {
            match self.output_si(name, v1, v2) {
                Ok(si) if si.is_finite() => {
                    let unit = self
                        .config
                        .output_units
                        .get(name)
                        .map(String::as_str)
                        .unwrap_or("");
                    let v = pnex_core::thermo_from_si(name, unit, si);
                    payload_obj.insert(name.clone(), serde_json::json!(v));
                    scalars.push(Some(v));
                }
                Ok(_) => {
                    payload_obj.insert(name.clone(), serde_json::Value::Null);
                    scalars.push(None);
                }
                Err(e) => {
                    log::warn!("pnex-coolprop [{}] : {name} : {e}", self.name());
                    payload_obj.insert(name.clone(), serde_json::Value::Null);
                    scalars.push(None);
                    last_err = Some(e);
                }
            }
        }
        if let Some(e) = last_err.filter(|_| scalars.iter().all(Option::is_none)) {
            return Err(self.reject(e).into());
        }
        let phase = if self.config.include_phase {
            let c = &self.config;
            let phase = pnex_coolprop::phase_si(&c.input1, v1, &c.input2, v2, &c.fluid_spec)
                .map_err(|e| {
                    log::warn!("pnex-coolprop [{}] : {}", self.name(), e.0);
                    self.reject(e.0)
                })?;
            payload_obj.insert("phase".to_string(), serde_json::json!(phase));
            Some(phase)
        } else {
            None
        };

        // 3) Fan-out. Ports beyond the artifact's wires array (outputs not
        // wired) are skipped.
        let to_variant = |v: serde_json::Value| -> Result<Variant> {
            serde_json::from_value(v).map_err(|e| {
                EdgelinkError::InvalidOperation(format!(
                    "pnex-coolprop : résultat non convertible : {e}"
                ))
                .into()
            })
        };
        let port_count = self.get_base().ports.len();
        let mut envelopes: smallvec::SmallVec<[Envelope; 4]> = smallvec::SmallVec::new();
        {
            let mut m = msg.write().await;
            m.set(
                "payload".to_string(),
                to_variant(serde_json::Value::Object(payload_obj))?,
            );
            // The stamped input topic must not leak downstream (merge keys).
            m.remove("topic");
        }
        envelopes.push(Envelope { port: 0, msg });
        let scalar_ports = self
            .config
            .outputs
            .iter()
            .zip(scalars)
            .enumerate()
            .filter_map(|(i, (name, v))| v.map(|v| (1 + i, name.clone(), serde_json::json!(v))));
        let phase_port = phase.map(|p| {
            (
                1 + self.config.outputs.len(),
                "phase".to_string(),
                serde_json::json!(p),
            )
        });
        for (port, topic, value) in scalar_ports.chain(phase_port) {
            if port >= port_count {
                continue;
            }
            let mut m = Msg::default();
            m.set("payload".to_string(), to_variant(value)?);
            m.set("topic".to_string(), Variant::String(topic));
            envelopes.push(Envelope {
                port,
                msg: MsgHandle::new(m),
            });
        }
        self.fan_out_many(envelopes, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for PnexCoolPropNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &PnexCoolPropNode, msg: MsgHandle| async move {
                    match node.execute(msg.clone(), cancel.child_token()).await {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            log::warn!("pnex-coolprop [{}] : message rejeté : {e}", node.name());
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
    use edgelink_core::runtime::registry::RegistryBuilder;

    #[test]
    fn node_enregistre_dans_le_registre() {
        let reg = RegistryBuilder::default().build().expect("registre");
        let meta = reg
            .get("pnex-coolprop")
            .expect("nœud pnex-coolprop absent du registre");
        assert_eq!(meta.type_, "pnex-coolprop");
    }

    #[test]
    fn registered_ne_panique_pas() {
        registered();
    }
}
