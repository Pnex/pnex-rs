//! Pins & commandes devices génériques (Brick 0, brick0.md §6).
//!
//! - `GET /api/v1/devices/{id}/pins` — instances + labels overlay +
//!   `last_value` (mémoire de session, absent si offline) ; viewer inclus ;
//! - `POST /api/v1/devices/{id}/commands` — action **manuelle** (D17) :
//!   `caps::validate` AVANT tout push (400 + raison si illégal), maj de
//!   l'instance, downlink mpsc → le device répond Ack/StateReport ;
//! - `POST /api/v1/devices/{id}/config-sector` — secteur PNEXCFG1 4 Ko
//!   (B0.1) à flasher à 0x200000 à côté du .bin générique ; inclut le
//!   token device (écriture owner/admin, jamais au viewer).
//!
//! Les deux POST refusent proprement (409) si le device n'est pas
//! connecté — jamais d'attente serveur (D17 : pas de boucle, action UI).

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde::Deserialize;
use std::collections::HashMap;

use crate::auth::OrgContext;
use crate::controllers::ws_device;
use crate::models::_entities::{device_capability_instances, device_registries, device_tokens};
use pnex_core::{Mode, ModeOpts, SafeState, ServerMsg};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/devices")
        .add("/{id}/pins", get(pins))
        .add("/{id}/commands", post(commands))
        .add("/{id}/config-sector", post(config_sector))
}

/// Device de l'org courante, sinon 404 (le 404 masque l'existence —
/// même convention que devices.rs).
async fn device_of_org(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: i64,
) -> Result<device_registries::Model> {
    device_registries::Entity::find_by_id(id)
        .filter(device_registries::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .ok_or_else(|| Error::NotFound)
}

/// Instance par gpio pour un device (helper de lecture).
async fn instances_of(
    db: &DatabaseConnection,
    device_registry_id: i64,
) -> Result<Vec<device_capability_instances::Model>> {
    device_capability_instances::Entity::find()
        .filter(device_capability_instances::Column::DeviceRegistryId.eq(device_registry_id))
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

/// Rôle dérivé du mode (sensor/actuator — pas de colonne role, B0.6).
fn role_str(m: Mode) -> &'static str {
    match m {
        Mode::DigitalIn | Mode::AdcIn => "sensor",
        Mode::DigitalOut => "actuator",
    }
}

/// DTO d'un pin pour l'UI.
#[derive(serde::Serialize)]
struct PinDto {
    gpio: i32,
    label: String,
    mode: String,
    role: &'static str,
    pullup: bool,
    safe_state: String,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_value: Option<serde_json::Value>,
}

async fn pins(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let device = device_of_org(&ctx.db, &org, id).await?;
    let rows = instances_of(&ctx.db, device.id).await?;
    let last: HashMap<i32, serde_json::Value> = ws_device::LAST_VALUES
        .lock()
        .expect("last_values")
        .get(&device.id)
        .cloned()
        .unwrap_or_default();
    let dtos: Vec<PinDto> = rows
        .iter()
        .map(|r| PinDto {
            gpio: r.gpio,
            label: r.label.clone(),
            mode: r.mode.clone(),
            role: role_str(str_to_mode_local(&r.mode)),
            pullup: pin_cfg(r).pullup.unwrap_or(false),
            safe_state: match pin_cfg(r).safe_state.unwrap_or(SafeState::Low) {
                SafeState::Low => "low",
                SafeState::High => "high",
            }.into(),
            enabled: r.enabled,
            last_value: last.get(&r.gpio).cloned(),
        })
        .collect();
    format::json(serde_json::json!({ "pins": dtos, "connected": ws_device::is_connected(device.id) }))
}

/// Config jsonb → ModeOpts (défauts si absent/illisible).
fn pin_cfg(r: &device_capability_instances::Model) -> ModeOpts {
    r.config.as_ref()
        .and_then(|c| serde_json::from_value(c.clone()).ok())
        .unwrap_or_default()
}

/// Mode fil → Mode code (miroir de ws_device::str_to_mode).
fn str_to_mode_local(s: &str) -> Mode {
    match s {
        "digital_out" => Mode::DigitalOut,
        "analog_in" => Mode::AdcIn,
        _ => Mode::DigitalIn,
    }
}

// ───────────────────────── POST /commands (D17 : manuel) ─────────────────────

#[derive(Deserialize)]
struct CommandBody {
    /// set_mode | write | subscribe
    op: String,
    gpio: u16,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    opts: Option<ModeOpts>,
    #[serde(default)]
    value: Option<serde_json::Value>,
    #[serde(default)]
    interval_ms: Option<u32>,
}

/// Génération cmd_id (RPC ThingsBoard-like : l'UI peut tracer la commande).
fn new_cmd_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

async fn commands(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    body: String,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let device = device_of_org(&ctx.db, &org, id).await?;
    let cmd: CommandBody = serde_json::from_str(&body)
        .map_err(|e| bad_request(&format!("corps invalide : {e}")))?;
    let rows = instances_of(&ctx.db, device.id).await?;
    let Some(row) = rows.iter().find(|r| r.gpio as u16 == cmd.gpio) else {
        return Err(bad_request(&format!("gpio {} non admis pour ce device", cmd.gpio)));
    };
    let mut cfg: ModeOpts = row
        .config
        .as_ref()
        .and_then(|c| serde_json::from_value(c.clone()).ok())
        .unwrap_or_default();
    let _ = &mut cfg;

    let msg = match cmd.op.as_str() {
        "set_mode" => {
            let mode = str_to_mode_local(cmd.mode.as_deref().unwrap_or(""));
            let opts = cmd.opts.unwrap_or_default();
            // Point unique de validation (brick0.md §2) — AVANT toute maj/push.
            let validated = pnex_core::caps::validate(cmd.gpio, mode, &opts)
                .map_err(|v| bad_request(&v.reason()))?;
            cfg.pullup = opts.pullup;
            cfg.safe_state = opts.safe_state;
            persist_instance(&ctx.db, row, mode, &cfg, None, Some(validated)).await?;
            ServerMsg::SetMode {
                cmd_id: new_cmd_id(),
                gpio: cmd.gpio,
                mode,
                opts: cfg,
            }
        }
        "write" => {
            let pin_mode = str_to_mode_local(&row.mode);
            if pin_mode != Mode::DigitalOut {
                return Err(bad_request("write: le pin n'est pas en digital_out"));
            }
            let Some(v) = cmd.value else {
                return Err(bad_request("write: value requise"));
            };
            let ok = v == serde_json::Value::Bool(true)
                || v == serde_json::Value::Bool(false)
                || v == serde_json::json!(0)
                || v == serde_json::json!(1);
            if !ok {
                return Err(bad_request("write: value doit être true/false (ou 0/1)"));
            }
            ServerMsg::Write { cmd_id: new_cmd_id(), gpio: cmd.gpio, value: v }
        }
        "subscribe" => {
            let Some(interval_ms) = cmd.interval_ms else {
                return Err(bad_request("subscribe: interval_ms requis (0 = désabonner)"));
            };
            if interval_ms > 0 && interval_ms < 100 {
                return Err(bad_request("subscribe: interval_ms min 100 (0 = désabonner)"));
            }
            if interval_ms > 3_600_000 {
                return Err(bad_request("subscribe: interval_ms max 3 600 000"));
            }
            persist_instance(&ctx.db, row, str_to_mode_local(&row.mode), &cfg, Some(interval_ms), None).await?;
            ServerMsg::Subscribe { cmd_id: new_cmd_id(), gpio: cmd.gpio, interval_ms }
        }
        other => {
            return Err(bad_request(&format!("op inconnue : {other} (set_mode | write | subscribe)")));
        }
    };
    // Downlink : 409 si pas de session vivante (offline) — jamais d'attente
    // serveur (D17 : pas de boucle, action utilisateur).
    if !ws_device::push_command(device.id, msg) {
        return Err(Error::CustomError(
            StatusCode::CONFLICT,
            loco_rs::controller::ErrorDetail::new("offline", "device non connecté".to_string()),
        ));
    }
    format::json(serde_json::json!({ "sent": true }))
}

/// Maj de l'instance (mode/config/snapshot) — la base est la source de
/// vérité, appliquée AVANT le push (le device applique ensuite).
#[allow(clippy::too_many_arguments)]
async fn persist_instance(
    db: &DatabaseConnection,
    row: &device_capability_instances::Model,
    mode: Mode,
    cfg: &ModeOpts,
    interval_ms: Option<u32>,
    validated: Option<pnex_core::ValidatedPin>,
) -> Result<()> {
    // config jsonb = ModeOpts + interval_ms optionnel (fusion objet JSON).
    let mut obj = serde_json::to_value(cfg).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(ms) = interval_ms {
        if let Some(map) = obj.as_object_mut() {
            map.insert("interval_ms".into(), serde_json::json!(ms));
        }
    }
    let mut am: device_capability_instances::ActiveModel = row.clone().into();
    am.mode = Set(mode_to_str(mode).to_string());
    am.config = Set(Some(obj));
    if let Some(v) = validated {
        am.constraints_snapshot = Set(Some(serde_json::to_value(v).unwrap_or_default()));
    }
    am.update(db).await?;
    Ok(())
}

/// Mode code → string fil (miroir provisioning::mode_to_str).
fn mode_to_str(m: Mode) -> &'static str {
    match m {
        Mode::DigitalIn => "digital_in",
        Mode::DigitalOut => "digital_out",
        Mode::AdcIn => "analog_in",
    }
}

fn bad_request(msg: &str) -> Error {
    Error::CustomError(
        StatusCode::BAD_REQUEST,
        loco_rs::controller::ErrorDetail::new("bad_request", msg.to_string()),
    )
}

fn forbidden() -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new("forbidden", "écriture réservée owner/admin".to_string()),
    )
}

// ───────────────── POST /config-sector (B0.1 : flash sans compilation) ─────────────────

#[derive(Deserialize)]
struct ConfigSectorBody {
    wifi_ssid: String,
    wifi_password: String,
    host: String,
    #[serde(default = "default_ws_ssl")]
    ws_ssl: bool,
}
fn default_ws_ssl() -> bool {
    true
}
async fn config_sector(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    body: String,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden());
    }
    let device = device_of_org(&ctx.db, &org, id).await?;
    let b: ConfigSectorBody = serde_json::from_str(&body)
        .map_err(|e| bad_request(&format!("corps invalide : {e}")))?;
    let Some(token_row) = device_tokens::Entity::find()
        .filter(device_tokens::Column::DeviceRegistryId.eq(device.id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    let sector = pnex_firmware_builder::DeviceConfig {
        wifi_ssid: b.wifi_ssid,
        wifi_password: b.wifi_password,
        host: b.host,
        token: token_row.token,
        device_id: device.device_id.clone(),
        ws_ssl: b.ws_ssl,
    };
    let bytes = pnex_firmware_builder::build_config_sector(&sector)
        .map_err(|e| bad_request(&e.to_string()))?;
    let mut resp = ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response();
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_static("attachment; filename=\"config.bin\""),
    )
        ;
    Ok(resp)
}
