//! Predictive telemetry contract (ml-vision.md step 3, roadmap P2.3):
//! `anomaly` and `forecast` flow node configs, shared by the editor
//! (validation, port counts) and the runtime (`pnex-node-predict`).
//! Pure, wasm-safe — the algorithms (augurs) live in the runtime crate.
//!
//! Both nodes consume one numeric series per `msg.topic` (a scalar payload,
//! or the `key` field of an object payload) and keep a bounded sliding
//! window of samples, persisted in Valkey when available so a redeploy or
//! a runtime restart keeps the history.

use serde::{Deserialize, Serialize};

/// Sliding window bounds (samples per series).
pub const PREDICT_WINDOW_MIN: u32 = 16;
pub const PREDICT_WINDOW_MAX: u32 = 5000;
/// Smallest warm-up accepted before a node starts scoring / forecasting.
pub const PREDICT_MIN_SAMPLES_FLOOR: u32 = 8;
/// Distinct series (`msg.topic` values) tracked by one node; the least
/// recently updated series is evicted beyond this.
pub const PREDICT_MAX_SERIES: usize = 64;
/// Forecast horizon bound (steps).
pub const FORECAST_HORIZON_MAX: u32 = 1000;
/// Anomaly node output ports: 0 = detail object, 1 = boolean state.
pub const ANOMALY_PORT_COUNT: usize = 2;
/// Forecast node output ports: 0 = detail object, 1 = boolean breach,
/// 2 = seconds until the predicted breach (only when one is predicted).
pub const FORECAST_PORT_COUNT: usize = 3;

/// Anomaly scoring method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnomalyMethod {
    /// Robust z-score against the window median / MAD: no model, no
    /// tuning, resistant to the outliers it is looking for.
    #[default]
    RobustZ,
    /// One-step-ahead forecast (ETS, MSTL when seasonal): the sample is
    /// anomalous when it falls outside the prediction interval.
    ForecastBand,
    /// Bayesian online changepoint detection: flags a regime change
    /// (level / variance shift), not a single outlier.
    Changepoint,
}

impl AnomalyMethod {
    pub const ALL: [AnomalyMethod; 3] = [Self::RobustZ, Self::ForecastBand, Self::Changepoint];

    pub fn wire(self) -> &'static str {
        match self {
            Self::RobustZ => "robust_z",
            Self::ForecastBand => "forecast_band",
            Self::Changepoint => "changepoint",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.wire() == s)
    }
}

/// Forecast model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForecastModel {
    /// AutoETS (exponential smoothing, model picked by AICc); MSTL
    /// decomposition first when `season_length` > 0.
    #[default]
    Ets,
    /// Least-squares linear trend with a prediction interval — the robust
    /// choice for slow wear / fouling drifts.
    Linear,
}

impl ForecastModel {
    pub const ALL: [ForecastModel; 2] = [Self::Ets, Self::Linear];

    pub fn wire(self) -> &'static str {
        match self {
            Self::Ets => "ets",
            Self::Linear => "linear",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.wire() == s)
    }
}

/// Which side of the threshold is a breach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreachDirection {
    #[default]
    Above,
    Below,
}

impl BreachDirection {
    /// True when `v` is on the breach side of `threshold`.
    pub fn breaches(self, v: f64, threshold: f64) -> bool {
        match self {
            Self::Above => v >= threshold,
            Self::Below => v <= threshold,
        }
    }
}

fn default_anomaly_window() -> u32 {
    200
}
fn default_anomaly_min_samples() -> u32 {
    30
}
fn default_z_threshold() -> f64 {
    3.5
}
fn default_band_level() -> f64 {
    0.99
}
fn default_hazard() -> f64 {
    250.0
}
fn default_forecast_window() -> u32 {
    500
}
fn default_forecast_min_samples() -> u32 {
    48
}
/// Default horizon: 300 steps = 5 min at 1 Hz, 5 h at one sample a minute
/// (48 steps was under a minute on a 1 Hz series, useless for slow drifts).
pub const FORECAST_DEFAULT_HORIZON: u32 = 300;

fn default_horizon() -> u32 {
    FORECAST_DEFAULT_HORIZON
}
fn default_forecast_level() -> f64 {
    0.95
}
fn default_every() -> u32 {
    1
}

/// `pnex-anomaly` node config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnomalyConfig {
    #[serde(default)]
    pub method: AnomalyMethod,
    /// Field read from an object payload (empty = the payload is the value).
    #[serde(default)]
    pub key: String,
    #[serde(default = "default_anomaly_window")]
    pub window: u32,
    /// Samples collected before scoring starts (warm-up).
    #[serde(default = "default_anomaly_min_samples")]
    pub min_samples: u32,
    /// `robust_z`: |z| at or above this is anomalous.
    #[serde(default = "default_z_threshold")]
    pub threshold: f64,
    /// `forecast_band`: prediction interval level.
    #[serde(default = "default_band_level")]
    pub level: f64,
    /// `changepoint`: expected run length between changes (samples).
    #[serde(default = "default_hazard")]
    pub hazard: f64,
    /// `forecast_band`: season period in samples (0 = none).
    #[serde(default)]
    pub season_length: u32,
}

impl Default for AnomalyConfig {
    fn default() -> Self {
        Self {
            method: AnomalyMethod::default(),
            key: String::new(),
            window: default_anomaly_window(),
            min_samples: default_anomaly_min_samples(),
            threshold: default_z_threshold(),
            level: default_band_level(),
            hazard: default_hazard(),
            season_length: 0,
        }
    }
}

/// `pnex-forecast` node config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForecastConfig {
    #[serde(default)]
    pub model: ForecastModel,
    #[serde(default)]
    pub key: String,
    #[serde(default = "default_forecast_window")]
    pub window: u32,
    #[serde(default = "default_forecast_min_samples")]
    pub min_samples: u32,
    /// Steps forecast ahead (one step = the median sampling interval).
    #[serde(default = "default_horizon")]
    pub horizon: u32,
    /// `ets`: season period in samples (0 = none).
    #[serde(default)]
    pub season_length: u32,
    #[serde(default = "default_forecast_level")]
    pub level: f64,
    /// Breach threshold (none = forecast only, no breach detection).
    #[serde(default)]
    pub threshold: Option<f64>,
    #[serde(default)]
    pub direction: BreachDirection,
    /// Refit every N samples (1 = every sample).
    #[serde(default = "default_every")]
    pub every: u32,
}

impl Default for ForecastConfig {
    fn default() -> Self {
        Self {
            model: ForecastModel::default(),
            key: String::new(),
            window: default_forecast_window(),
            min_samples: default_forecast_min_samples(),
            horizon: default_horizon(),
            season_length: 0,
            level: default_forecast_level(),
            threshold: None,
            direction: BreachDirection::default(),
            every: default_every(),
        }
    }
}

type Violation = Option<(&'static str, String)>;

fn check_window(window: u32, min_samples: u32) -> Violation {
    if !(PREDICT_WINDOW_MIN..=PREDICT_WINDOW_MAX).contains(&window) {
        return Some((
            "predict_window_invalid",
            format!("window must be between {PREDICT_WINDOW_MIN} and {PREDICT_WINDOW_MAX} samples"),
        ));
    }
    if min_samples < PREDICT_MIN_SAMPLES_FLOOR || min_samples > window {
        return Some((
            "predict_min_samples_invalid",
            format!(
                "warm-up must be between {PREDICT_MIN_SAMPLES_FLOOR} samples and the window size"
            ),
        ));
    }
    None
}

fn check_season(season_length: u32, min_samples: u32) -> Violation {
    // MSTL needs at least two full periods before it can decompose.
    if season_length == 1 || (season_length > 0 && season_length * 2 > min_samples) {
        return Some((
            "predict_season_invalid",
            "season must be 0 (none) or at least 2 samples, with a warm-up of two full seasons"
                .into(),
        ));
    }
    None
}

fn check_level(level: f64) -> Violation {
    if !level.is_finite() || !(0.5..=0.999).contains(&level) {
        return Some((
            "predict_level_invalid",
            "confidence level must be between 0.5 and 0.999".into(),
        ));
    }
    None
}

impl AnomalyConfig {
    /// First violation (machine code + English message), `None` when valid.
    pub fn check(&self) -> Violation {
        check_window(self.window, self.min_samples).or_else(|| match self.method {
            AnomalyMethod::RobustZ => {
                (!self.threshold.is_finite() || self.threshold <= 0.0).then(|| {
                    (
                        "anomaly_threshold_invalid",
                        "threshold must be a positive number".into(),
                    )
                })
            }
            AnomalyMethod::ForecastBand => check_level(self.level)
                .or_else(|| check_season(self.season_length, self.min_samples)),
            AnomalyMethod::Changepoint => {
                (!self.hazard.is_finite() || self.hazard < 2.0).then(|| {
                    (
                        "anomaly_hazard_invalid",
                        "expected run length must be at least 2 samples".into(),
                    )
                })
            }
        })
    }
}

impl ForecastConfig {
    /// First violation (machine code + English message), `None` when valid.
    pub fn check(&self) -> Violation {
        if let Some(v) = check_window(self.window, self.min_samples) {
            return Some(v);
        }
        if self.horizon == 0 || self.horizon > FORECAST_HORIZON_MAX {
            return Some((
                "forecast_horizon_invalid",
                format!("horizon must be between 1 and {FORECAST_HORIZON_MAX} steps"),
            ));
        }
        if let Some(v) = check_level(self.level) {
            return Some(v);
        }
        if self.model == ForecastModel::Ets {
            if let Some(v) = check_season(self.season_length, self.min_samples) {
                return Some(v);
            }
        }
        if self.threshold.is_some_and(|t| !t.is_finite()) {
            return Some((
                "forecast_threshold_invalid",
                "threshold must be a number".into(),
            ));
        }
        if self.every == 0 || self.every > 1000 {
            return Some((
                "forecast_every_invalid",
                "refit period must be between 1 and 1000 samples".into(),
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert_eq!(AnomalyConfig::default().check(), None);
        assert_eq!(ForecastConfig::default().check(), None);
        for m in AnomalyMethod::ALL {
            let c = AnomalyConfig {
                method: m,
                ..Default::default()
            };
            assert_eq!(c.check(), None, "{m:?}");
        }
    }

    #[test]
    fn empty_json_takes_defaults() {
        let a: AnomalyConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(a, AnomalyConfig::default());
        let f: ForecastConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(f, ForecastConfig::default());
    }

    #[test]
    fn wire_roundtrip() {
        for m in AnomalyMethod::ALL {
            assert_eq!(AnomalyMethod::from_wire(m.wire()), Some(m));
            assert_eq!(serde_json::to_value(m).unwrap(), m.wire());
        }
        for m in ForecastModel::ALL {
            assert_eq!(ForecastModel::from_wire(m.wire()), Some(m));
        }
    }

    #[test]
    fn rejects_bad_bounds() {
        let c = AnomalyConfig {
            window: 4,
            ..Default::default()
        };
        assert_eq!(c.check().unwrap().0, "predict_window_invalid");
        let c = AnomalyConfig {
            min_samples: 500,
            ..Default::default()
        };
        assert_eq!(c.check().unwrap().0, "predict_min_samples_invalid");
        let c = AnomalyConfig {
            threshold: 0.0,
            ..Default::default()
        };
        assert_eq!(c.check().unwrap().0, "anomaly_threshold_invalid");
        let c = AnomalyConfig {
            method: AnomalyMethod::ForecastBand,
            season_length: 24,
            ..Default::default()
        };
        assert_eq!(c.check().unwrap().0, "predict_season_invalid");
        let c = ForecastConfig {
            horizon: 0,
            ..Default::default()
        };
        assert_eq!(c.check().unwrap().0, "forecast_horizon_invalid");
        let c = ForecastConfig {
            level: 1.0,
            ..Default::default()
        };
        assert_eq!(c.check().unwrap().0, "predict_level_invalid");
        let c = ForecastConfig {
            threshold: Some(f64::NAN),
            ..Default::default()
        };
        assert_eq!(c.check().unwrap().0, "forecast_threshold_invalid");
        let c = ForecastConfig {
            every: 0,
            ..Default::default()
        };
        assert_eq!(c.check().unwrap().0, "forecast_every_invalid");
    }

    #[test]
    fn season_ignored_by_linear_model() {
        let c = ForecastConfig {
            model: ForecastModel::Linear,
            season_length: 1,
            ..Default::default()
        };
        assert_eq!(c.check(), None);
    }

    #[test]
    fn breach_direction() {
        assert!(BreachDirection::Above.breaches(10.0, 10.0));
        assert!(!BreachDirection::Above.breaches(9.9, 10.0));
        assert!(BreachDirection::Below.breaches(9.9, 10.0));
    }
}
