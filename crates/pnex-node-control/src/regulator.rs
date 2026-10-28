//! Mixed control cards (`pnex-reg-tt-heat` / `pnex-reg-tt-cool` /
//! `pnex-reg-pid`) — **passive** nodes: see the crate doc. The
//! build validation rebuilds the typed `pnex_core` contract and calls
//! the real `validate_graph` (same source of truth as the backend and
//! the wasm editor).

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

/// Card direction — carried by the **Node-RED type** (`pnex-reg-tt-heat`…),
/// never by a config field: a direction mistake is impossible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegKind {
    TtHeat,
    TtCool,
    Pid,
}

/// Red config of a card — flat struct mirroring the projected entry (the
/// TT/PID fields outside the kind are present but ignored; same philosophy
/// as the wire `ControlSpec`).
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
    // Stamped by the projection at deploy (traceability + stale
    // artefact detection).
    #[serde(default)]
    pnex_node_id: String,
    #[serde(default)]
    pnex_flow_id: i64,
}

impl RegNodeConfig {
    /// Rebuilds the typed pnex-core contract and runs the **real**
    /// `validate_graph` (reg rules + actuator uniqueness trivially true
    /// on a single-node graph — cross-flow uniqueness stays guaranteed by the
    /// backend sync).
    fn validate(&self, kind: RegKind) -> Result<()> {
        if self.pnex_flow_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "{}: pnex_flow_id missing from the artefact (redeploy the flow)",
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
                "{}: invalid config: {}",
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

/// Declares a passive card type + its build (the struct carries the validated
/// config; the `run()` loop consumes and drops).
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
                    EdgelinkError::BadFlowsJson(format!("{}: invalid config: {e}", $type_name))
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
                // Passive node: the control loop lives on the device (D13/D17).
                // Consuming and dropping avoids clogging the input buffer
                // if a producer is wired upstream.
                while !stop_token.is_cancelled() {
                    with_uow(
                        self.as_ref(),
                        stop_token.child_token(),
                        |node: &$struct_name, msg: MsgHandle| async move {
                            log::trace!(
                                "{} [{}]: message ignored (authoring card, no fan-out)",
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
