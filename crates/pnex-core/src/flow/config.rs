//! Per-kind node configuration types: default configs, pin/reg extractors
//! and the notify/HTTP-fetch config family.
//!
//! (Extracted verbatim from the former single-file `flow.rs` — comments
//! below are kept as written at the time of the move.)

use super::validate::pin_key;
use super::*;
use crate::proto::SafeState;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 2D position of a node on the editor canvas (metadata, ignored by the runtime).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

/// Configuration of the `inject` node (EdgeLinkd interval/cron trigger).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct InjectConfig {
    /// Repeat interval in seconds (Node-RED `repeat`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_secs: Option<f64>,
    /// Cron expression (Node-RED `crontab`, 5 or 6 fields).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cron: String,
    /// One-shot injection after a delay (Node-RED `once` + `onceDelay`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub once_delay_secs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Injected payload (any JSON value; `null` = timestamp).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub payload: serde_json::Value,
}

/// Configuration of the custom `pnex-device-read` node — reads the **latest
/// values** of the pins of **a single device** (physical consistency: 1 node =
/// 1 device). The read goes through OpenObserve (PromQL `last_over_time` on
/// the same series as ingestion): a single source of truth, consistent
/// with the dashboard/Visualization. No access coordinates here — the real
/// O2 org is stamped into the artifact at deploy (`pnex_o2_org`) and the
/// creds come from the runtime env. Output ports: one per pin (payload
/// = raw pin value, `null` if the window is empty) + a final
/// "device name" port (payload = device slug).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceReadConfig {
    /// Device slug (`device_id` dimension of the O2 series).
    pub device_id: String,
    /// Labels of the pins read, in port order (the "device
    /// name" port is added after the last one). The O2 series is the
    /// normalized name, computed by the runtime via `normalize_measurement_name`.
    #[serde(default)]
    pub pins: Vec<String>,
    /// Freshness window (seconds): the latest value within the window
    /// is returned; beyond it the key is omitted from the payload. 1..=3600.
    #[serde(default = "default_window_secs")]
    pub window_secs: f64,
}

fn default_window_secs() -> f64 {
    60.0
}

/// Configuration for the `pnex-device-write` custom node — writes the
/// **output** pins of a **single device** (digital 1/0, pwm duty 0..=100).
/// Incoming payload = `{pin: value}` map (batch path) or a scalar routed by
/// `msg.topic` (wire drawn on the pin's input anchor). Downlink goes through
/// the backend (`device-write` internal route, dedicated token): the runtime
/// never touches the device WS directly.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceWriteConfig {
    /// Target device slug.
    pub device_id: String,
    /// Labels of the written pins, in visual-anchor order. Two write paths:
    /// an object payload is the batch path (its keys are matched against
    /// these labels); a scalar payload is written to the pin named by
    /// `msg.topic` when the wire was drawn on this pin's input anchor (the
    /// deploy projection stamps the topic — see `routed_wires`), or to the
    /// sole label on a single-pin node.
    #[serde(default)]
    pub pins: Vec<String>,
    /// Commands announced by a custom firmware (`pnex.onCommand`, D88/D146),
    /// in visual-anchor order after the pins. Same two paths as the pins
    /// (object payload key, or `msg.topic` stamped by a wire on the
    /// command's anchor); the device receives `args = {"value": payload}`.
    /// Commands are not pins: they never take part in the one-write-source
    /// rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
    /// Value declared per pin (O70): when set, any message reaching that pin
    /// writes this value — the message is only the trigger. `true`/`false`
    /// (or 0/1) on a digital output, a duty 0..=100 on a PWM output. Pins
    /// without an entry write the value the message carries.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub values: std::collections::BTreeMap<String, serde_json::Value>,
}

impl DeviceWriteConfig {
    /// Input anchors of the node: pins first, then commands.
    pub fn anchors(&self) -> impl Iterator<Item = &String> {
        self.pins.iter().chain(self.commands.iter())
    }
}

/// Configuration of the custom `calc` node — expression over the device
/// payload keys (identifier variables, evaluator [`crate::calc`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CalcConfig {
    pub expression: String,
}

/// Configuration of the custom `pnex-value` node — replaces `msg.payload`
/// with a fixed JSON document (the editor ships an object template; the
/// palette name is "Json Values"). Trigger stays upstream (inject): the
/// node is a transformer, not an autonomous source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValueConfig {
    /// Payload (free JSON; `null` = not set).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub value: serde_json::Value,
}

impl ValueConfig {
    /// Structural check shared by save validation and the runtime node build
    /// — `Some((code, message))` = config the runtime would reject. Single
    /// source of truth: what the editor flags is exactly what fails at deploy.
    pub fn check(&self) -> Option<(&'static str, String)> {
        self.value
            .is_null()
            .then(|| ("value_static_missing", "enter a value (empty field)".into()))
    }
}

/// Configuration of the PNEX `json-split` node — structural split of the
/// payload driven by the declared keys:
/// - non-empty `keys`: one output port **named per key** — `payload[key]`
///   goes out on that port (`payload` = value, `topic` = key); key missing from the
///   payload = silent port, key without a port = ignored; array → elements on
///   port 0 (`topic` = index); other payload → passthrough port 0.
/// - empty `keys` (single-port mode): a single port, one msg per key (`topic` = key),
///   array → one msg per element; any other payload passes unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonSplitConfig {
    /// Declared keys = named output ports, in declared order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
    /// Automatic upstream tracking (static-object Value or Merge): the
    /// keys — hence the **number of ports** — are regenerated by the editor on
    /// every graph edit. `true` by default.
    #[serde(default = "default_split_auto")]
    pub auto: bool,
}

impl Default for JsonSplitConfig {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            auto: true,
        }
    }
}

fn default_split_auto() -> bool {
    true
}

/// Configuration of the PNEX `json-merge` node (canvas: "Merge to JSON") —
/// compaction: each payload is stored under `msg.topic` (if present and non
/// empty), otherwise under the default key; the accumulated object is emitted on every
/// msg. The named inputs (`inputs`) are canvas rows: a wire
/// arriving there is stamped `topic = name` at deploy (tagger) — an input
/// carries **one variable**. State = runtime node context (lives as long as the
/// flow runs, reset on redeploy/restart).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonMergeConfig {
    /// Key for a scalar payload without `msg.topic`.
    #[serde(default = "default_merge_key")]
    pub default_key: String,
    /// Named inputs = canvas input rows, in declared order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
}

fn default_merge_key() -> String {
    "value".to_string()
}

impl JsonMergeConfig {
    /// Structural check shared by save validation and the runtime node build
    /// — `Some((code, message))` = config the runtime would reject. Single
    /// source of truth: what the editor flags is exactly what fails at deploy.
    pub fn check(&self) -> Option<(&'static str, String)> {
        if self.default_key.trim().is_empty() {
            return Some((
                "merge_default_key_missing",
                "enter the default key (scalar payload without topic)".into(),
            ));
        }
        None
    }
}

/// Configuration of the custom `metric` node — writes an OpenObserve
/// metric (remote-write): name auto-prefixed `etl_`, labels
/// `device_id="flow_{id}"`, `pred_dev="virtual_device"`, `source_type="etl"`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MetricConfig {
    /// Name entered by the user — prefixed `etl_` and sanitized on write
    /// (`etl_metric_name`), previewed identically in the editor.
    pub metric_name: String,
    /// Free series labels (D171): label name → source (`msg.topic`,
    /// `msg.payload.<field>` / `payload.<field>` with one or two levels, or
    /// a literal), resolved per message by the runtime.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub labels: std::collections::BTreeMap<String, String>,
}

/// Max distinct label-value tuples one `metric` node writes; new tuples
/// past it are dropped while known ones keep being written (D171).
pub const METRIC_LABEL_TUPLES_MAX: usize = 200;

/// Where a `metric` label value comes from (D171).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetricLabelSource<'a> {
    /// `msg.topic`.
    Topic,
    /// Dotted path under `msg.payload` (one or two levels).
    Payload(Vec<&'a str>),
    /// Fixed value.
    Literal(&'a str),
}

impl<'a> MetricLabelSource<'a> {
    /// Parses a label source; `None` = a malformed payload path.
    pub fn parse(raw: &'a str) -> Option<Self> {
        let raw = raw.trim();
        if raw == "msg.topic" {
            return Some(Self::Topic);
        }
        let Some(path) = raw
            .strip_prefix("msg.payload.")
            .or_else(|| raw.strip_prefix("payload."))
        else {
            return Some(Self::Literal(raw));
        };
        let parts: Vec<&str> = path.split('.').collect();
        (parts.len() <= 2 && parts.iter().all(|p| !p.is_empty())).then_some(Self::Payload(parts))
    }

    /// Label value for one message: numbers, booleans and strings are
    /// stringified then sanitized; a missing path gives `unknown`.
    pub fn resolve(
        &self,
        topic: Option<&serde_json::Value>,
        payload: Option<&serde_json::Value>,
    ) -> String {
        let found = match self {
            Self::Literal(v) => return crate::naming::sanitize_series_label_value(v),
            Self::Topic => topic,
            Self::Payload(parts) => parts
                .iter()
                .try_fold(payload, |cur, key| Some(cur?.get(*key)))
                .flatten(),
        };
        let text = match found {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Number(n)) => n.to_string(),
            Some(serde_json::Value::Bool(b)) => b.to_string(),
            _ => String::new(),
        };
        crate::naming::sanitize_series_label_value(&text)
    }
}

impl MetricConfig {
    /// Label check shared by the save validation and the runtime build.
    pub fn check(&self) -> Option<(&'static str, String)> {
        if self.metric_name.trim().is_empty() {
            return Some(("metric_name_missing", "the metric name is required".into()));
        }
        if self.labels.len() > crate::naming::SERIES_LABELS_MAX {
            return Some((
                "metric_labels_too_many",
                format!("at most {} labels", crate::naming::SERIES_LABELS_MAX),
            ));
        }
        for (name, source) in &self.labels {
            if !crate::naming::valid_series_label_name(name) {
                return Some((
                    "metric_label_name_invalid",
                    format!("invalid or reserved label name `{name}`"),
                ));
            }
            match MetricLabelSource::parse(source) {
                None => {
                    return Some((
                        "metric_label_source_invalid",
                        format!("invalid payload path for label `{name}`"),
                    ))
                }
                Some(MetricLabelSource::Literal(v))
                    if !crate::naming::valid_series_label_value(v) =>
                {
                    return Some((
                        "metric_label_value_invalid",
                        format!("invalid value for label `{name}`"),
                    ))
                }
                Some(_) => {}
            }
        }
        None
    }
}

/// Configuration of the custom `pnex-coolprop` node — in-process
/// thermophysical properties (`pnex-coolprop`, CoolProp v8): fluid/mixture
/// spec (PropsSI syntax: plain name, predefined mixture or
/// inline `Propane[0.5]&Ethane[0.5]`), two input properties (CoolProp
/// names: `T`, `P`, `Hmass`…) read from `msg.payload` (object
/// `key → numeric`, output of the device/calc nodes), and computed outputs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CoolPropConfig {
    /// Fluid/mixture spec (validated + frozen at edit time — D6).
    pub fluid_spec: String,
    /// Input property 1 (CoolProp name, e.g. `"T"`).
    pub input1: String,
    /// Input property 2 (CoolProp name, e.g. `"P"`).
    pub input2: String,
    /// `msg.payload` key for input value 1 (e.g. `"t"`).
    pub v1_key: String,
    /// `msg.payload` key for input value 2 (e.g. `"p"`).
    pub v2_key: String,
    /// CoolProp outputs (`Dmolar`, `Hmolar`, `Smolar`, `T`…) — at least one.
    #[serde(default)]
    pub outputs: Vec<String>,
    /// Add `"phase"` (PhaseSI text name) to the outgoing payload.
    #[serde(default)]
    pub include_phase: bool,
    /// Display name of the picked fluid (org mixture name or CoolProp
    /// fluid); `fluid_spec` stays the frozen PropsSI spec.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fluid_label: String,
    /// Unit of input 1 (catalogue id, see `thermo_quantities`); empty = SI.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unit1: String,
    /// Unit of input 2; empty = SI.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unit2: String,
    /// Unit per output id; a missing entry = SI.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub output_units: std::collections::BTreeMap<String, String>,
}

impl CoolPropConfig {
    /// Output port count: port 0 = every output as one object, then one
    /// port per output, then the phase port when enabled.
    pub fn port_count(&self) -> usize {
        1 + self.outputs.len() + usize::from(self.include_phase)
    }

    /// Unit id of an output (empty = SI).
    pub fn output_unit(&self, id: &str) -> &str {
        self.output_units.get(id).map(String::as_str).unwrap_or("")
    }
}

/// Configuration of the `debug` node (captures a pipeline's output).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DebugConfig {
    /// Node-RED `active` — capture enabled (default: yes).
    #[serde(default = "default_true")]
    pub active: bool,
    /// Captured property (`"payload"` by default, `"true"` = whole message).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complete: Option<String>,
    /// Also echoes to the runtime console.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub console: bool,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            active: true,
            complete: None,
            console: false,
        }
    }
}

fn default_true() -> bool {
    true
}

/// Configuration of the custom `pnex-display` node (probe) — passthrough +
/// publication to the debug panel. **No user field**: the identity
/// (`pnex_node_id`, flow, version) is stamped by the projection at
/// deploy — never read from a client config (attribution anti-forgery).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DisplayConfig;

/// Configuration of an **on/off** control card — shared by
/// the kinds `reg_tt_heat` (heating) and `reg_tt_cool` (cooling, inverted): the
/// kind encodes the direction of action, the config does not repeat it (a config
/// mistake is impossible).
///
/// **Mixed** card: the sensor and the output belong to the **same
/// device** — control runs embedded on the ESP (D13/D17: no
/// server loop), the server only casts this config (docs/
/// architecture/control-cards.md). Pins are **labels** (authoring)
/// resolved to gpio by the backend at sync (parity with `DeviceRead`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegTtConfig {
    /// Target device slug (the controlled mixed card).
    pub device_id: String,
    /// Sensor pin label (`digital_in`/`adc_in` mode — derived from the resolved
    /// pin, never entered).
    pub sensor_pin: String,
    /// Actuator pin label (`digital_out` mode).
    pub actuator_pin: String,
    /// Setpoint (sensor unit).
    pub setpoint: f64,
    /// Value hysteresis half-band: ON when crossing
    /// `setpoint ∓ deadband`, OFF when back at the setpoint. > 0.
    pub deadband: f64,
    /// Anti short-cycling: minimum ON duration (time hysteresis).
    #[serde(default = "default_reg_min_on_secs")]
    pub min_on_secs: u32,
    /// Anti short-cycling: minimum OFF duration.
    #[serde(default = "default_reg_min_off_secs")]
    pub min_off_secs: u32,
    /// Sensor sampling period on the ESP (200..=60_000 ms).
    #[serde(default = "default_reg_sample_ms")]
    pub sample_ms: u32,
    /// Sensor silent/unreadable beyond this duration → safe output.
    #[serde(default = "default_reg_data_timeout_secs")]
    pub data_timeout_secs: u32,
    /// Idle state of the output when control is stopped
    /// (undeploy, silent sensor, boot without config).
    #[serde(default)]
    pub safe_state: SafeState,
}

fn default_reg_min_on_secs() -> u32 {
    5
}
fn default_reg_min_off_secs() -> u32 {
    5
}
fn default_reg_sample_ms() -> u32 {
    5_000
}
fn default_reg_data_timeout_secs() -> u32 {
    30
}

impl Default for RegTtConfig {
    fn default() -> Self {
        Self {
            device_id: String::new(),
            sensor_pin: String::new(),
            actuator_pin: String::new(),
            setpoint: 0.0,
            deadband: 0.0,
            min_on_secs: default_reg_min_on_secs(),
            min_off_secs: default_reg_min_off_secs(),
            sample_ms: default_reg_sample_ms(),
            data_timeout_secs: default_reg_data_timeout_secs(),
            safe_state: SafeState::Low,
        }
    }
}

/// Configuration of a **PID** control card — time-proportional relay
/// output (the duty % is spread over `cycle_time_secs`, mathematical
/// contract: `pnex_core::control`). Same mixed model as
/// [`RegTtConfig`]: a single device, embedded control.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegPidConfig {
    pub device_id: String,
    pub sensor_pin: String,
    pub actuator_pin: String,
    /// Setpoint (sensor unit).
    pub setpoint: f64,
    /// Finite gains ≥ 0 (the derivative action is computed on the measurement).
    pub kp: f64,
    pub ki: f64,
    pub kd: f64,
    /// Time-proportional relay cycle period (1..=60 s).
    #[serde(default = "default_reg_cycle_time_secs")]
    pub cycle_time_secs: u32,
    #[serde(default = "default_reg_sample_ms")]
    pub sample_ms: u32,
    #[serde(default = "default_reg_data_timeout_secs")]
    pub data_timeout_secs: u32,
    #[serde(default)]
    pub safe_state: SafeState,
}

fn default_reg_cycle_time_secs() -> u32 {
    10
}

impl Default for RegPidConfig {
    fn default() -> Self {
        Self {
            device_id: String::new(),
            sensor_pin: String::new(),
            actuator_pin: String::new(),
            setpoint: 0.0,
            kp: 0.0,
            ki: 0.0,
            kd: 0.0,
            cycle_time_secs: default_reg_cycle_time_secs(),
            sample_ms: default_reg_sample_ms(),
            data_timeout_secs: default_reg_data_timeout_secs(),
            safe_state: SafeState::Low,
        }
    }
}

/// Control card extracted from a graph — unified form consumed by the
/// backend (`regulator_config` materialization + `control_config` cast).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RegConfig {
    TtHeat(RegTtConfig),
    TtCool(RegTtConfig),
    Pid(RegPidConfig),
}

// Manual `Deserialize`: same school as `FlowNodeKind` — internally-tagged
// enums buffer numeric fields through serde's `Content` deserializer and
// `arbitrary_precision` (unified workspace-wide by the starlark dependency)
// turns those buffered numbers into private magic maps, breaking typed
// numeric fields. Parsing through a `serde_json::Value` keeps numbers as
// real Numbers. Runtime parse site: `regulator.rs` (card materialization —
// a parse failure would silently drop the card). Internally-tagged newtype
// variants flatten the inner struct onto the same map as the tag, so the
// config deserializes from the whole map (its derive ignores `kind`).
impl<'de> Deserialize<'de> for RegConfig {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = serde_json::Value::deserialize(d)?;
        let parsed: Result<Self, String> = (|| {
            let kind = v
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "missing field `kind`".to_string())?;
            match kind {
                "tt_heat" => Ok(Self::TtHeat(
                    serde_json::from_value(v.clone()).map_err(|e| e.to_string())?,
                )),
                "tt_cool" => Ok(Self::TtCool(
                    serde_json::from_value(v.clone()).map_err(|e| e.to_string())?,
                )),
                "pid" => Ok(Self::Pid(
                    serde_json::from_value(v.clone()).map_err(|e| e.to_string())?,
                )),
                other => Err(format!("unknown regulation card: {other}")),
            }
        })();
        parsed.map_err(serde::de::Error::custom)
    }
}

impl RegConfig {
    /// Card discriminant (`tt_heat` | `tt_cool` | `pid`) — the same
    /// string as the `kind` field of the wire `ControlSpec`.
    pub fn kind_str(&self) -> &'static str {
        match self {
            RegConfig::TtHeat(_) => "tt_heat",
            RegConfig::TtCool(_) => "tt_cool",
            RegConfig::Pid(_) => "pid",
        }
    }

    pub fn device_id(&self) -> &str {
        match self {
            RegConfig::TtHeat(c) | RegConfig::TtCool(c) => &c.device_id,
            RegConfig::Pid(c) => &c.device_id,
        }
    }

    pub fn sensor_pin(&self) -> &str {
        match self {
            RegConfig::TtHeat(c) | RegConfig::TtCool(c) => &c.sensor_pin,
            RegConfig::Pid(c) => &c.sensor_pin,
        }
    }

    pub fn actuator_pin(&self) -> &str {
        match self {
            RegConfig::TtHeat(c) | RegConfig::TtCool(c) => &c.actuator_pin,
            RegConfig::Pid(c) => &c.actuator_pin,
        }
    }

    pub fn setpoint(&self) -> f64 {
        match self {
            RegConfig::TtHeat(c) | RegConfig::TtCool(c) => c.setpoint,
            RegConfig::Pid(c) => c.setpoint,
        }
    }

    pub fn sample_ms(&self) -> u32 {
        match self {
            RegConfig::TtHeat(c) | RegConfig::TtCool(c) => c.sample_ms,
            RegConfig::Pid(c) => c.sample_ms,
        }
    }

    pub fn data_timeout_secs(&self) -> u32 {
        match self {
            RegConfig::TtHeat(c) | RegConfig::TtCool(c) => c.data_timeout_secs,
            RegConfig::Pid(c) => c.data_timeout_secs,
        }
    }

    pub fn safe_state(&self) -> SafeState {
        match self {
            RegConfig::TtHeat(c) | RegConfig::TtCool(c) => c.safe_state,
            RegConfig::Pid(c) => c.safe_state,
        }
    }

    /// Uniqueness key of a control per output: (device, actuator pin),
    /// normalized labels (trim + ASCII case ignored) — used by
    /// `validate_graph` (graph) then by the backend sync (all deployed
    /// flows).
    pub fn actuator_key(&self) -> String {
        format!(
            "{}|{}",
            self.device_id().trim(),
            self.actuator_pin().trim().to_ascii_lowercase()
        )
    }
}

/// Extracts the control cards of a graph: `(node id, config)` in
/// graph order. A single definition, consumed by the backend sync
/// (materialization + cast) and by the tests.
pub fn regulator_configs_of(g: &FlowGraph) -> Vec<(String, RegConfig)> {
    g.nodes
        .iter()
        .filter_map(|n| reg_config_of(&n.kind).map(|c| (n.id.clone(), c)))
        .collect()
}

/// The card carried by this kind, if it carries one.
pub fn reg_config_of(kind: &FlowNodeKind) -> Option<RegConfig> {
    match kind {
        FlowNodeKind::RegTtHeat { config } => Some(RegConfig::TtHeat(config.clone())),
        FlowNodeKind::RegTtCool { config } => Some(RegConfig::TtCool(config.clone())),
        FlowNodeKind::RegPid { config } => Some(RegConfig::Pid(config.clone())),
        _ => None,
    }
}

/// Use of a device pin by a graph node — lets the backend
/// qualify the impact of a device config change (409 guard / stop).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePinUsage {
    /// Read (`device-read` node or sensor pin of a control card).
    Read,
    /// Write (a `device-write` pin, or a regulator card's actuator pin —
    /// both drive the physical output).
    Write,
}

/// Reference (node, pin, usage) of a device in a graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicePinRef {
    pub node_id: String,
    pub pin: String,
    pub usage: DevicePinUsage,
}

/// All pins of a device (exact slug) referenced by a graph — `device-read`/
/// `device-write` nodes (labels normalized trim+case) and BOTH pins of
/// regulator cards (sensor = Read, actuator = Write). Consumed by the
/// backend for the "device used by a deployed flow" guard (set_mode → 409
/// / stop) and for write-ownership scans (one write source per output).
pub fn device_pin_refs_of(g: &FlowGraph, device_id: &str) -> Vec<DevicePinRef> {
    let target = device_id.trim();
    let mut refs = Vec::new();
    for n in &g.nodes {
        match &n.kind {
            FlowNodeKind::DeviceRead { config } if config.device_id.trim() == target => {
                for pin in &config.pins {
                    refs.push(DevicePinRef {
                        node_id: n.id.clone(),
                        pin: pin_key(pin).into_owned(),
                        usage: DevicePinUsage::Read,
                    });
                }
            }
            FlowNodeKind::DeviceWrite { config } if config.device_id.trim() == target => {
                for pin in &config.pins {
                    refs.push(DevicePinRef {
                        node_id: n.id.clone(),
                        pin: pin_key(pin).into_owned(),
                        usage: DevicePinUsage::Write,
                    });
                }
            }
            kind => {
                if let Some(cfg) = reg_config_of(kind) {
                    if cfg.device_id().trim() == target {
                        refs.push(DevicePinRef {
                            node_id: n.id.clone(),
                            pin: pin_key(cfg.sensor_pin()).into_owned(),
                            usage: DevicePinUsage::Read,
                        });
                        refs.push(DevicePinRef {
                            node_id: n.id.clone(),
                            pin: pin_key(cfg.actuator_pin()).into_owned(),
                            usage: DevicePinUsage::Write,
                        });
                    }
                }
            }
        }
    }
    refs
}

/// A single output-pin WRITE claim of a graph — `device-write` pins and
/// regulator-card actuator pins both drive the physical output. Device slug
/// trimmed, pin label normalized (`pin_key`: trim + ASCII-lowercase).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceWritePinRef {
    pub node_id: String,
    pub device_id: String,
    pub pin: String,
}

/// All output-pin write claims of a graph (no device filter) — the pure
/// primitive behind the cross-flow "one write source per output" rule
/// (deploy gate, manual write guard, pinout reservations).
pub fn device_write_pin_refs_of(g: &FlowGraph) -> Vec<DeviceWritePinRef> {
    let mut refs = Vec::new();
    for n in &g.nodes {
        match &n.kind {
            FlowNodeKind::DeviceWrite { config } => {
                for pin in &config.pins {
                    refs.push(DeviceWritePinRef {
                        node_id: n.id.clone(),
                        device_id: config.device_id.trim().to_string(),
                        pin: pin_key(pin).into_owned(),
                    });
                }
            }
            kind => {
                if let Some(cfg) = reg_config_of(kind) {
                    refs.push(DeviceWritePinRef {
                        node_id: n.id.clone(),
                        device_id: cfg.device_id().trim().to_string(),
                        pin: pin_key(cfg.actuator_pin()).into_owned(),
                    });
                }
            }
        }
    }
    refs
}

/// Org controls listened to by a graph (`control-source` nodes), deduplicated
/// and sorted: "no effect" badge, deploy existence check, delete guard.
pub fn control_refs_of(g: &FlowGraph) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = g
        .nodes
        .iter()
        .filter_map(|n| match &n.kind {
            FlowNodeKind::ControlSource { config } => Some(config.controls.iter().copied()),
            _ => None,
        })
        .flatten()
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Ontology actions handled by a graph (`action-trigger` nodes), as
/// `(object_type, action)`, deduplicated and sorted: invocation listener
/// lookup and deploy existence check (D183).
pub fn action_refs_of(g: &FlowGraph) -> Vec<(String, String)> {
    let mut refs: Vec<(String, String)> = g
        .nodes
        .iter()
        .filter_map(|n| match &n.kind {
            FlowNodeKind::ActionTrigger { config } => {
                Some((config.object_type.clone(), config.action.clone()))
            }
            _ => None,
        })
        .collect();
    refs.sort();
    refs.dedup();
    refs
}

/// Reserved name of the notify node's permanent boolean gate input: a wire
/// annotated with this pin feeds the trigger gate (`topic = "trigger"` at
/// runtime), never a template var.
pub const NOTIFY_TRIGGER_PIN: &str = "trigger";

/// Configuration of the custom `pnex-notify` node — renders an org
/// minijinja template then sends to one or more channels (D49–D54).
/// The node only carries **references**: at deploy, the backend resolves
/// and stamps the snapshot (`pnex_notify_channels` / `pnex_notify_template`)
/// into the artifact — editing a channel/template does not mutate already
/// deployed flows (redeploy to propagate, "saved ≠ deployed" school).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NotifyNodeConfig {
    /// Target channels (`notify_channels.id`) — at least one (validate_graph).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channel_ids: Vec<Uuid>,
    /// Render template (`notify_templates.id`) — nil = not set.
    /// Tolerates JSON `null`/`""` (the editor emits null for "not set").
    #[serde(default, deserialize_with = "deserialize_nil_uuid")]
    pub template_id: Uuid,
    /// `true`: a send failure fails the node (isolated flow_error, never a
    /// crash-loop); lenient default: warn + passthrough (D53).
    #[serde(default)]
    pub strict: bool,
    /// Node overrides — literal **or** `{{ msg.x }}` expression
    /// pre-rendered against `{msg, meta}` before the template is rendered
    /// (2-level composition, D52).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub vars: std::collections::BTreeMap<String, String>,
    /// Names of the template vars stamped at pick time (template order) —
    /// source of the named canvas input anchors and of the deploy routing
    /// (tagger `topic = var`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub template_vars: Vec<String>,
    /// Anti-spam — fixed window "at most N sends per window of D seconds":
    /// the first send opens the window, the surplus is held until the
    /// reset (the send is deferred, not lost — the filled set stays pending
    /// on the runtime side). `None` = no limit. The passthrough is unaffected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anti_spam: Option<AntiSpamConfig>,
    /// Path, in the render context, of the media id of a PDF to attach
    /// (D209: `asset_id` after a `page_pdf` node). Empty = no attachment.
    /// Only channels that carry files (`smtp`) attach it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub attach_field: String,
    /// Path, in the render context, of the recipient e-mail (D313): only on
    /// SMTP channels marked `dynamic_recipients`, one address, capped per
    /// day per channel and per recipient by the server. Empty = the
    /// channel's fixed recipients.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub to_field: String,
    /// Path of one address in copy (with `to_field` only).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cc_field: String,
}

/// Anti-spam rule of a notify node (count × duration, a single readable
/// rule — the low-code spirit, not the Alertmanager school).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct AntiSpamConfig {
    /// Sends allowed per window — ≥ 1 (validate_graph).
    pub max_msgs: u32,
    /// Window duration in seconds — ≥ 1 (validate_graph).
    pub window_secs: u64,
}

/// Configuration of the custom `pnex-http-fetch` node (C1a) — "curl-like"
/// HTTP client request: GET/POST method, headers, auth (basic/bearer/
/// key in a header), configurable proxy (direct or scraping provider).
/// The response replaces the payload; the standard ETL engine takes over.
/// Secret fields (basic/proxy passwords, bearer token, key header value)
/// are [`crate::SecretSlot`] vault references once saved (secrets.md S5).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HttpFetchHeader {
    pub name: String,
    #[serde(default)]
    pub value: String,
}

/// HTTP method v1 — GET/POST cover the collection case (JSON API
/// reads, third-party ingestion webhook).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HttpFetchMethod {
    #[default]
    Get,
    Post,
}

/// Request auth — tagged `mode` (the editor emits `{"mode": "basic", …}`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum HttpFetchAuth {
    #[default]
    None,
    Basic {
        username: String,
        #[serde(default)]
        password: crate::SecretSlot,
    },
    Bearer {
        #[serde(default)]
        token: crate::SecretSlot,
    },
    /// API key in an arbitrary header (`X-Api-Key: …`).
    Header {
        name: String,
        #[serde(default)]
        value: crate::SecretSlot,
    },
}

/// Outbound proxy — `custom` covers scraping providers in proxy mode
/// (HTTP CONNECT + user/pass); the "key in the target URL" idiom
/// (ScraperAPI/ZenRows) needs no proxy at all.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum HttpFetchProxy {
    #[default]
    None,
    Custom {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        username: Option<String>,
        #[serde(default, skip_serializing_if = "crate::SecretSlot::is_unset")]
        password: crate::SecretSlot,
    },
}

/// Error semantics: `reject` → message rejected (`flow.handle_error`,
/// isolated flow_error, the flow survives — notify strict school); `passthrough`
/// → null payload + `http_error` + statusCode, the graph decides (switch…).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpFetchOnError {
    #[default]
    Reject,
    Passthrough,
}

/// **Manual** `Default`: aligned with the serde default (`timeout_secs: 30`),
/// otherwise the Rust constructor would yield 0 (spurious violation in the editor).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HttpFetchNodeConfig {
    /// Full URL (query included) — no templating in v1 (compute upstream
    /// with a calc node).
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub method: HttpFetchMethod,
    /// Additional static headers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<HttpFetchHeader>,
    #[serde(default)]
    pub auth: HttpFetchAuth,
    #[serde(default)]
    pub proxy: HttpFetchProxy,
    /// Total request timeout (s) — bounded 1..=300 by validate_graph.
    #[serde(default = "default_http_timeout_secs")]
    pub timeout_secs: u64,
    /// Literal body (POST) — absent ⇒ msg payload serialized as JSON
    /// (+ `content-type: application/json`, overridable by an explicit header).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default)]
    pub on_error: HttpFetchOnError,
}

fn default_http_timeout_secs() -> u64 {
    30
}

impl HttpFetchNodeConfig {
    /// Destination key of a secret field (R9): the request URL's origin
    /// for `auth.*`, the proxy's for `proxy.password`.
    pub fn secret_destination(&self, field: &str) -> String {
        match (&self.proxy, field) {
            (HttpFetchProxy::Custom { url, .. }, "proxy.password") => crate::destination_key(url),
            _ => crate::destination_key(&self.url),
        }
    }

    /// The secret fields of the node, by stable field name
    /// (`auth.password`, `auth.token`, `auth.value`, `proxy.password`).
    pub fn secret_slots(&self) -> Vec<(&'static str, &crate::SecretSlot)> {
        let mut out = Vec::new();
        match &self.auth {
            HttpFetchAuth::Basic { password, .. } => out.push(("auth.password", password)),
            HttpFetchAuth::Bearer { token } => out.push(("auth.token", token)),
            HttpFetchAuth::Header { value, .. } => out.push(("auth.value", value)),
            HttpFetchAuth::None => {}
        }
        if let HttpFetchProxy::Custom { password, .. } = &self.proxy {
            out.push(("proxy.password", password));
        }
        out
    }

    /// Mutable twin of [`Self::secret_slots`].
    pub fn secret_slots_mut(&mut self) -> Vec<(&'static str, &mut crate::SecretSlot)> {
        let mut out = Vec::new();
        match &mut self.auth {
            HttpFetchAuth::Basic { password, .. } => out.push(("auth.password", password)),
            HttpFetchAuth::Bearer { token } => out.push(("auth.token", token)),
            HttpFetchAuth::Header { value, .. } => out.push(("auth.value", value)),
            HttpFetchAuth::None => {}
        }
        if let HttpFetchProxy::Custom { password, .. } = &mut self.proxy {
            out.push(("proxy.password", password));
        }
        out
    }
}

impl Default for HttpFetchNodeConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            method: HttpFetchMethod::default(),
            headers: Vec::new(),
            auth: HttpFetchAuth::default(),
            proxy: HttpFetchProxy::default(),
            timeout_secs: default_http_timeout_secs(),
            body: None,
            on_error: HttpFetchOnError::default(),
        }
    }
}

/// UUID tolerant to JSON `null`/`""` (becomes nil) — the front editor handles
/// strings and represents "not set" as null.
fn deserialize_nil_uuid<'de, D>(d: D) -> Result<Uuid, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v: Option<String> = Option::deserialize(d)?;
    match v.as_deref() {
        None | Some("") => Ok(Uuid::nil()),
        Some(s) => Uuid::parse_str(s).map_err(serde::de::Error::custom),
    }
}

// ───────────────────── Camera & video nodes (D78) ─────────────────────

/// Configuration of the `camera-source` node — the first event-driven
/// source node: subscribes to the camera frame bus of one device (Valkey
/// pub/sub, fed by the backend CameraHub) and emits one message per frame.
/// Messages carry a **reference** to the JPEG (`payload.frame_key`, TTL
/// 15 s), never the bytes (camera-video.md D74).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CameraSourceConfig {
    /// Where the frames come from (D175); `device` by default.
    #[serde(default, skip_serializing_if = "CameraSourceKind::is_device")]
    pub source: CameraSourceKind,
    /// Camera device slug (`source: device`).
    #[serde(default)]
    pub device_id: String,
    /// Media stream slug (`source: stream`): a stream of the org whose
    /// `tracks` has video, checked at deploy.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stream: String,
    /// Sampling: at most this many messages per second (0 = every frame).
    #[serde(default)]
    pub max_fps: f64,
}

/// Frame source of a `camera-source` node (D175).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraSourceKind {
    /// A camera device (`pnex:cam:v1:{org}:{device}`).
    #[default]
    Device,
    /// The video track of a media stream (`pnex:media:v1:{org}:{slug}:cam`).
    Stream,
}

impl CameraSourceKind {
    pub fn is_device(&self) -> bool {
        *self == Self::Device
    }
}

/// Upper bound of `max_fps` on camera nodes (the camera itself caps at 25).
pub const CAMERA_NODE_MAX_FPS: f64 = 25.0;

impl CameraSourceConfig {
    /// Structural check shared by the save validation and the runtime build.
    pub fn check(&self) -> Option<(&'static str, String)> {
        match self.source {
            CameraSourceKind::Device if !crate::naming::valid_device_label(&self.device_id) => {
                return Some(("camera_device_missing", "select a camera".into()));
            }
            CameraSourceKind::Stream if !crate::media_ingest::is_valid_slug(&self.stream) => {
                return Some(("camera_stream_missing", "select a media stream".into()));
            }
            _ => {}
        }
        if !self.max_fps.is_finite() || self.max_fps < 0.0 || self.max_fps > CAMERA_NODE_MAX_FPS {
            return Some((
                "camera_fps_invalid",
                format!("max fps must be between 0 (every frame) and {CAMERA_NODE_MAX_FPS}"),
            ));
        }
        None
    }
}

/// Configuration of the `video-record` node — accumulates the frames of
/// incoming `camera-source` messages into MJPEG-AVI segments, one buffer
/// per camera, and stores each segment through the backend MediaStore
/// (fs | s3). Flush on duration, size or frame gap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoRecordConfig {
    /// Segment duration in seconds.
    #[serde(default = "default_segment_secs")]
    pub segment_secs: u32,
    /// Segment size cap in MB (flushes early when reached).
    #[serde(default = "default_segment_mb")]
    pub max_segment_mb: u32,
    /// Flush when no frame arrived for this many seconds (camera stopped).
    #[serde(default = "default_gap_secs")]
    pub gap_secs: u32,
    /// Recording rate cap (0 = every incoming frame).
    #[serde(default)]
    pub max_fps: f64,
    /// Days before the segments are pruned (0 = keep forever).
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    /// Logical stream name (empty = the camera slug).
    #[serde(default)]
    pub stream: String,
}

fn default_segment_secs() -> u32 {
    60
}
fn default_segment_mb() -> u32 {
    32
}
fn default_gap_secs() -> u32 {
    10
}
fn default_retention_days() -> u32 {
    7
}

impl Default for VideoRecordConfig {
    fn default() -> Self {
        Self {
            segment_secs: default_segment_secs(),
            max_segment_mb: default_segment_mb(),
            gap_secs: default_gap_secs(),
            max_fps: 0.0,
            retention_days: default_retention_days(),
            stream: String::new(),
        }
    }
}

pub const VIDEO_SEGMENT_SECS_RANGE: (u32, u32) = (5, 3600);
pub const VIDEO_SEGMENT_MB_RANGE: (u32, u32) = (1, 256);
pub const VIDEO_GAP_SECS_RANGE: (u32, u32) = (1, 600);
pub const VIDEO_RETENTION_DAYS_MAX: u32 = 3650;

impl VideoRecordConfig {
    /// Structural check shared by the save validation and the runtime build.
    pub fn check(&self) -> Option<(&'static str, String)> {
        let in_range = |v: u32, (lo, hi): (u32, u32)| (lo..=hi).contains(&v);
        if !in_range(self.segment_secs, VIDEO_SEGMENT_SECS_RANGE) {
            return Some((
                "video_segment_secs_invalid",
                format!(
                    "segment duration must be between {} and {} seconds",
                    VIDEO_SEGMENT_SECS_RANGE.0, VIDEO_SEGMENT_SECS_RANGE.1
                ),
            ));
        }
        if !in_range(self.max_segment_mb, VIDEO_SEGMENT_MB_RANGE) {
            return Some((
                "video_segment_mb_invalid",
                format!(
                    "segment size must be between {} and {} MB",
                    VIDEO_SEGMENT_MB_RANGE.0, VIDEO_SEGMENT_MB_RANGE.1
                ),
            ));
        }
        if !in_range(self.gap_secs, VIDEO_GAP_SECS_RANGE) {
            return Some((
                "video_gap_secs_invalid",
                format!(
                    "gap must be between {} and {} seconds",
                    VIDEO_GAP_SECS_RANGE.0, VIDEO_GAP_SECS_RANGE.1
                ),
            ));
        }
        if !self.max_fps.is_finite() || self.max_fps < 0.0 || self.max_fps > CAMERA_NODE_MAX_FPS {
            return Some((
                "camera_fps_invalid",
                format!("max fps must be between 0 (every frame) and {CAMERA_NODE_MAX_FPS}"),
            ));
        }
        if self.retention_days > VIDEO_RETENTION_DAYS_MAX {
            return Some((
                "video_retention_invalid",
                format!("retention must be at most {VIDEO_RETENTION_DAYS_MAX} days"),
            ));
        }
        if self.stream.chars().count() > 128 {
            return Some((
                "video_stream_too_long",
                "stream name is limited to 128 characters".into(),
            ));
        }
        None
    }
}

/// A camera recorded by a `video-record` node — one recorder per camera
/// across the org (two recorders would write the same frames twice into
/// one timeline).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoRecordClaim {
    pub node_id: String,
    pub device_id: String,
}

/// Recordings of a graph: for each `video-record` node, the cameras of the
/// `camera-source` nodes upstream of it (any path — through a vision node
/// for instance).
pub fn video_record_claims_of(g: &FlowGraph) -> Vec<VideoRecordClaim> {
    use std::collections::{HashMap, HashSet};
    // Reverse adjacency: target -> sources.
    let mut parents: HashMap<&str, Vec<&str>> = HashMap::new();
    for n in &g.nodes {
        for w in &n.outputs {
            for t in &w.targets {
                parents.entry(t.as_str()).or_default().push(n.id.as_str());
            }
        }
    }
    let by_id: HashMap<&str, &FlowNode> = g.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut claims = Vec::new();
    for n in &g.nodes {
        let FlowNodeKind::VideoRecord { .. } = &n.kind else {
            continue;
        };
        let mut seen: HashSet<&str> = HashSet::new();
        let mut stack = vec![n.id.as_str()];
        let mut cameras: Vec<String> = Vec::new();
        while let Some(id) = stack.pop() {
            for &p in parents.get(id).map(Vec::as_slice).unwrap_or(&[]) {
                if !seen.insert(p) {
                    continue;
                }
                if let Some(FlowNodeKind::CameraSource { config: src }) =
                    by_id.get(p).map(|n| &n.kind)
                {
                    // A stream never shares a claim key with a device slug.
                    let cam = match src.source {
                        CameraSourceKind::Device => src.device_id.trim().to_string(),
                        CameraSourceKind::Stream => format!("stream:{}", src.stream.trim()),
                    };
                    if !cam.is_empty() && !cameras.contains(&cam) {
                        cameras.push(cam);
                    }
                }
                stack.push(p);
            }
        }
        for cam in cameras {
            claims.push(VideoRecordClaim {
                node_id: n.id.clone(),
                device_id: cam,
            });
        }
    }
    claims
}
