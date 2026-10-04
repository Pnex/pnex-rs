//! `pnex-weather` — timed weather source (D140,
//! `docs/architecture/home-dashboards.md`).
//!
//! No input. Every `interval_min` minutes (and once at start with
//! `emit_on_start`) the node fetches the forecast of its coordinates from an
//! allowlisted provider (the URL is built by `pnex_core::weather`, never
//! typed by the user, R8) and emits three normalized messages:
//! port 0 = current conditions, port 1 = 7-day daily forecast, port 2 =
//! 48-hour hourly forecast (`topic` = `weather.current|daily|hourly`).
//! Wire them to `memory-write` (live, Valkey) or `pnex-metric` (series,
//! O2: one series per numeric field) to reuse the weather everywhere.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use edgelink_core::runtime::context::*;
use edgelink_core::runtime::debug_channel::DebugMessage;
use edgelink_core::runtime::flow::*;
use edgelink_core::runtime::model::json::*;
use edgelink_core::runtime::model::*;
use edgelink_core::runtime::nodes::*;
use edgelink_core::{EdgelinkError, Result};
use edgelink_macro::*;

use pnex_core::vision::{NodeStatus, NodeStatusLevel, NODE_STATUS_FORMAT};
use pnex_core::weather::{parse_weather, WeatherConfig, WeatherPayloads};

const NODE: &str = "pnex-weather";
/// Bound of one provider call.
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Retry delay after a failed fetch (shorter than the interval).
const RETRY_DELAY: Duration = Duration::from_secs(120);
/// Heartbeat period of the node status (D103 school).
const STATUS_PERIOD: Duration = Duration::from_secs(10);
/// Identified User-Agent, required by the MET Norway terms of service.
const USER_AGENT: &str = concat!(
    "PneX/",
    env!("CARGO_PKG_VERSION"),
    " (+https://pnex.io) pnex-weather"
);

/// Anti-stripping anchor: the runtime binary calls it so the linker keeps
/// the `inventory` submission of the node.
pub fn registered() {}

#[derive(Debug, Deserialize)]
struct NodeConfig {
    #[serde(flatten)]
    weather: WeatherConfig,
    /// Canvas id (status badge matching).
    #[serde(default)]
    pnex_node_id: String,
}

#[derive(Default)]
struct Stats {
    fetches: AtomicU64,
    last_ok_ms: AtomicI64,
    error: Mutex<Option<String>>,
}

#[flow_node("pnex-weather", red_name = "pnex-weather")]
struct WeatherNode {
    base: BaseFlowNodeState,
    config: NodeConfig,
    http: reqwest::Client,
    stats: Stats,
}

/// The three messages of one fetch, in port order.
fn messages(p: WeatherPayloads) -> Vec<(usize, BTreeMap<String, Variant>)> {
    [
        (0, "weather.current", p.current),
        (1, "weather.daily", p.daily),
        (2, "weather.hourly", p.hourly),
    ]
    .into_iter()
    .map(|(port, topic, payload)| {
        let mut body = BTreeMap::new();
        body.insert("payload".to_string(), Variant::from(payload));
        body.insert("topic".to_string(), Variant::from(topic));
        (port, body)
    })
    .collect()
}

impl WeatherNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = NodeConfig::deserialize(&config.rest)
            .map_err(|e| EdgelinkError::BadFlowsJson(format!("{NODE} : invalid config : {e}")))?;
        if let Some((code, message)) = cfg.weather.check() {
            return Err(EdgelinkError::BadFlowsJson(format!("{NODE} [{code}] : {message}")).into());
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(FETCH_TIMEOUT)
            // Fixed provider hosts: never follow a redirect elsewhere.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| EdgelinkError::InvalidOperation(format!("{NODE} : http client: {e}")))?;
        Ok(Box::new(WeatherNode {
            base: base_node,
            config: cfg,
            http,
            stats: Stats::default(),
        }))
    }

    async fn fetch(&self) -> std::result::Result<WeatherPayloads, String> {
        let url = self.config.weather.url();
        let res = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        let status = res.status();
        if !status.is_success() {
            return Err(format!("provider answered HTTP {}", status.as_u16()));
        }
        let body: serde_json::Value = res
            .json()
            .await
            .map_err(|e| format!("unreadable response: {e}"))?;
        parse_weather(self.config.weather.provider, &body).map_err(|e| e.0)
    }

    /// One fetch + emission; returns whether it succeeded.
    async fn tick(&self, stop: &CancellationToken) -> bool {
        self.stats.fetches.fetch_add(1, Ordering::Relaxed);
        match self.fetch().await {
            Ok(p) => {
                *self.stats.error.lock().expect("stats") = None;
                self.stats
                    .last_ok_ms
                    .store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
                for (port, body) in messages(p) {
                    let envelope = Envelope {
                        port,
                        msg: MsgHandle::with_properties(body),
                    };
                    if let Err(e) = self.fan_out_one(envelope, stop.child_token()).await {
                        log::warn!("{NODE} [{}] : fan-out failed: {e}", self.name());
                    }
                }
                true
            }
            Err(e) => {
                log::warn!("{NODE} [{}] : {e}", self.name());
                *self.stats.error.lock().expect("stats") = Some(e);
                false
            }
        }
    }

    fn status(&self) -> NodeStatus {
        let s = &self.stats;
        let base = match s.error.lock().expect("stats").clone() {
            Some(err) => NodeStatus::new(NodeStatusLevel::Error, "weather-unavailable").detail(err),
            None if s.last_ok_ms.load(Ordering::Relaxed) == 0 => {
                NodeStatus::new(NodeStatusLevel::Ok, "weather-waiting")
            }
            None => NodeStatus::new(NodeStatusLevel::Ok, "weather-updated"),
        };
        base.stat("fetches", s.fetches.load(Ordering::Relaxed))
    }

    /// Publishes the status on the engine debug channel (D103 school).
    fn publish_status(&self) {
        let Some(engine) = self.engine() else {
            return;
        };
        let msg = serde_json::to_value(self.status()).unwrap_or(serde_json::Value::Null);
        let name = self.name();
        engine.debug_channel().send(DebugMessage {
            id: self.config.pnex_node_id.clone(),
            name: if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            },
            msg,
            property: Some("payload".into()),
            format: Some(NODE_STATUS_FORMAT.into()),
            path: self
                .flow()
                .map(|f| f.get_path())
                .unwrap_or_else(|| "global".to_string()),
            topic: None,
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
            msgid: None,
        });
    }
}

#[async_trait]
impl FlowNodeBehavior for WeatherNode {
    fn get_base(&self) -> &BaseFlowNodeState {
        &self.base
    }

    async fn run(self: Arc<Self>, stop_token: CancellationToken) {
        {
            let node = self.clone();
            let stop = stop_token.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = stop.cancelled() => break,
                        _ = tokio::time::sleep(STATUS_PERIOD) => {}
                    }
                    node.publish_status();
                }
            });
        }
        let interval = Duration::from_secs(u64::from(self.config.weather.interval_min) * 60);
        let mut wait = if self.config.weather.emit_on_start {
            Duration::ZERO
        } else {
            interval
        };
        loop {
            tokio::select! {
                _ = stop_token.cancelled() => break,
                _ = tokio::time::sleep(wait) => {}
            }
            let ok = self.tick(&stop_token).await;
            self.publish_status();
            wait = if ok {
                interval
            } else {
                RETRY_DELAY.min(interval)
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_messages_in_port_order_with_topics() {
        let p = WeatherPayloads {
            current: serde_json::json!({"temperature": 12.0}),
            daily: serde_json::json!({"d0_t_max": 15.0}),
            hourly: serde_json::json!({"h0_temperature": 11.0}),
        };
        let msgs = messages(p);
        let ports: Vec<usize> = msgs.iter().map(|(p, _)| *p).collect();
        assert_eq!(ports, vec![0, 1, 2]);
        let topic = serde_json::to_value(&msgs[1].1["topic"]).unwrap();
        assert_eq!(topic, "weather.daily");
        let payload = serde_json::to_value(&msgs[0].1["payload"]).unwrap();
        assert_eq!(payload["temperature"], 12.0);
    }

    #[test]
    fn user_agent_identifies_the_platform() {
        assert!(USER_AGENT.starts_with("PneX/") && USER_AGENT.contains("pnex.io"));
    }

    /// Live call to both providers (network): `cargo test -p
    /// pnex-node-weather -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn live_providers_parse() {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(FETCH_TIMEOUT)
            .build()
            .unwrap();
        for provider in [
            pnex_core::weather::WeatherProvider::MetNorway,
            pnex_core::weather::WeatherProvider::OpenMeteo,
        ] {
            let cfg = WeatherConfig {
                provider,
                ..Default::default()
            };
            let body: serde_json::Value = http
                .get(cfg.url())
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let p = parse_weather(provider, &body).unwrap();
            assert!(
                p.current["temperature"].is_number(),
                "{provider:?}: {}",
                p.current
            );
            assert!(p.daily["d0_t_max"].is_number(), "{provider:?}: {}", p.daily);
            assert!(p.hourly["h0_temperature"].is_number(), "{provider:?}");
            println!("{provider:?} current = {}", p.current);
            println!("{provider:?} d0 = {}", p.daily["days"][0]);
        }
    }
}
