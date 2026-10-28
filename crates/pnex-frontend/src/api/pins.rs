//! Brick 0 pin endpoints — `GET /api/v1/devices/{id}/pins` and
//! `POST /api/v1/devices/{id}/commands` (manual action D17).

use crate::api::client;
use crate::api::error::ApiError;

/// Flow owning the WRITE claim on a pin (deployed flows only) — mirrors the
/// backend `reserved_by` payload. The UI greys manual write controls and
/// pin pickers for reserved pins (one write source per output).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct PinReservation {
    pub flow_id: i64,
    pub flow_name: String,
}

/// A pin of the generic device (mirror of the backend PinDto).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct PinInfo {
    pub gpio: i32,
    pub label: String,
    pub mode: String,
    pub role: String,
    pub pullup: bool,
    pub safe_state: String,
    pub enabled: bool,
    /// Persisted read cadence (ms, 0/absent = manual) — initialises
    /// the cadence select to its effective value (lesson of controlled
    /// selects: the select must SHOW the real state, not a default).
    #[serde(default)]
    pub interval_ms: Option<u32>,
    #[serde(default)]
    pub last_value: Option<serde_json::Value>,
    /// Modes the UI may offer for this gpio (chip caps, base naming —
    /// `analog_in`; empty when the SoC is unknown → the current mode only).
    #[serde(default)]
    pub available_modes: Vec<String>,
    /// Flows holding the WRITE claim on this pin (empty = free). All owners
    /// are listed — grandfathered double-claims stay visible.
    #[serde(default)]
    pub reserved_by: Vec<PinReservation>,
}

/// `GET /devices/{id}/pins` response.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct PinsResponse {
    pub pins: Vec<PinInfo>,
    pub connected: bool,
}

/// `POST /devices/{id}/commands` body (op set_mode | write | subscribe).
#[derive(Debug, Clone)]
pub struct Command {
    pub op: &'static str,
    pub gpio: u16,
    pub mode: Option<&'static str>,
    pub safe_state: Option<&'static str>,
    pub value: Option<serde_json::Value>,
    pub interval_ms: Option<u32>,
    /// set_mode only — `true` after the UI confirmation "stop the
    /// flows then apply" (409 guard `pin-flow-conflict`).
    pub confirm_stop_flows: Option<bool>,
}

impl Command {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "op": self.op,
            "gpio": self.gpio,
            "mode": self.mode,
            "opts": self.safe_state.map(|s| serde_json::json!({"safe_state": s})),
            "value": self.value,
            "interval_ms": self.interval_ms,
            "confirm_stop_flows": self.confirm_stop_flows,
        })
    }
}

/// `GET /devices/{id}/pins`.
pub async fn pins(device_pk: i64) -> Result<PinsResponse, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/devices/{device_pk}/pins"),
        None,
    )
    .await
}

/// A pin of the pinout (`source` = instance | overlay — the board default
/// when the device has never connected).
#[derive(Clone, Debug, PartialEq)]
pub struct PinoutPin {
    /// `None` = power/ground/reset pin (not configurable, drawn by the SVG).
    pub gpio: Option<i32>,
    pub label: String,
    /// `None` = reserved screen pin (no mode, greyed out in the editor).
    pub mode: Option<String>,
    /// `instance` (real mode) or `overlay` (board default, device never
    /// connected) — consumed by the inspector (suffix of the pin label).
    pub source: String,
    /// Active subscription (`interval_ms`) — `None` = never subscribed (or
    /// unsubscribed): the generic firmware publishes nothing for this pin.
    pub subscribed_ms: Option<u32>,
    // ── Board v2 enrichment (all optional — absent outside a v2 profile).
    pub pos: Option<serde_json::Value>,
    pub kind: Option<String>,
    pub fns: Vec<String>,
    pub flags: Vec<String>,
    pub available_modes: Vec<String>,
    pub warnings: Vec<String>,
    pub reserved: bool,
    pub last_value: Option<serde_json::Value>,
    /// Flows holding the WRITE claim on this pin (empty = free). All owners
    /// are listed — grandfathered double-claims stay visible.
    pub reserved_by: Vec<PinReservation>,
}

/// `board` section of the pinout (v2 profile) — `None` outside a profile (the
/// front then falls back to the old card grid).
#[derive(Clone, Debug, PartialEq)]
pub struct BoardSummary {
    pub id: Option<i64>,
    pub name: Option<String>,
    pub soc: Option<String>,
    pub pretty_name: Option<String>,
    pub chip_label: Option<String>,
    pub profile_id: Option<String>,
    pub layout: serde_json::Value,
    pub peripherals: serde_json::Value,
    /// Device screen state ({"screen": bool}).
    pub peripherals_state: serde_json::Value,
}

/// `GET /devices/{id}/pinout` response — pins + WS connection of the device.
#[derive(Debug, Clone, PartialEq)]
pub struct Pinout {
    /// Is the generic device connected to the server? `false` = offline
    /// (not reconnected yet after a restart, for instance).
    pub connected: bool,
    pub board: Option<BoardSummary>,
    pub pins: Vec<PinoutPin>,
    /// Commands announced by a custom firmware (`pnex.onCommand`, D146).
    pub commands: Vec<String>,
}

/// `GET /devices/{id}/pinout` — full pinout (instances + v2 profile).
pub async fn pinout(device_pk: i64) -> Result<Pinout, ApiError> {
    let body = client::request::<serde_json::Value>(
        reqwest::Method::GET,
        &format!("/api/v1/devices/{device_pk}/pinout"),
        None,
    )
    .await?;
    let board = if body["board"].is_null() {
        None
    } else {
        let b = &body["board"];
        Some(BoardSummary {
            id: b["id"].as_i64(),
            name: b["name"].as_str().map(str::to_string),
            soc: b["soc"].as_str().map(str::to_string),
            pretty_name: b["pretty_name"].as_str().map(str::to_string),
            chip_label: b["chip_label"].as_str().map(str::to_string),
            profile_id: b["profile_id"].as_str().map(str::to_string),
            layout: b["layout"].clone(),
            peripherals: b["peripherals"].clone(),
            peripherals_state: b["peripherals_state"].clone(),
        })
    };
    let mut pins = Vec::new();
    if let Some(list) = body["pins"].as_array() {
        for p in list {
            pins.push(PinoutPin {
                gpio: p["gpio"].as_i64().map(|g| g as i32),
                label: p["label"].as_str().unwrap_or_default().to_string(),
                mode: p["mode"].as_str().map(str::to_string),
                source: p["source"].as_str().unwrap_or_default().to_string(),
                subscribed_ms: p["subscribed_ms"].as_u64().map(|ms| ms as u32),
                pos: p.get("pos").cloned().filter(|v| !v.is_null()),
                kind: p["kind"].as_str().map(str::to_string),
                fns: json_str_list(p.get("fn")),
                flags: json_str_list(p.get("flags")),
                available_modes: json_str_list(p.get("available_modes")),
                warnings: p
                    .get("warnings")
                    .and_then(|w| w.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                reserved: p["reserved"].as_bool().unwrap_or(false),
                last_value: p.get("last_value").cloned().filter(|v| !v.is_null()),
                reserved_by: p
                    .get("reserved_by")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default(),
            });
        }
    }
    Ok(Pinout {
        connected: body["connected"].as_bool().unwrap_or(false),
        board,
        pins,
        commands: json_str_list(body.get("commands")),
    })
}

/// Tolerant list of strings (field absent/null/array).
fn json_str_list(v: Option<&serde_json::Value>) -> Vec<String> {
    v.and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `POST /devices/{id}/commands` — 400 if illegal (chip-caps reason
/// relayed as is), 409 if the device is offline. The response body
/// is returned as is (contains `flow_impacts` when a set_mode stopped
/// deployed flows — Phase 6).
pub enum CommandOutcome {
    /// Command sent (contains `flow_impacts` when a set_mode stopped
    /// deployed flows).
    Sent(serde_json::Value),
    /// 409 `pin-flow-conflict`: deployed flows affected, stop to confirm
    /// (flow names).
    Conflict { flows: Vec<String> },
}

pub async fn command(device_pk: i64, cmd: Command) -> Result<CommandOutcome, ApiError> {
    match client::request_opt::<serde_json::Value>(
        reqwest::Method::POST,
        &format!("/api/v1/devices/{device_pk}/commands"),
        Some(cmd.to_json()),
    )
    .await
    {
        Ok(Some(v)) => Ok(CommandOutcome::Sent(v)),
        Ok(None) => Err(ApiError::local(
            pnex_core::err_codes::CLIENT_EMPTY_BODY,
            "unexpected empty response",
            None,
        )),
        Err(e) if e.code.as_deref() == Some(pnex_core::err_codes::PIN_FLOW_CONFLICT) => {
            // shortcut: names come joined by ", " in `errors.args.flows`; a
            // flow name containing ", " splits in two, harmless for display.
            let flows: Vec<String> = e
                .args
                .as_ref()
                .and_then(|a| a.get("flows"))
                .and_then(|f| f.as_str())
                .map(|f| f.split(", ").map(str::to_string).collect())
                .unwrap_or_default();
            if flows.is_empty() {
                return Err(e);
            }
            Ok(CommandOutcome::Conflict { flows })
        }
        Err(e) => Err(e),
    }
}
