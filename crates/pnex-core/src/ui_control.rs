//! Org controls (D125–D127, `docs/architecture/surfaces-controls.md`):
//! the write side of the surfaces. Pure and wasm-safe (serde only).
//!
//! A surface (dashboard card, annotation) never writes a pin: it writes the
//! value of an org-level **control**, stored and published in Valkey. A
//! deployed flow listens through the `control-source` node and decides what
//! happens (`device-write` on 1..N devices). Symmetric of the org memory
//! ([`crate::memory`]): memory = flow → surfaces, control = surfaces → flow.
//!
//! `ControlSpec::accepts` is the single value gate, run by the browser before
//! sending and by the server before storing.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::memory::valid_memory_key;

/// Max length of a control label.
pub const CONTROL_LABEL_MAX_LEN: usize = 120;
/// Max number of controls one `control-source` node listens to.
pub const CONTROL_SOURCE_MAX: usize = 32;
/// Min interval between two accepted writes of one control (server-side
/// rate limit, ~4 writes per second).
pub const CONTROL_WRITE_MIN_INTERVAL_MS: i64 = 250;
/// Max number of controls read in one `values` request.
pub const CONTROL_VALUES_MAX_IDS: usize = 200;
/// Max length of the free `via` origin tag (`dashboard:{uuid}`…).
pub const CONTROL_VIA_MAX_LEN: usize = 80;

/// Control key charset: same as memory keys (`[A-Za-z0-9_.-]`, 1..=64).
/// The key is the `msg.topic` emitted by the `control-source` node.
pub fn valid_control_key(key: &str) -> bool {
    valid_memory_key(key)
}

/// Valkey key of the last commanded value: `pnex:ctl:v1:{org_id}:{id}`.
/// No TTL: the last command persists (re-emitted by `emit_on_start`).
pub fn control_value_key(org_id: i64, control_id: Uuid) -> String {
    format!("pnex:ctl:v1:{org_id}:{control_id}")
}

/// Valkey pub/sub channel of the org control writes (one per org; the
/// `control-source` nodes filter on their control ids).
pub fn control_channel(org_id: i64) -> String {
    format!("pnex:ctl:v1:{org_id}")
}

/// Kind of a control. Plain enum (no internally tagged enum: the workspace
/// unifies serde_json `arbitrary_precision`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlKind {
    /// Two values (`on` / `off`, default 1 / 0): a digital output as is.
    Switch,
    /// Bounded range with a step (default 0..=100 step 1): a PWM duty as is.
    Slider,
    /// Momentary: every press sends the same `press` value (default 1).
    Button,
    /// Free numeric input, optionally bounded.
    Number,
}

impl ControlKind {
    pub const ALL: [ControlKind; 4] = [
        ControlKind::Switch,
        ControlKind::Slider,
        ControlKind::Button,
        ControlKind::Number,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ControlKind::Switch => "switch",
            ControlKind::Slider => "slider",
            ControlKind::Button => "button",
            ControlKind::Number => "number",
        }
    }

    /// Widget type of the dashboard card that drives this kind.
    pub fn widget_type(self) -> &'static str {
        self.as_str()
    }
}

/// Why a value is refused by [`ControlSpec::accepts`]. The code is a
/// machine token (field-error school): the UI translates it.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlValueError {
    NotFinite,
    /// Switch: neither `on` nor `off`.
    NotAState {
        on: f64,
        off: f64,
    },
    /// Button: not the `press` value.
    NotThePress {
        press: f64,
    },
    OutOfRange {
        min: Option<f64>,
        max: Option<f64>,
    },
    OffStep {
        step: f64,
    },
}

impl ControlValueError {
    pub fn code(&self) -> &'static str {
        match self {
            ControlValueError::NotFinite => "not_finite",
            ControlValueError::NotAState { .. } => "not_a_state",
            ControlValueError::NotThePress { .. } => "not_the_press",
            ControlValueError::OutOfRange { .. } => "out_of_range",
            ControlValueError::OffStep { .. } => "off_step",
        }
    }
}

/// Definition of a control value domain. Flat struct with optional fields
/// read through defaulting accessors (an unused field for the kind is
/// ignored).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlSpec {
    pub kind: ControlKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub press: Option<f64>,
    /// Display unit (`%`, `°C`…), informative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Ask for a confirmation before sending (dangerous actuators).
    #[serde(default)]
    pub confirm: bool,
}

impl ControlSpec {
    /// Default spec of a kind (switch 1/0, slider 0..=100 step 1, button 1).
    pub fn new(kind: ControlKind) -> Self {
        Self {
            kind,
            on: None,
            off: None,
            min: None,
            max: None,
            step: None,
            press: None,
            unit: None,
            confirm: false,
        }
    }

    pub fn on_value(&self) -> f64 {
        self.on.unwrap_or(1.0)
    }

    pub fn off_value(&self) -> f64 {
        self.off.unwrap_or(0.0)
    }

    pub fn press_value(&self) -> f64 {
        self.press.unwrap_or(1.0)
    }

    /// Effective lower bound: slider defaults to 0, number is unbounded.
    pub fn min_value(&self) -> Option<f64> {
        match self.kind {
            ControlKind::Slider => Some(self.min.unwrap_or(0.0)),
            ControlKind::Number => self.min,
            _ => None,
        }
    }

    /// Effective upper bound: slider defaults to 100, number is unbounded.
    pub fn max_value(&self) -> Option<f64> {
        match self.kind {
            ControlKind::Slider => Some(self.max.unwrap_or(100.0)),
            ControlKind::Number => self.max,
            _ => None,
        }
    }

    /// Effective step: slider defaults to 1, number accepts any value.
    pub fn step_value(&self) -> Option<f64> {
        match self.kind {
            ControlKind::Slider => Some(self.step.unwrap_or(1.0)),
            ControlKind::Number => self.step,
            _ => None,
        }
    }

    /// Structural check (create/update API, front form).
    pub fn check(&self) -> Option<(&'static str, String)> {
        let all = [self.on, self.off, self.min, self.max, self.step, self.press];
        if all.iter().flatten().any(|x| !x.is_finite()) {
            return Some(("control_spec_not_finite", "values must be finite".into()));
        }
        if let Some(unit) = &self.unit {
            if unit.chars().count() > 16 || unit.chars().any(char::is_control) {
                return Some((
                    "control_spec_unit",
                    "unit must be at most 16 printable characters".into(),
                ));
            }
        }
        match self.kind {
            ControlKind::Switch if self.on_value() == self.off_value() => Some((
                "control_spec_switch_same",
                "on and off values must differ".into(),
            )),
            ControlKind::Slider | ControlKind::Number => {
                if let (Some(lo), Some(hi)) = (self.min_value(), self.max_value()) {
                    if lo >= hi {
                        return Some(("control_spec_range", "min must be < max".into()));
                    }
                }
                if let Some(step) = self.step_value() {
                    if step <= 0.0 {
                        return Some(("control_spec_step", "step must be > 0".into()));
                    }
                    if let (Some(lo), Some(hi)) = (self.min_value(), self.max_value()) {
                        // A slider with millions of positions is a number input.
                        if (hi - lo) / step > 100_000.0 {
                            return Some((
                                "control_spec_step",
                                "at most 100000 steps between min and max".into(),
                            ));
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Value gate: returns the value to store (snapped to the step grid to
    /// absorb float noise such as `0.30000000000000004`), or why it is
    /// refused.
    pub fn accepts(&self, v: f64) -> Result<f64, ControlValueError> {
        if !v.is_finite() {
            return Err(ControlValueError::NotFinite);
        }
        match self.kind {
            ControlKind::Switch => {
                let (on, off) = (self.on_value(), self.off_value());
                if v == on || v == off {
                    Ok(v)
                } else {
                    Err(ControlValueError::NotAState { on, off })
                }
            }
            ControlKind::Button => {
                let press = self.press_value();
                if v == press {
                    Ok(v)
                } else {
                    Err(ControlValueError::NotThePress { press })
                }
            }
            ControlKind::Slider | ControlKind::Number => {
                let (min, max) = (self.min_value(), self.max_value());
                let below = min.is_some_and(|lo| v < lo);
                let above = max.is_some_and(|hi| v > hi);
                if below || above {
                    return Err(ControlValueError::OutOfRange { min, max });
                }
                let Some(step) = self.step_value() else {
                    return Ok(v);
                };
                let origin = min.unwrap_or(0.0);
                let n = ((v - origin) / step).round();
                let snapped = origin + n * step;
                if (snapped - v).abs() > step * 1e-6 {
                    return Err(ControlValueError::OffStep { step });
                }
                // Clamp: the snap may cross a bound by float noise only.
                let snapped = min.map_or(snapped, |lo| snapped.max(lo));
                Ok(max.map_or(snapped, |hi| snapped.min(hi)))
            }
        }
    }
}

/// Label rule (create/update API, front form).
pub fn check_control_label(label: &str) -> Option<(&'static str, String)> {
    let trimmed = label.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > CONTROL_LABEL_MAX_LEN
        || trimmed.chars().any(char::is_control)
    {
        return Some((
            "control_label_invalid",
            format!("label must be 1 to {CONTROL_LABEL_MAX_LEN} printable characters"),
        ));
    }
    None
}

/// Last commanded value. Stored and published wrapped in a
/// [`ControlEvent`] (under [`control_value_key`], on [`control_channel`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlValue {
    pub v: f64,
    /// Write time, epoch milliseconds (server clock).
    pub ts_ms: i64,
    /// Display name of the user who wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// Originating surface (`dashboard:{uuid}`, `annotation:{uuid}`),
    /// informative only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

/// Pub/sub frame: one control write. `key` is denormalized so the node can
/// set `msg.topic` without a database lookup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlEvent {
    pub control_id: Uuid,
    pub key: String,
    pub value: ControlValue,
}

/// `via` rule: short, printable, optional.
pub fn valid_control_via(via: &str) -> bool {
    !via.is_empty()
        && via.len() <= CONTROL_VIA_MAX_LEN
        && via
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '.' | '-'))
}

/// Reference from a surface item (dashboard widget, annotation) to a control.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ControlRef {
    pub control_id: Uuid,
}

/// Configuration of the `control-source` flow node: one output port per
/// listed control (`payload` = value, `topic` = control key).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ControlSourceConfig {
    #[serde(default)]
    pub controls: Vec<Uuid>,
    /// Re-emit the current values at engine start / redeploy, so outputs
    /// recover their commanded state after a restart.
    #[serde(default)]
    pub emit_on_start: bool,
}

impl ControlSourceConfig {
    /// One output port per control.
    pub fn port_count(&self) -> usize {
        self.controls.len()
    }

    /// Structural check shared by the save validation and the runtime build
    /// (existence in the org is checked at deploy).
    pub fn check(&self) -> Option<(&'static str, String)> {
        if self.controls.is_empty() {
            return Some((
                "control_source_empty",
                "add at least one control to listen to".into(),
            ));
        }
        if self.controls.len() > CONTROL_SOURCE_MAX {
            return Some((
                "control_source_too_many",
                format!("at most {CONTROL_SOURCE_MAX} controls per node"),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for id in &self.controls {
            if id.is_nil() {
                return Some(("control_source_unset", "pick a control".into()));
            }
            if !seen.insert(*id) {
                return Some((
                    "control_source_duplicate",
                    format!("control {id} is listed twice"),
                ));
            }
        }
        None
    }
}

// ─────────────────────────────── API DTOs ───────────────────────────────

/// A deployed flow listening to a control (`control-source`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlListener {
    pub flow_id: i64,
    pub flow_name: String,
}

/// An org control.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiControl {
    pub id: Uuid,
    pub org_id: i64,
    pub key: String,
    pub label: String,
    pub spec: ControlSpec,
    /// Deployed flows listening to it; empty = writing it has no effect
    /// (the UI shows a "no effect" badge).
    #[serde(default)]
    pub listened_by: Vec<ControlListener>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateUiControl {
    pub key: String,
    pub label: String,
    pub spec: ControlSpec,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UpdateUiControl {
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub spec: Option<ControlSpec>,
}

/// `POST /api/v1/controls/{id}/value`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WriteControlValue {
    pub value: f64,
    #[serde(default)]
    pub via: Option<String>,
}

/// `POST /api/v1/controls/values`: batch read for a surface.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ControlValuesRequest {
    #[serde(default)]
    pub ids: Vec<Uuid>,
}

/// Values by control id (`null` = never written, or unknown in the org).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ControlValuesResponse {
    pub values: BTreeMap<Uuid, Option<ControlValue>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: ControlKind) -> ControlSpec {
        ControlSpec::new(kind)
    }

    #[test]
    fn switch_accepts_only_its_two_states() {
        let s = spec(ControlKind::Switch);
        assert_eq!(s.accepts(1.0), Ok(1.0));
        assert_eq!(s.accepts(0.0), Ok(0.0));
        assert_eq!(s.accepts(0.5).unwrap_err().code(), "not_a_state");
        let custom = ControlSpec {
            on: Some(255.0),
            off: Some(10.0),
            ..spec(ControlKind::Switch)
        };
        assert_eq!(custom.accepts(255.0), Ok(255.0));
        assert!(custom.accepts(1.0).is_err());
    }

    #[test]
    fn slider_defaults_to_pwm_duty_and_snaps_float_noise() {
        let s = spec(ControlKind::Slider);
        assert_eq!(s.accepts(0.0), Ok(0.0));
        assert_eq!(s.accepts(100.0), Ok(100.0));
        assert_eq!(s.accepts(101.0).unwrap_err().code(), "out_of_range");
        assert_eq!(s.accepts(-1.0).unwrap_err().code(), "out_of_range");
        assert_eq!(s.accepts(42.5).unwrap_err().code(), "off_step");
        let fine = ControlSpec {
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.1),
            ..spec(ControlKind::Slider)
        };
        let v = fine.accepts(0.1 + 0.2).unwrap();
        assert!((v - 0.3).abs() < 1e-12);
        assert!(fine.accepts(1.0).unwrap() <= 1.0);
    }

    #[test]
    fn button_and_number_rules() {
        let b = spec(ControlKind::Button);
        assert_eq!(b.accepts(1.0), Ok(1.0));
        assert_eq!(b.accepts(0.0).unwrap_err().code(), "not_the_press");
        let n = spec(ControlKind::Number);
        assert_eq!(n.accepts(-1e9), Ok(-1e9));
        assert_eq!(n.accepts(f64::NAN).unwrap_err().code(), "not_finite");
        let bounded = ControlSpec {
            min: Some(5.0),
            max: Some(30.0),
            ..spec(ControlKind::Number)
        };
        assert_eq!(bounded.accepts(21.7), Ok(21.7));
        assert!(bounded.accepts(31.0).is_err());
    }

    #[test]
    fn spec_check_catches_degenerate_domains() {
        assert!(spec(ControlKind::Slider).check().is_none());
        let same = ControlSpec {
            on: Some(0.0),
            ..spec(ControlKind::Switch)
        };
        assert_eq!(same.check().unwrap().0, "control_spec_switch_same");
        let inverted = ControlSpec {
            min: Some(10.0),
            max: Some(10.0),
            ..spec(ControlKind::Slider)
        };
        assert_eq!(inverted.check().unwrap().0, "control_spec_range");
        let zero_step = ControlSpec {
            step: Some(0.0),
            ..spec(ControlKind::Slider)
        };
        assert_eq!(zero_step.check().unwrap().0, "control_spec_step");
        let huge = ControlSpec {
            step: Some(1e-6),
            ..spec(ControlKind::Slider)
        };
        assert_eq!(huge.check().unwrap().0, "control_spec_step");
    }

    #[test]
    fn source_config_check_and_ports() {
        let empty = ControlSourceConfig::default();
        assert_eq!(empty.check().unwrap().0, "control_source_empty");
        let a = Uuid::from_u128(1);
        let dup = ControlSourceConfig {
            controls: vec![a, a],
            emit_on_start: false,
        };
        assert_eq!(dup.check().unwrap().0, "control_source_duplicate");
        let nil = ControlSourceConfig {
            controls: vec![Uuid::nil()],
            emit_on_start: false,
        };
        assert_eq!(nil.check().unwrap().0, "control_source_unset");
        let ok = ControlSourceConfig {
            controls: vec![a, Uuid::from_u128(2)],
            emit_on_start: true,
        };
        assert!(ok.check().is_none());
        assert_eq!(ok.port_count(), 2);
    }

    #[test]
    fn keys_channel_and_via() {
        let id = Uuid::from_u128(7);
        assert_eq!(control_value_key(3, id), format!("pnex:ctl:v1:3:{id}"));
        assert_eq!(control_channel(3), "pnex:ctl:v1:3");
        assert!(valid_control_key("salle.machine-light_1"));
        assert!(!valid_control_key("bad key"));
        assert!(valid_control_via(&format!("dashboard:{id}")));
        assert!(!valid_control_via("dashboard {x}"));
        assert!(check_control_label("Éclairage salle").is_none());
        assert!(check_control_label("  ").is_some());
    }

    #[test]
    fn spec_roundtrip_skips_unset_fields() {
        let s = spec(ControlKind::Slider);
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"kind": "slider", "confirm": false})
        );
        let back: ControlSpec = serde_json::from_value(json).unwrap();
        assert_eq!(back, s);
    }
}
