//! Cartes de régulation mixtes (`pnex-reg-tt-heat` / `pnex-reg-tt-cool` /
//! `pnex-reg-pid`) — nœuds **passifs** : voir la doc du crate. La
//! validation au build reconstruit le contrat typé `pnex_core` et appelle
//! la vraie `validate_graph` (même source de vérité que le backend et
//! l'éditeur wasm).

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

use pnex_core::{FlowGraph, FlowNode, FlowNodeKind, RegPidConfig, RegTtConfig, SafeState};

fn default_cycle_time_secs() -> u32 {
    10
}
fn default_min_on_secs() -> u32 {
    5
}
fn default_min_off_secs() -> u32 {
    5
}
fn default_sample_ms() -> u32 {
    5_000
}
fn default_data_timeout_secs() -> u32 {
    30
}

/// Sens de la carte — porté par le **type Node-RED** (`pnex-reg-tt-heat`…),
/// jamais par un champ de config : une faute de sens est impossible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegKind {
    TtHeat,
    TtCool,
    Pid,
}

/// Config red d'une carte — struct plat miroir de l'entrée projetée (les
/// champs TT/PID hors kind sont présents mais ignorés ; même philosophie
/// que le `ControlSpec` fil).
#[derive(Debug, Clone, Deserialize)]
struct RegNodeConfig {
    device_id: String,
    sensor_pin: String,
    actuator_pin: String,
    setpoint: f64,
    #[serde(default)]
    deadband: f64,
    #[serde(default)]
    kp: f64,
    #[serde(default)]
    ki: f64,
    #[serde(default)]
    kd: f64,
    #[serde(default = "default_cycle_time_secs")]
    cycle_time_secs: u32,
    #[serde(default = "default_min_on_secs")]
    min_on_secs: u32,
    #[serde(default = "default_min_off_secs")]
    min_off_secs: u32,
    #[serde(default = "default_sample_ms")]
    sample_ms: u32,
    #[serde(default = "default_data_timeout_secs")]
    data_timeout_secs: u32,
    #[serde(default)]
    safe_state: SafeState,
    // Estampillé par la projection au deploy (traçabilité + détection
    // d'artefact périmé).
    #[serde(default)]
    pnex_node_id: String,
    #[serde(default)]
    pnex_flow_id: i64,
}

impl RegNodeConfig {
    /// Reconstruit le contrat typé pnex-core et passe la **vraie**
    /// `validate_graph` (règles reg + unicité actuateur triviallement vraie
    /// sur un graphe mono-nœud — l'unicité cross-flows reste garantie par le
    /// sync backend).
    fn validate(&self, kind: RegKind) -> Result<()> {
        if self.pnex_flow_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "{} : pnex_flow_id absent de l'artefact (redéployer le flow)",
                kind.type_name()
            ))
            .into());
        }
        let config_kind = match kind {
            RegKind::TtHeat => FlowNodeKind::RegTtHeat {
                config: RegTtConfig {
                    device_id: self.device_id.clone(),
                    sensor_pin: self.sensor_pin.clone(),
                    actuator_pin: self.actuator_pin.clone(),
                    setpoint: self.setpoint,
                    deadband: self.deadband,
                    min_on_secs: self.min_on_secs,
                    min_off_secs: self.min_off_secs,
                    sample_ms: self.sample_ms,
                    data_timeout_secs: self.data_timeout_secs,
                    safe_state: self.safe_state,
                },
            },
            RegKind::TtCool => FlowNodeKind::RegTtCool {
                config: RegTtConfig {
                    device_id: self.device_id.clone(),
                    sensor_pin: self.sensor_pin.clone(),
                    actuator_pin: self.actuator_pin.clone(),
                    setpoint: self.setpoint,
                    deadband: self.deadband,
                    min_on_secs: self.min_on_secs,
                    min_off_secs: self.min_off_secs,
                    sample_ms: self.sample_ms,
                    data_timeout_secs: self.data_timeout_secs,
                    safe_state: self.safe_state,
                },
            },
            RegKind::Pid => FlowNodeKind::RegPid {
                config: RegPidConfig {
                    device_id: self.device_id.clone(),
                    sensor_pin: self.sensor_pin.clone(),
                    actuator_pin: self.actuator_pin.clone(),
                    setpoint: self.setpoint,
                    kp: self.kp,
                    ki: self.ki,
                    kd: self.kd,
                    cycle_time_secs: self.cycle_time_secs,
                    sample_ms: self.sample_ms,
                    data_timeout_secs: self.data_timeout_secs,
                    safe_state: self.safe_state,
                },
            },
        };
        let node = FlowNode {
            id: if self.pnex_node_id.is_empty() {
                "reg".to_string()
            } else {
                self.pnex_node_id.clone()
            },
            name: None,
            position: None,
            outputs: vec![],
            inputs: vec![],
            kind: config_kind,
        };
        let violations = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] });
        if violations.is_empty() {
            Ok(())
        } else {
            let messages: Vec<String> = violations.iter().map(|v| v.message.clone()).collect();
            Err(EdgelinkError::BadFlowsJson(format!(
                "{} : config invalide : {}",
                kind.type_name(),
                messages.join(" ; ")
            ))
            .into())
        }
    }
}

impl RegKind {
    fn type_name(self) -> &'static str {
        match self {
            RegKind::TtHeat => "pnex-reg-tt-heat",
            RegKind::TtCool => "pnex-reg-tt-cool",
            RegKind::Pid => "pnex-reg-pid",
        }
    }
}

/// Déclare un type de carte passif + son build (le struct porte la config
/// validée ; la boucle `run()` consomme et jette).
macro_rules! reg_node {
    ($type_name:literal, $struct_name:ident, $kind:expr) => {
        #[flow_node($type_name, red_name = $type_name)]
        struct $struct_name {
            base: BaseFlowNodeState,
            #[allow(dead_code)]
            config: RegNodeConfig,
        }

        impl $struct_name {
            fn build(
                _flow: &Flow,
                base_node: BaseFlowNodeState,
                config: &RedFlowNodeConfig,
                _options: Option<&config::Config>,
            ) -> Result<Box<dyn FlowNodeBehavior>> {
                let cfg = RegNodeConfig::deserialize(&config.rest).map_err(|e| {
                    EdgelinkError::BadFlowsJson(format!("{} : config invalide : {e}", $type_name))
                })?;
                cfg.validate($kind)?;
                Ok(Box::new($struct_name {
                    base: base_node,
                    config: cfg,
                }))
            }
        }

        #[async_trait]
        impl FlowNodeBehavior for $struct_name {
            fn get_base(&self) -> &BaseFlowNodeState {
                &self.base
            }

            async fn run(self: Arc<Self>, stop_token: CancellationToken) {
                // Nœud passif : la régulation vit sur le device (D13/D17).
                // Consommer et jeter évite l'engorgement du tampon d'entrée
                // si un producteur est câblé en amont.
                while !stop_token.is_cancelled() {
                    with_uow(
                        self.as_ref(),
                        stop_token.child_token(),
                        |node: &$struct_name, msg: MsgHandle| async move {
                            log::trace!(
                                "{} [{}] : message ignoré (carte d'authoring, pas de fan-out)",
                                $type_name,
                                node.name()
                            );
                            drop(msg);
                            Ok(())
                        },
                    )
                    .await;
                }
            }
        }
    };
}

reg_node!("pnex-reg-tt-heat", PnexRegTtHeatNode, RegKind::TtHeat);
reg_node!("pnex-reg-tt-cool", PnexRegTtCoolNode, RegKind::TtCool);
reg_node!("pnex-reg-pid", PnexRegPidNode, RegKind::Pid);
