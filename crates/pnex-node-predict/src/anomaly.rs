//! `pnex-anomaly` — scores each sample of a series against its window.
//! Port 0 = detail object, port 1 = boolean anomaly state (a `pnex-notify`
//! trigger fires on its rising edge).

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

use pnex_core::predictive::{AnomalyConfig, AnomalyMethod, ANOMALY_PORT_COUNT};

use crate::algo;
use crate::series::{now_secs, Sample, Series, SeriesStore};
use crate::ArtifactMeta;

const NODE: &str = "pnex-anomaly";

#[flow_node("pnex-anomaly", red_name = "pnex-anomaly")]
struct AnomalyNode {
    base: BaseFlowNodeState,
    config: AnomalyConfig,
    store: SeriesStore,
}

impl std::fmt::Debug for AnomalyNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnomalyNode")
            .field("config", &self.config)
            .finish()
    }
}

/// Scores the last sample of `s` (the one just pushed) against the rest of
/// the window. Pure over the series state: unit-tested without a runtime.
pub(crate) fn evaluate(
    c: &AnomalyConfig,
    s: &mut Series,
) -> std::result::Result<serde_json::Value, String> {
    let values = s.values();
    let x = *values.last().ok_or("empty series")?;
    let history = &values[..values.len() - 1];
    if history.len() < c.min_samples as usize {
        return Ok(json!({
            "value": x,
            "anomaly": false,
            "warming_up": true,
            "samples": history.len(),
            "min_samples": c.min_samples,
            "method": c.method,
        }));
    }
    let mut detail = json!({
        "value": x,
        "warming_up": false,
        "samples": history.len(),
        "method": c.method,
    });
    let score = match c.method {
        AnomalyMethod::RobustZ => algo::robust_z(history, x, c.threshold),
        AnomalyMethod::ForecastBand => {
            algo::forecast_band(history, x, c.level, c.season_length as usize)?
        }
        AnomalyMethod::Changepoint => {
            let latest = algo::changepoints(&values, c.hazard).last().copied();
            let abs = latest.map(|i| s.offset + i as u64);
            let fresh = s.changepoint_primed && abs.is_some() && abs > s.last_changepoint;
            s.changepoint_primed = true;
            if abs > s.last_changepoint {
                s.last_changepoint = abs;
            }
            if let Some(i) = latest {
                detail["changepoint_at"] = json!((s.samples[i].t * 1000.0).round());
                detail["samples_since_change"] = json!(values.len() - 1 - i);
            }
            // Level of the current regime vs the whole window.
            let regime = &values[latest.unwrap_or(0)..];
            let med = algo::median(regime);
            algo::Score {
                score: if fresh { 1.0 } else { 0.0 },
                anomaly: fresh,
                expected: med,
                lower: f64::NAN,
                upper: f64::NAN,
            }
        }
    };
    detail["anomaly"] = json!(score.anomaly);
    detail["score"] = json!(score.score);
    detail["expected"] = json!(score.expected);
    detail["lower"] = json!(score.lower);
    detail["upper"] = json!(score.upper);
    Ok(detail)
}

impl AnomalyNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = AnomalyConfig::deserialize(&config.rest)
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
        // The window holds the history plus the sample being scored.
        let store = SeriesStore::from_env(
            NODE,
            cfg.window as usize + 1,
            meta.pnex_org_id,
            meta.pnex_flow_id,
            &node_id,
        );
        Ok(Box::new(AnomalyNode {
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
        let flag = detail["anomaly"].clone();
        let port_count = self.get_base().ports.len().min(ANOMALY_PORT_COUNT);
        let envs = crate::envelopes(NODE, msg, &topic, detail, vec![(1, flag)], port_count).await?;
        self.fan_out_many(envs, cancel).await
    }
}

#[async_trait]
impl FlowNodeBehavior for AnomalyNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        while !stop_token.is_cancelled() {
            let cancel = stop_token.child_token();
            with_uow(
                self.as_ref(),
                cancel.child_token(),
                |node: &AnomalyNode, msg: MsgHandle| async move {
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

    fn series_of(values: &[f64]) -> Series {
        let mut s = Series::default();
        for (i, v) in values.iter().enumerate() {
            s.samples.push_back(Sample {
                t: i as f64 * 60.0,
                v: *v,
            });
        }
        s
    }

    fn noise(i: usize) -> f64 {
        (i * 7919 % 97) as f64 / 97.0 - 0.5
    }

    #[test]
    fn warm_up_then_spike() {
        let c = AnomalyConfig {
            min_samples: 10,
            ..Default::default()
        };
        let d = evaluate(&c, &mut series_of(&[1.0; 5])).unwrap();
        assert_eq!(d["warming_up"], true);
        assert_eq!(d["anomaly"], false);
        let mut v: Vec<f64> = (0..40).map(|i| 20.0 + noise(i)).collect();
        v.push(30.0);
        let d = evaluate(&c, &mut series_of(&v)).unwrap();
        assert_eq!(d["anomaly"], true, "{d}");
        assert!(d["score"].as_f64().unwrap() > 3.5);
    }

    #[test]
    fn changepoint_reported_once_and_not_on_priming() {
        let c = AnomalyConfig {
            method: AnomalyMethod::Changepoint,
            min_samples: 20,
            ..Default::default()
        };
        // A change already in the history at first scoring: recorded, silent.
        let mut v: Vec<f64> = (0..60).map(noise).collect();
        for x in v.iter_mut().skip(30) {
            *x += 6.0;
        }
        let mut s = series_of(&v);
        assert_eq!(evaluate(&c, &mut s).unwrap()["anomaly"], false);
        // A new shift appears: reported on the first sample that reveals it,
        // then silent again.
        let mut fired = 0;
        for i in 60..120 {
            let bump = if i >= 80 { 12.0 } else { 6.0 };
            s.samples.push_back(Sample {
                t: i as f64 * 60.0,
                v: noise(i) + bump,
            });
            if evaluate(&c, &mut s).unwrap()["anomaly"] == true {
                fired += 1;
            }
        }
        assert_eq!(fired, 1);
    }

    #[test]
    fn band_method_runs() {
        let c = AnomalyConfig {
            method: AnomalyMethod::ForecastBand,
            ..Default::default()
        };
        let mut v: Vec<f64> = (0..60).map(|i| 5.0 + noise(i)).collect();
        v.push(15.0);
        let d = evaluate(&c, &mut series_of(&v)).unwrap();
        assert_eq!(d["anomaly"], true, "{d}");
        assert!(d["upper"].as_f64().unwrap() < 15.0);
    }
}
