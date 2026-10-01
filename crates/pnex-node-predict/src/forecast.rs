//! `pnex-forecast` — forecasts a series and, with a threshold, predicts
//! when it will be breached. Port 0 = detail object, port 1 = boolean
//! breach (mean forecast crosses within the horizon, or already crossed),
//! port 2 = seconds until the predicted breach (only when one is predicted).

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use pnex_core::predictive::{BreachDirection, ForecastConfig, ForecastModel, FORECAST_PORT_COUNT};

use crate::algo;
use crate::series::{now_secs, Sample, Series, SeriesStore};
use crate::ArtifactMeta;

const NODE: &str = "pnex-forecast";

#[flow_node("pnex-forecast", red_name = "pnex-forecast")]
struct ForecastNode {
    base: BaseFlowNodeState,
    config: ForecastConfig,
    store: SeriesStore,
}

impl std::fmt::Debug for ForecastNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForecastNode")
            .field("config", &self.config)
            .finish()
    }
}

/// Outcome of one evaluation: `None` = not a refit sample (nothing emitted).
pub(crate) fn evaluate(
    c: &ForecastConfig,
    s: &Series,
) -> std::result::Result<Option<serde_json::Value>, String> {
    let values = s.values();
    let x = *values.last().ok_or("empty series")?;
    if values.len() < c.min_samples as usize {
        return Ok(Some(json!({
            "value": x,
            "breach": false,
            "warming_up": true,
            "samples": values.len(),
            "min_samples": c.min_samples,
            "model": c.model,
        })));
    }
    if !s.received.is_multiple_of(c.every as u64) {
        return Ok(None);
    }
    let horizon = c.horizon as usize;
    let points = match c.model {
        ForecastModel::Ets => {
            algo::forecast_ets(&values, horizon, c.level, c.season_length as usize)?
        }
        ForecastModel::Linear => algo::forecast_linear(&values, horizon, c.level)?,
    };
    let times = s.times();
    // One step = the median sampling interval (1 s when unknown).
    let step = algo::median_step(&times).unwrap_or(1.0);
    let t_last = times.last().copied().unwrap_or_else(now_secs);
    let ms = |steps: usize| ((t_last + steps as f64 * step) * 1000.0).round();
    let mut detail = json!({
        "value": x,
        "warming_up": false,
        "samples": values.len(),
        "model": c.model,
        "level": c.level,
        "step_secs": step,
        "horizon_secs": step * horizon as f64,
        "points": points.iter().enumerate().map(|(i, p)| json!({
            "ts": ms(i + 1),
            "mean": p.mean,
            "lower": p.lower,
            "upper": p.upper,
        })).collect::<Vec<_>>(),
        "breach": false,
    });
    if let Some(thr) = c.threshold {
        let d = c.direction;
        let crosses = |v: f64| d.breaches(v, thr);
        // Already on the breach side = breach now (0 steps).
        let steps = if crosses(x) {
            Some(0)
        } else {
            algo::first_breach(points.iter().map(|p| p.mean), crosses)
        };
        // Earliest plausible breach: the interval bound on the breach side.
        let bound = |p: &algo::Point| {
            if d == BreachDirection::Above {
                p.upper
            } else {
                p.lower
            }
        };
        let earliest = if crosses(x) {
            Some(0)
        } else {
            algo::first_breach(points.iter().map(bound), crosses)
        };
        detail["threshold"] = json!(thr);
        detail["direction"] = json!(d);
        detail["breach"] = json!(steps.is_some());
        detail["breach_in_steps"] = json!(steps);
        detail["breach_in_secs"] = json!(steps.map(|n| n as f64 * step));
        detail["breach_at"] = json!(steps.map(ms));
        detail["earliest_breach_in_secs"] = json!(earliest.map(|n| n as f64 * step));
    }
    Ok(Some(detail))
}

impl ForecastNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = ForecastConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        if let Some((code, message)) = cfg.check() {
            return Err(EdgelinkError::BadFlowsJson(format!("{NODE} [{code}] : {message}")).into());
        }
        let meta = ArtifactMeta::deserialize(&config.rest).unwrap_or_default();
        let node_id = if meta.pnex_node_id.is_empty() {
            base_node.id.to_string()
        } else {
            meta.pnex_node_id
        };
        let store = SeriesStore::from_env(
            NODE,
            cfg.window as usize,
            meta.pnex_org_id,
            meta.pnex_flow_id,
            &node_id,
        );
        Ok(Box::new(ForecastNode {
            base: base_node,
            config: cfg,
            store,
        }))
    }

    async fn execute(&self, msg: MsgHandle, cancel: CancellationToken) -> Result<()> {
        let (topic, value) = crate::read_input(NODE, &msg, &self.config.key).await?;
        let sample = Sample {
            t: now_secs(),
            v: value,
        };
        let detail = self
            .store
            .push(&topic, sample, |s| evaluate(&self.config, s))
            .await
            .map_err(|e| {
                EdgelinkError::InvalidOperation(format!("{NODE} [{}] : {e}", self.name()))
            })?;
        let Some(detail) = detail else { return Ok(()) };
        let mut extra = vec![(1, detail["breach"].clone())];
        if let Some(secs) = detail.get("breach_in_secs").filter(|v| v.is_number()) {
            extra.push((2, secs.clone()));
        }
        let port_count = self.get_base().ports.len().min(FORECAST_PORT_COUNT);
        let envs = crate::envelopes(NODE, msg, &topic, detail, extra, port_count).await?;
        self.fan_out_many(envs, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for ForecastNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &ForecastNode, msg: MsgHandle| async move {
                    let r = node.execute(msg, cancel.child_token()).await;
                    if let Err(e) = &r {
                        log::warn!("{NODE} [{}] : message rejected : {e}", node.name());
                    }
                    r
                },
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bearing heating 0.01 °C per minute from 60 °C, sampled every minute.
    fn heating(n: usize) -> Series {
        let mut s = Series::default();
        for i in 0..n {
            let noise = (i * 7919 % 97) as f64 / 97.0 - 0.5;
            s.samples.push_back(Sample {
                t: 1_000.0 + i as f64 * 60.0,
                v: 60.0 + 0.01 * i as f64 + 0.2 * noise,
            });
        }
        s.received = n as u64;
        s
    }

    #[test]
    fn linear_predicts_breach_eta() {
        let c = ForecastConfig {
            model: ForecastModel::Linear,
            horizon: 600,
            threshold: Some(65.0),
            ..Default::default()
        };
        // 300 samples: now ≈ 63 °C, 65 °C reached ≈ 200 minutes later.
        let d = evaluate(&c, &heating(300)).unwrap().unwrap();
        assert_eq!(d["breach"], true, "{d}");
        assert_eq!(d["step_secs"], 60.0);
        let eta = d["breach_in_secs"].as_f64().unwrap();
        assert!((eta - 201.0 * 60.0).abs() < 20.0 * 60.0, "eta {eta}");
        assert!(d["earliest_breach_in_secs"].as_f64().unwrap() <= eta);
        assert_eq!(d["points"].as_array().unwrap().len(), 600);
    }

    #[test]
    fn no_breach_within_horizon() {
        let c = ForecastConfig {
            model: ForecastModel::Linear,
            horizon: 10,
            threshold: Some(65.0),
            ..Default::default()
        };
        let d = evaluate(&c, &heating(300)).unwrap().unwrap();
        assert_eq!(d["breach"], false);
        assert!(d["breach_in_secs"].is_null());
    }

    #[test]
    fn already_breached_and_below_direction() {
        let c = ForecastConfig {
            model: ForecastModel::Linear,
            threshold: Some(61.0),
            ..Default::default()
        };
        let d = evaluate(&c, &heating(300)).unwrap().unwrap();
        assert_eq!(d["breach_in_secs"], 0.0);
        let c = ForecastConfig {
            direction: BreachDirection::Below,
            ..c
        };
        assert_eq!(
            evaluate(&c, &heating(300)).unwrap().unwrap()["breach"],
            false
        );
    }

    #[test]
    fn warm_up_and_refit_cadence() {
        let c = ForecastConfig {
            every: 5,
            ..Default::default()
        };
        assert_eq!(
            evaluate(&c, &heating(10)).unwrap().unwrap()["warming_up"],
            true
        );
        let mut s = heating(100);
        s.received = 101;
        assert!(evaluate(&c, &s).unwrap().is_none());
        s.received = 100;
        let d = evaluate(&c, &s).unwrap().unwrap();
        assert_eq!(d["model"], "ets");
        assert!(d.get("threshold").is_none());
    }
}
