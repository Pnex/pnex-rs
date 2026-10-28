//! `pnex-device-write` node — writes the output pins of a single device
//! (digital 1/0, pwm duty 0..=100). The runtime never has direct access to the
//! device WS: the downlink goes through the backend internal route
//! `/internal/flow/device-write` (`/internal/notify/deliver` pattern), shared
//! token injected into the runtime env by the supervisor.
//!
//! - fail-loud build if the write coordinates are missing from the env
//!   (`PNEX_FLOW_WRITE_URL`/`_TOKEN`) — the deploy's `--check` preflight
//!   fails explicitly (pnex-notify pattern);
//! - incoming payload = a `{pin: value}` map (batch — only the configured
//!   pins present in the map are sent), or a **scalar** routed by
//!   `msg.topic` when the wire was drawn on a pin's input anchor (the deploy
//!   projection stamps the topic; one unit write per message), or a
//!   **scalar** on a single-pin node (direct wiring from a device-read
//!   per-pin port); anything else is dropped with a warn (passthrough).
//! - commands announced by a custom firmware (`commands`, D146) follow the
//!   same routing as pins (map key, or topic of their input anchor) and are
//!   sent as `commands: {name: value}`; the backend wraps the value into
//!   `args = {"value": …}`.
//! - lenient write: a pin in error = warn + continue (never a
//!   crash-loop); the message passes through (passthrough).
//! - Outputs: port 0 = incoming message (passthrough), last port =
//!   "device name" (payload = device slug) — explicit naming of the
//!   device for downstream, without a payload convention.

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
    /// Custom-firmware commands (D146), input anchors after the pins.
    #[serde(default)]
    commands: Vec<String>,
    /// Value declared per pin (O70): written whatever the message carries.
    #[serde(default)]
    values: std::collections::BTreeMap<String, serde_json::Value>,
    /// Stamped by the projection at deploy (`FlowArtifactMeta.org_id`) —
    /// passed as is to the internal route (it resolves the device within the org).
    #[serde(default)]
    pnex_org_id: i64,
    /// Canvas id, stamped by the projection (status badge matching).
    #[serde(default)]
    pnex_node_id: String,
}

#[flow_node("pnex-device-write", red_name = "pnex-device-write")]
struct DeviceWriteNode {
    base: BaseFlowNodeState,
    config: DeviceWriteNodeConfig,
    http: reqwest::Client,
    url: String,
    token: String,
    org_id: i64,
    status: crate::status::StatusBoard,
}

impl DeviceWriteNode {
    fn build(
        _flow: &Flow,
        base_node: BaseFlowNodeState,
        config: &RedFlowNodeConfig,
        _options: Option<&config::Config>,
    ) -> Result<Box<dyn FlowNodeBehavior>> {
        let cfg = DeviceWriteNodeConfig::deserialize(&config.rest).map_err(|e| {
            EdgelinkError::BadFlowsJson(format!("pnex-device-write: invalid config: {e}"))
        })?;
        if !pnex_core::valid_device_label(&cfg.device_id) {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "pnex-device-write: invalid device \"{}\" (slug required)",
                cfg.device_id
            ))
            .into());
        }
        if cfg.pins.is_empty() && cfg.commands.is_empty() {
            return Err(EdgelinkError::BadFlowsJson(
                "pnex-device-write: no pin or command selected".into(),
            )
            .into());
        }
        if cfg
            .commands
            .iter()
            .any(|c| !pnex_core::valid_command_name(c))
        {
            return Err(EdgelinkError::BadFlowsJson(format!(
                "pnex-device-write: invalid command name for \"{}\"",
                cfg.device_id
            ))
            .into());
        }
        for pin in &cfg.pins {
            if pin.trim().is_empty() {
                return Err(EdgelinkError::BadFlowsJson(format!(
                    "pnex-device-write: empty pin for \"{}\"",
                    cfg.device_id
                ))
                .into());
            }
        }
        if cfg.pnex_org_id <= 0 {
            return Err(EdgelinkError::BadFlowsJson(
                "pnex-device-write: pnex_org_id missing from the artefact (redeploy the flow)"
                    .into(),
            )
            .into());
        }
        // Coordinates injected by the supervisor (FlowSettings.device_write
        // → apply_runtime_env). Never in flows.json — missing ⇒ fail-loud at
        // build: the deploy preflight fails explicitly.
        let url = std::env::var("PNEX_FLOW_WRITE_URL").map_err(|_| {
            EdgelinkError::InvalidOperation(
                "pnex-device-write: PNEX_FLOW_WRITE_URL missing from the runtime \
                 environment (runtime_token not configured on the server?)"
                    .into(),
            )
        })?;
        let token = std::env::var("PNEX_FLOW_WRITE_TOKEN").map_err(|_| {
            EdgelinkError::InvalidOperation(
                "pnex-device-write: PNEX_FLOW_WRITE_TOKEN missing from the runtime \
                 environment"
                    .into(),
            )
        })?;
        let http = reqwest::Client::new();
        Ok(Box::new(DeviceWriteNode {
            base: base_node,
            org_id: cfg.pnex_org_id,
            status: crate::status::StatusBoard::new(
                cfg.pnex_node_id.clone(),
                "device-write-ok",
                "device-write-rejected",
            ),
            config: cfg,
            http,
            url,
            token,
        }))
    }
}

impl DeviceWriteNode {
    /// Nothing (or not everything) was written: server log + canvas badge.
    fn reject(&self, detail: String) {
        if self.status.log_due(&detail) {
            log::warn!("pnex-device-write [{}] : {detail}", self.name());
        }
        self.status.rejected(self, detail);
    }

    fn declared_for(
        &self,
        topic: Option<&str>,
    ) -> Option<serde_json::Map<String, serde_json::Value>> {
        declared_values(&self.config.values, self.anchors(), topic)
    }

    /// Input anchors: pins first, then commands.
    fn anchors(&self) -> impl Iterator<Item = &String> {
        self.config.pins.iter().chain(self.config.commands.iter())
    }

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
        // Declared values (O70): the message is only the trigger. A wire
        // drawn on a pin's anchor (topic = that pin) writes that pin's
        // declared value; any other message writes every declared value.
        if payload_value.as_object().is_none() {
            if let Some(fixed) = self.declared_for(topic.as_deref()) {
                return self.write_values(msg, cancel, fixed).await;
            }
        }
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
                    self.reject("the message carries no value (empty payload)".into());
                    return self.fan_out_done(msg, cancel).await;
                }
                _ => serde_json::Value::Null,
            };
            if !scalar.is_null() {
                if let Some(topic) = topic.as_deref() {
                    if let Some(name) = self.anchors().find(|p| p.as_str() == topic) {
                        let mut single = serde_json::Map::new();
                        single.insert(name.clone(), scalar);
                        return self.write_values(msg, cancel, single).await;
                    }
                    self.reject(format!(
                        "topic \"{topic}\" matches no configured pin or command ({}): \
                         nothing written",
                        self.anchors().cloned().collect::<Vec<_>>().join(", ")
                    ));
                    return self.fan_out_done(msg, cancel).await;
                }
                let mut anchors = self.anchors();
                if let (Some(only), None) = (anchors.next(), anchors.next()) {
                    let mut single = serde_json::Map::new();
                    single.insert(only.clone(), scalar);
                    return self.write_values(msg, cancel, single).await;
                }
            }
            self.reject(if self.anchors().count() > 1 {
                "one value for several outputs: wire each output on its own input, \
                 or send a map {output: value}"
                    .into()
            } else {
                "the value is neither a number, a boolean nor a text: nothing written".into()
            });
            return self.fan_out_done(msg, cancel).await;
        }
        // Variant → JSON map once; filtering on the configured pins
        // lives in `write_values`.
        let obj: serde_json::Map<String, serde_json::Value> =
            match serde_json::to_value(&payload_value) {
                Ok(v) => v.as_object().cloned().unwrap_or_default(),
                Err(e) => {
                    return Err(EdgelinkError::InvalidOperation(format!(
                        "pnex-device-write: payload not serializable: {e}"
                    ))
                    .into());
                }
            };
        self.write_values(msg, cancel, obj).await
    }

    /// One batch POST to the internal route with the values filtered on the
    /// configured pins. Lenient result: pins in error are
    /// warned, the message passes (passthrough).
    async fn write_values(
        &self,
        msg: MsgHandle,
        cancel: CancellationToken,
        map: serde_json::Map<String, serde_json::Value>,
    ) -> Result<()> {
        let pick = |names: &[String]| {
            let mut out = serde_json::Map::new();
            for name in names {
                if let Some(v) = map.get(name.as_str()) {
                    // A declared value wins over the one in the map.
                    let v = self.config.values.get(name).unwrap_or(v);
                    out.insert(name.clone(), v.clone());
                }
            }
            out
        };
        let values = pick(&self.config.pins);
        let commands = pick(&self.config.commands);
        if values.is_empty() && commands.is_empty() {
            self.reject(format!(
                "the message names none of the selected outputs ({}): nothing written",
                self.anchors().cloned().collect::<Vec<_>>().join(", ")
            ));
            return self.fan_out_done(msg, cancel).await;
        }
        let mut body = serde_json::json!({
            "org_id": self.org_id,
            "device_id": self.config.device_id,
            "values": values,
        });
        if !commands.is_empty() {
            body["commands"] = serde_json::Value::Object(commands.clone());
        }
        let targets: Vec<String> = values.keys().chain(commands.keys()).cloned().collect();
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
                        self.reject(format!(
                            "write {}:{} failed: {status} {detail}",
                            self.config.device_id,
                            targets.join(",")
                        ));
                    } else {
                        match resp.json::<serde_json::Value>().await {
                            Ok(out) => {
                                let mut errors = Vec::new();
                                for r in out.get("results").and_then(|x| x.as_array()).unwrap_or(&vec![]) {
                                    let ok = r.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
                                    let pin = r
                                        .get("pin")
                                        .or_else(|| r.get("command"))
                                        .and_then(|x| x.as_str())
                                        .unwrap_or("?");
                                    if !ok {
                                        let err = r.get("err").and_then(|x| x.as_str()).unwrap_or("?");
                                        errors.push(format!("{pin}: {err}"));
                                    }
                                }
                                if errors.is_empty() {
                                    self.status.passed(self);
                                } else {
                                    self.reject(errors.join("; "));
                                }
                            }
                            Err(e) => self.reject(format!("unreadable write response: {e}")),
                        }
                    }
                }
                Ok(Err(e)) => self.reject(format!("write request failed: {e}")),
                Err(_) => self.reject("write timed out (10 s)".into()),
            },
            _ = cancel.cancelled() => return Err(EdgelinkError::TaskCancelled.into()),
        }
        self.fan_out_done(msg, cancel).await
    }
}

/// Declared values written by a message that carries no map: the pin
/// named by `topic` when it is an anchor, else every declared pin.
/// `None` = nothing declared for it, the message value is written.
fn declared_values<'a>(
    values: &std::collections::BTreeMap<String, serde_json::Value>,
    mut anchors: impl Iterator<Item = &'a String>,
    topic: Option<&str>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let on_anchor = topic.filter(|t| anchors.any(|a| a == t));
    let fixed: serde_json::Map<String, serde_json::Value> = values
        .iter()
        .filter(|(pin, _)| on_anchor.is_none_or(|t| t == pin.as_str()))
        .map(|(pin, v)| (pin.clone(), v.clone()))
        .collect();
    (!fixed.is_empty()).then_some(fixed)
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
                            node.reject(format!("message rejected: {e}"));
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
    use super::declared_values;

    #[test]
    fn declared_values_follow_the_anchor_or_apply_to_all() {
        let values = serde_json::from_value(serde_json::json!({"led": false, "fan": 40})).unwrap();
        let anchors = ["led".to_string(), "fan".to_string(), "door".to_string()];
        let pick =
            |topic| declared_values(&values, anchors.iter(), topic).map(serde_json::Value::Object);
        // A wire on the "led" anchor writes the led value only.
        assert_eq!(pick(Some("led")), Some(serde_json::json!({"led": false})));
        // An anchor without declared value: the message value is written.
        assert_eq!(pick(Some("door")), None);
        // No anchor named (plain trigger, foreign topic): every declared value.
        assert_eq!(
            pick(None),
            Some(serde_json::json!({"led": false, "fan": 40}))
        );
        assert_eq!(
            pick(Some("tick")),
            Some(serde_json::json!({"led": false, "fan": 40}))
        );
    }
}
