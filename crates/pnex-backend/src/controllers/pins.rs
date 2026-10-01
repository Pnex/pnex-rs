//! Pins & commandes devices génériques (Brick 0, brick0.md §6).
//!
//! - `GET /api/v1/devices/{id}/pins` — instances + labels overlay +
//!   `last_value` (mémoire de session, absent si offline) ; viewer inclus ;
//! - `POST /api/v1/devices/{id}/commands` — action **manuelle** (D17) :
//!   `caps::validate` AVANT tout push (400 + raison si illégal), maj de
//!   l'instance, downlink mpsc → le device répond Ack/StateReport.
//!
//! Le POST refuse proprement (409) si le device n'est pas connecté —
//! jamais d'attente serveur (D17 : pas de boucle, action utilisateur).
//! (L'endpoint `config-sector` a été retiré : décision du 2026-09-02,
//! le générique est compilé par device — plus de secteur PNEXCFG1.)

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

use crate::auth::OrgContext;
use crate::controllers::ws_device;
use crate::models::_entities::{device_capability_instances, device_registries};
use crate::services::provisioning;
use pnex_core::{Mode, ModeOpts, SafeState, ServerMsg};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/devices")
        .add("/{id}/pins", get(pins))
        .add("/{id}/pinout", get(pinout))
        .add("/{id}/commands", post(commands))
        .add("/{id}/custom-commands", post(custom_command))
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

// ─────────────── pinout (éditeur de flows, éditeur SVG, Phase 6+) ───────────────

/// Souscription d'une instance : `config.interval_ms` (> 0) — `None` si
/// absente ou nulle (0 = désabonné, cf. op `subscribe`).
fn instance_interval_ms(config: &Option<serde_json::Value>) -> Option<u32> {
    config
        .as_ref()
        .and_then(|c| c.get("interval_ms"))
        .and_then(|v| v.as_u64())
        .filter(|ms| *ms > 0)
        .map(|ms| ms as u32)
}

/// Mode par défaut d'un pin de profil non configuré (même dérivation que
/// l'admission : digital_in si légal, sinon repli analogique — A0).
fn default_mode_str(soc: pnex_core::Soc, gpio: u16) -> Option<&'static str> {
    if pnex_core::caps::validate(soc, gpio, Mode::DigitalIn, &ModeOpts::default()).is_ok() {
        Some("digital_in")
    } else if pnex_core::caps::validate(soc, gpio, Mode::AdcIn, &ModeOpts::default()).is_ok() {
        Some("analog_in")
    } else {
        None
    }
}

/// Enrichissement affichage d'un pin (mux, flags, modes offerts, avertisse-
/// ments) — dérivé des chip-caps serveur ; le front ne revalide jamais.
fn display_enrichment(soc: Option<pnex_core::Soc>, gpio: i32) -> serde_json::Value {
    let empty: Vec<String> = Vec::new();
    match soc {
        Some(s) => serde_json::json!({
            "fn": pnex_core::caps::pin_fns(s, gpio as u16),
            "flags": pnex_core::caps::pin_flags(s, gpio as u16),
            "available_modes": pnex_core::caps::available_modes(s, gpio as u16),
            "warnings": pnex_core::caps::pin_warnings(s, gpio as u16),
        }),
        None => serde_json::json!({
            "fn": empty, "flags": empty, "available_modes": empty, "warnings": empty,
        }),
    }
}

/// `GET /api/v1/devices/{id}/pinout` — pinout **lisible** du device pour
/// l'éditeur de flows (Phase 6) : les pins des devices génériques depuis les
/// instances configurées, complétés par l'overlay board pour les gpio non
/// encore configurés (device jamais connecté — `load_overlay` lit la base,
/// aucune connexion requise). Lecture pour tout membre de l'org.
///
/// `connected` = le device générique est-il connecté au serveur (WS) —
/// l'éditeur de flows s'en sert pour distinguer « device hors ligne »
/// (transient, reprend à la reconnexion) d'un pin réellement absent.
async fn pinout(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let device = device_of_org(&ctx.db, &org, id).await?;
    let rows = instances_of(&ctx.db, device.id).await?;
    let soc: Option<pnex_core::Soc> = provisioning::device_soc(&ctx.db, &device).await.ok();
    let details = provisioning::load_board_details(&ctx.db, &device).await?;
    let peripherals = provisioning::device_peripherals(&device);
    let v2 = details.as_ref().and_then(|d| d.v2());

    // Géométrie du profil v2 : gpio → (label, pos, kind, note) — enrichit
    // les instances ET les pins du profil sans instance (y compris l'écran
    // réservé, rendu grisé par l'éditeur).
    let mut profile_pins: HashMap<i32, (String, serde_json::Value, &'static str)> = HashMap::new();
    let mut profile_power_pins: Vec<(String, serde_json::Value, &'static str)> = Vec::new();
    if let Some(p) = v2 {
        for pp in p.pins.iter() {
            let kind = match pp.kind {
                pnex_core::BoardPinKind::Gpio => "gpio",
                pnex_core::BoardPinKind::Power => "power",
                pnex_core::BoardPinKind::Gnd => "gnd",
                pnex_core::BoardPinKind::En => "en",
            };
            match pp.gpio {
                Some(g) => {
                    profile_pins.insert(
                        g as i32,
                        (
                            pp.label.clone(),
                            serde_json::to_value(pp.pos).unwrap_or_default(),
                            kind,
                        ),
                    );
                }
                None => profile_power_pins.push((
                    pp.label.clone(),
                    serde_json::to_value(pp.pos).unwrap_or_default(),
                    kind,
                )),
            }
        }
    }

    // Réservation écran (périphériques activés) — pins grisées, sans mode.
    let reserved_set: HashSet<i32> = details
        .as_ref()
        .map(|d| d.reserved_gpios(&peripherals))
        .unwrap_or_default()
        .into_iter()
        .map(|g| g as i32)
        .collect();

    let last: HashMap<i32, serde_json::Value> =
        ws_device::last_values(&ctx.config, device.id).await;

    // 1) Instances (mode réel configuré) + géométrie/affichage du profil.
    let mut pins: Vec<(String, serde_json::Value)> = rows
        .iter()
        .map(|r| {
            let profile = profile_pins.get(&r.gpio);
            let mut obj = serde_json::json!({
                "gpio": r.gpio,
                "label": r.label,
                "mode": r.mode,
                "source": "instance",
                "subscribed_ms": instance_interval_ms(&r.config),
                "last_value": last.get(&r.gpio),
                "pos": profile.map(|(_, pos, _)| pos.clone()),
                "kind": profile.map(|(_, _, k)| *k),
                "reserved": reserved_set.contains(&r.gpio),
            });
            if let (Some(m), Some(e)) = (
                obj.as_object_mut(),
                display_enrichment(soc, r.gpio).as_object(),
            ) {
                for (k, v) in e {
                    m.insert(k.clone(), v.clone());
                }
            }
            (r.label.clone(), obj)
        })
        .collect();

    // 2) Pins du profil sans instance (jamais configurés + écran réservé) —
    //    défaut de la carte dérivé des chip-caps, même logique que l'admission.
    for (gpio, (label, pos, kind)) in &profile_pins {
        if rows.iter().any(|r| r.gpio == *gpio) {
            continue;
        }
        let reserved = reserved_set.contains(gpio);
        let mode = if reserved {
            None
        } else {
            soc.and_then(|s| default_mode_str(s, *gpio as u16))
        };
        let mut obj = serde_json::json!({
            "gpio": gpio,
            "label": label,
            "mode": mode,
            "source": "overlay",
            "subscribed_ms": null,
            "last_value": null,
            "pos": pos,
            "kind": kind,
            "reserved": reserved,
        });
        if let (Some(m), Some(e)) = (
            obj.as_object_mut(),
            display_enrichment(soc, *gpio).as_object(),
        ) {
            for (k, v) in e {
                m.insert(k.clone(), v.clone());
            }
        }
        // Pin réservé écran : aucun mode proposé + raison nominative en
        // avertissement (le front le rend grisé, non configurable).
        if reserved {
            if let Some(m) = obj.as_object_mut() {
                m.insert("available_modes".into(), serde_json::json!([]));
                if let Some(g) = (*gpio).try_into().ok() {
                    if let Some(reason) = details
                        .as_ref()
                        .and_then(|d| d.reserved_reason(g, &peripherals))
                    {
                        m.insert("warnings".into(), serde_json::json!([reason]));
                    }
                }
            }
        }
        pins.push((label.clone(), obj));
    }

    // 3) Pins d'alim/masse/reset du profil (gpio null) — rendus par le SVG,
    //    non configurables.
    for (label, pos, kind) in &profile_power_pins {
        pins.push((
            label.clone(),
            serde_json::json!({
                "gpio": null,
                "label": label,
                "mode": null,
                "source": "overlay",
                "subscribed_ms": null,
                "last_value": null,
                "pos": pos,
                "kind": kind,
                "fn": [],
                "flags": [],
                "available_modes": [],
                "warnings": [],
                "reserved": false,
            }),
        ));
    }

    // Flow write reservations (one write source per output): a pin written
    // by a deployed flow carries its owner — the UI greys manual writes and
    // the flow editor's pin pickers. One scan for the whole device; power
    // pins (gpio null) are never claimed.
    let owners =
        super::flows::pin_write_owners_by_device(&ctx, org.org.id, &device.device_id).await?;
    for (label, obj) in pins.iter_mut() {
        if let Some(owner_list) = owners.get(&pnex_core::normalize_measurement_name(label)) {
            if let Some(m) = obj.as_object_mut() {
                m.insert(
                    "reserved_by".into(),
                    serde_json::json!(owner_list
                        .iter()
                        .map(|(flow_id, flow_name)| {
                            serde_json::json!({ "flow_id": flow_id, "flow_name": flow_name })
                        })
                        .collect::<Vec<_>>()),
                );
            }
        }
    }

    pins.sort_by_key(|(label, _)| pin_sort_key(label));
    // Section board (profil v2) — `null` si pas de profil (v1, custom
    // Tier 2) : le front bascule alors sur l'ancienne grille de cartes.
    let board_json = match v2 {
        Some(p) => {
            let board = provisioning::device_board(&ctx.db, &device).await.ok();
            Some(serde_json::json!({
                "id": board.as_ref().map(|b| b.id),
                "name": board.as_ref().map(|b| b.name.clone()),
                "soc": board.as_ref().map(|b| b.soc.clone()),
                "pretty_name": board.as_ref().and_then(|b| b.pretty_name.clone()),
                "chip_label": p.chip_label,
                "profile_id": p.board,
                "pio_board": board.as_ref().and_then(|b| b.pio_board.clone()),
                "layout": serde_json::to_value(p.layout).unwrap_or_default(),
                "peripherals": serde_json::to_value(&p.peripherals).unwrap_or_default(),
                // Resolved state for the UI: builtin wins, legacy `true` is
                // shielded — the picker only ever sees a kind or null.
                "peripherals_state": serde_json::json!({
                    "screen": details.as_ref().and_then(|d| d.resolved_screen_kind(&peripherals)),
                }),
            }))
        }
        None => None,
    };
    format::json(serde_json::json!({
        "device_id": device.device_id,
        "connected": ws_device::is_connected(device.id).await,
        "board": board_json,
        "pins": pins.into_iter().map(|(_, obj)| obj).collect::<Vec<_>>(),
    }))
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
        Mode::DigitalOut | Mode::PwmOut => "actuator",
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
    /// Cadence persistée (ms) — initialisation du select UI à la valeur
    /// effective (0 = manuel).
    #[serde(skip_serializing_if = "Option::is_none")]
    interval_ms: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_value: Option<serde_json::Value>,
    /// Modes the UI may offer for this gpio (chip caps, base naming —
    /// `analog_in`, never the wire string `adc_in`); empty when the SoC is
    /// unknown (the UI then falls back to its legacy hardcoded list).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    available_modes: Vec<String>,
    /// Flows owning the WRITE claim on this pin (deployed flows only) —
    /// the Pins panel greys manual write controls for reserved pins. All
    /// owners are listed (grandfathered double-claims stay visible).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    reserved_by: Vec<ReservedByDto>,
}

/// Owner of a pin's write claim (deployed flow).
#[derive(serde::Serialize)]
struct ReservedByDto {
    flow_id: i64,
    flow_name: String,
}

async fn pins(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let device = device_of_org(&ctx.db, &org, id).await?;
    let rows = instances_of(&ctx.db, device.id).await?;
    // Chip caps per SoC — feeds `available_modes` (same derivation as the
    // pinout endpoint).
    let soc = provisioning::device_soc(&ctx.db, &device).await.ok();
    let owners =
        super::flows::pin_write_owners_by_device(&ctx, org.org.id, &device.device_id).await?;
    let last: HashMap<i32, serde_json::Value> =
        ws_device::last_values(&ctx.config, device.id).await;
    // Disabled rows (UART0 console pins retired at admission) are not
    // user pins anymore.
    let mut dtos: Vec<PinDto> = rows
        .iter()
        .filter(|r| r.enabled)
        .map(|r| {
            let available_modes: Vec<String> = soc
                .as_ref()
                .map(|s| {
                    pnex_core::caps::available_modes(*s, r.gpio as u16)
                        .into_iter()
                        .map(mode_to_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            PinDto {
                gpio: r.gpio,
                label: r.label.clone(),
                mode: r.mode.clone(),
                role: role_str(str_to_mode_local(&r.mode)),
                pullup: pin_cfg(r).pullup.unwrap_or(false),
                safe_state: match pin_cfg(r).safe_state.unwrap_or(SafeState::Low) {
                    SafeState::Low => "low",
                    SafeState::High => "high",
                }
                .into(),
                enabled: r.enabled,
                interval_ms: r
                    .config
                    .as_ref()
                    .and_then(|c| c.get("interval_ms"))
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32)
                    .filter(|v| *v > 0),
                last_value: last.get(&r.gpio).cloned(),
                available_modes,
                reserved_by: owners
                    .get(&pnex_core::normalize_measurement_name(&r.label))
                    .map(|owner_list| {
                        owner_list
                            .iter()
                            .map(|(id, name)| ReservedByDto {
                                flow_id: *id,
                                flow_name: name.clone(),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        })
        .collect();
    // Tri naturel des labels (A0 < D0 < D1 < … < D8) : l'ordre SQL est
    // arbitraire et changeait d'un poll à l'autre — les cartes de l'UI
    // se mélangeaient (retour utilisateur 2026-09-03).
    dtos.sort_by_key(|p| pin_sort_key(&p.label));
    format::json(
        serde_json::json!({ "pins": dtos, "connected": ws_device::is_connected(device.id).await }),
    )
}

/// Clé de tri « naturel » d'un label de pin : préfixe alphabétique puis
/// numéro (A0 < D0 < … < D8) — comparable aux tris de fichiers explorateur.
fn pin_sort_key(label: &str) -> (String, u32) {
    let split = label
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or(label.len());
    let (alpha, num) = label.split_at(split);
    (alpha.to_ascii_lowercase(), num.parse().unwrap_or(u32::MAX))
}

/// Config jsonb → ModeOpts (défauts si absent/illisible).
fn pin_cfg(r: &device_capability_instances::Model) -> ModeOpts {
    r.config
        .as_ref()
        .and_then(|c| serde_json::from_value(c.clone()).ok())
        .unwrap_or_default()
}

/// Mode fil → Mode code (miroir de ws_device::str_to_mode).
fn str_to_mode_local(s: &str) -> Mode {
    match s {
        "digital_out" => Mode::DigitalOut,
        "pwm_out" => Mode::PwmOut,
        // `analog_in` = base naming (mode columns); `adc_in` = the wire
        // string — accepted as an alias so a client echoing the wire format
        // back is never surprised.
        "analog_in" | "adc_in" => Mode::AdcIn,
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
    /// set_mode only — `true` = l'utilisateur a confirmé l'arrêt des flows
    /// déployés qui utilisent la pin (garde 409 sans ce flag).
    #[serde(default)]
    confirm_stop_flows: Option<bool>,
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
        return Err(forbidden(
            "pin-commands-forbidden",
            "Owner, admin or member role required to send device commands.",
        ));
    }
    let device = device_of_org(&ctx.db, &org, id).await?;
    // Edge agent (D95): no pins, no firmware commands.
    if super::edge_agents::is_agent(&ctx.db, &device).await {
        return Err(super::edge_agents::unsupported_action());
    }
    // SoC du board — la validation silicium dépend du SoC (flash/USB/strapping
    // divergent entre esp8266 et esp32-c3).
    let soc = crate::services::provisioning::device_soc(&ctx.db, &device).await?;
    let cmd: CommandBody = serde_json::from_str(&body).map_err(|e| {
        bad_request_args(
            "pin-invalid-body",
            "Invalid request body.",
            serde_json::json!({ "detail": e.to_string() }),
        )
    })?;
    // Garde périphérique (écran intégré) : un gpio consommé par un
    // périphérique ACTIVÉ est refusé avec raison explicite, même si une
    // instance périmée traînait — plus parlant que « gpio non admis ».
    if let Some(details) = provisioning::load_board_details(&ctx.db, &device).await? {
        let peripherals = provisioning::device_peripherals(&device);
        if let Some(reason) = details.reserved_reason(cmd.gpio, &peripherals) {
            // pnex-core serves a `board-reserved-screen:<gpio>` token — split
            // it into the machine code + `$value` arg for the ErrorDetail
            // shape (canonical English fallback in `description`).
            let (code, gpio) = reason.split_once(':').unwrap_or((reason.as_str(), ""));
            return Err(bad_request_args(
                code,
                "This GPIO is reserved by an enabled peripheral.",
                serde_json::json!({ "value": gpio }),
            ));
        }
    }
    let rows = instances_of(&ctx.db, device.id).await?;
    let Some(row) = rows.iter().find(|r| r.gpio as u16 == cmd.gpio) else {
        return Err(bad_request_args(
            "pin-gpio-not-admitted",
            "GPIO not admitted for this device.",
            serde_json::json!({ "gpio": cmd.gpio.to_string() }),
        ));
    };
    let mut cfg: ModeOpts = row
        .config
        .as_ref()
        .and_then(|c| serde_json::from_value(c.clone()).ok())
        .unwrap_or_default();
    let _ = &mut cfg;

    // Impacts sur les flows déployés (rempli par set_mode — Phase 6).
    let mut flow_impacts: Vec<(i64, String)> = Vec::new();
    let msg = match cmd.op.as_str() {
        "set_mode" => {
            let mode = str_to_mode_local(cmd.mode.as_deref().unwrap_or(""));
            let opts = cmd.opts.unwrap_or_default();
            // Point unique de validation (brick0.md §2) — AVANT toute maj/push.
            // pnex-core owns the per-violation code (`caps-*`) and the
            // canonical English reason; `$value` carries the gpio.
            let validated = pnex_core::caps::validate(soc, cmd.gpio, mode, &opts).map_err(|v| {
                bad_request_args(
                    v.code(),
                    &v.reason(),
                    serde_json::json!({ "value": cmd.gpio.to_string() }),
                )
            })?;
            // Garde « config device ↔ flows déployés » : le changement de
            // mode demande l'arrêt préalable des flows qui utilisent la pin
            // (409 listant les flows ; `confirm_stop_flows` = confirmation
            // UI « arrêter et appliquer »).
            let impacted = super::flows::flows_impacted_by_pin(
                &ctx,
                org.org.id,
                &device.device_id,
                &row.label,
            )
            .await?;
            if !impacted.is_empty() && cmd.confirm_stop_flows != Some(true) {
                // 409 porteur de la liste des flows (école POI placement) —
                // le front affiche la liste et propose « arrêter et appliquer ».
                return Ok((
                    StatusCode::CONFLICT,
                    axum::Json(serde_json::json!({
                        "error": "pin-flow-conflict",
                        // Legacy marker kept for pre-i18n consumers.
                        "code": "pin_flow_conflict",
                        "description": "This device is used by deployed flow(s) — stop them before changing the config.",
                        "errors": { "args": { "count": impacted.len().to_string() } },
                        "flows": impacted
                            .iter()
                            .map(|(id, name)| serde_json::json!({"flow_id": id, "name": name}))
                            .collect::<Vec<_>>(),
                    })),
                )
                    .into_response());
            }
            cfg.pullup = opts.pullup;
            cfg.safe_state = opts.safe_state;
            persist_instance(&ctx.db, row, mode, &cfg, None, Some(validated)).await?;
            // Dépendances pin ↔ flows : arrêt effectif des flows impactés
            // (base = source de vérité, avant même le push device) —
            // l'utilisateur vient de confirmer.
            flow_impacts = super::flows::stop_flows_reading_pin(
                &ctx,
                org.org.id,
                &device.device_id,
                &row.label,
            )
            .await?;
            ServerMsg::SetMode {
                cmd_id: new_cmd_id(),
                gpio: cmd.gpio,
                mode,
                opts: cfg,
            }
        }
        "write" => {
            let pin_mode = str_to_mode_local(&row.mode);
            let Some(v) = cmd.value else {
                return Err(bad_request(
                    "pin-write-value-required",
                    "Write command requires a value.",
                ));
            };
            match pin_mode {
                Mode::DigitalOut => {
                    let ok = v == serde_json::Value::Bool(true)
                        || v == serde_json::Value::Bool(false)
                        || v == serde_json::json!(0)
                        || v == serde_json::json!(1);
                    if !ok {
                        return Err(bad_request(
                            "pin-write-value-invalid",
                            "Write: value must be true/false (or 0/1).",
                        ));
                    }
                }
                Mode::PwmOut => {
                    // Duty % 0..=100 (abstraction matérielle — le firmware
                    // met à l'échelle en 8-bit).
                    let Some(duty) = v.as_f64() else {
                        return Err(bad_request(
                            "pin-write-duty-type",
                            "Write: value must be a number (duty 0-100).",
                        ));
                    };
                    if !duty.is_finite() || !(0.0..=100.0).contains(&duty) {
                        return Err(bad_request(
                            "pin-write-duty-range",
                            "Write: duty must be between 0 and 100.",
                        ));
                    }
                }
                _ => {
                    return Err(bad_request(
                        "pin-write-mode-not-output",
                        "Write: the pin is not in digital_out/pwm_out mode.",
                    ));
                }
            }
            // Output pins claimed by a deployed flow are flow-owned: a
            // manual write would fight the flow's writes. Reads on the same
            // pin never block (one write source per output). All owning
            // flows are named (grandfathered double-claims).
            let writers =
                super::flows::flows_writing_pin(&ctx, org.org.id, &device.device_id, &row.label)
                    .await?;
            if !writers.is_empty() {
                let names: Vec<String> = writers.iter().map(|(_, name)| name.clone()).collect();
                return Err(custom_error(
                    StatusCode::CONFLICT,
                    pnex_core::err_codes::PIN_RESERVED_BY_FLOW,
                    "This pin is written by a deployed flow — stop that flow to write manually.",
                    Some(serde_json::json!({ "pin": row.label, "flow": names.join(", ") })),
                ));
            }
            ServerMsg::Write {
                cmd_id: new_cmd_id(),
                gpio: cmd.gpio,
                value: v,
            }
        }
        "subscribe" => {
            let Some(interval_ms) = cmd.interval_ms else {
                return Err(bad_request(
                    "pin-subscribe-interval-required",
                    "Subscribe: interval_ms is required (0 = unsubscribe).",
                ));
            };
            if interval_ms > 0 && interval_ms < 100 {
                return Err(bad_request(
                    "pin-subscribe-interval-min",
                    "Subscribe: interval_ms must be at least 100 (0 = unsubscribe).",
                ));
            }
            if interval_ms > 3_600_000 {
                return Err(bad_request(
                    "pin-subscribe-interval-max",
                    "Subscribe: interval_ms must be at most 3,600,000.",
                ));
            }
            persist_instance(
                &ctx.db,
                row,
                str_to_mode_local(&row.mode),
                &cfg,
                Some(interval_ms),
                None,
            )
            .await?;
            ServerMsg::Subscribe {
                cmd_id: new_cmd_id(),
                gpio: cmd.gpio,
                interval_ms,
            }
        }
        other => {
            return Err(bad_request_args(
                "pin-op-unknown",
                "Unknown op (set_mode | write | subscribe).",
                serde_json::json!({ "op": other.to_string() }),
            ));
        }
    };
    // Downlink : 409 si pas de session vivante (offline) — jamais d'attente
    // serveur (D17 : pas de boucle, action utilisateur).
    if !ws_device::push_command(device.id, msg).await {
        return Err(custom_error(
            StatusCode::CONFLICT,
            "pin-device-offline",
            "Device is not connected.",
            None,
        ));
    }
    // Réponse enrichie : les flows arrêtés par le changement de mode
    // (Phase 6) — l'UI les signale explicitement.
    let mut body = serde_json::json!({ "sent": true });
    if !flow_impacts.is_empty() {
        body["flow_impacts"] = serde_json::json!(flow_impacts
            .iter()
            .map(|(id, name)| serde_json::json!({"flow_id": id, "name": name}))
            .collect::<Vec<_>>());
    }
    format::json(body)
}

// ─────────────── POST /custom-commands (D88: custom firmware) ───────────────

/// `POST /api/v1/devices/{id}/custom-commands` — invokes a command
/// registered by a custom firmware (`pnex.onCommand`). The name must be a
/// `command` cap of the last announce (400 otherwise); 409 when offline.
/// Returns the `cmd_id` — the device answers with `Ack{cmd_id}`.
async fn custom_command(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    body: String,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "pin-commands-forbidden",
            "Owner, admin or member role required to send device commands.",
        ));
    }
    let device = device_of_org(&ctx.db, &org, id).await?;
    // Edge agent (D95): no pins, no firmware commands.
    if super::edge_agents::is_agent(&ctx.db, &device).await {
        return Err(super::edge_agents::unsupported_action());
    }
    let req: pnex_core::firmware::CustomCommandRequest =
        serde_json::from_str(&body).map_err(|e| {
            bad_request_args(
                "pin-invalid-body",
                "Invalid request body.",
                serde_json::json!({ "detail": e.to_string() }),
            )
        })?;
    if !announced_command(&device, &req.name) {
        return Err(bad_request_args(
            "device-command-unknown",
            "This device firmware does not announce this command.",
            serde_json::json!({ "value": req.name }),
        ));
    }
    let cmd_id = new_cmd_id();
    let msg = ServerMsg::Command {
        cmd_id: cmd_id.clone(),
        name: req.name,
        args: req.args,
    };
    if !ws_device::push_command(device.id, msg).await {
        return Err(custom_error(
            StatusCode::CONFLICT,
            "pin-device-offline",
            "Device is not connected.",
            None,
        ));
    }
    format::json(serde_json::json!({ "sent": true, "cmd_id": cmd_id }))
}

/// Is `name` a `command` cap of the device's last announce manifest?
fn announced_command(device: &device_registries::Model, name: &str) -> bool {
    device
        .announced_caps
        .as_ref()
        .and_then(|v| v.as_array())
        .is_some_and(|caps| {
            caps.iter().any(|c| {
                c.get("family").and_then(|f| f.as_str()) == Some("command")
                    && c.get("id").and_then(|i| i.as_str()) == Some(name)
            })
        })
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
        Mode::PwmOut => "pwm_out",
        Mode::AdcIn => "analog_in",
    }
}

/// ErrorDetail builder with optional structured args (machine code +
/// canonical English description; the frontend resolves `err-<code>` at
/// render time, verbatim fallback for unknown codes).
fn custom_error(
    status: StatusCode,
    code: &str,
    msg: &str,
    args: Option<serde_json::Value>,
) -> Error {
    Error::CustomError(
        status,
        loco_rs::controller::ErrorDetail {
            error: Some(code.to_string()),
            description: Some(msg.to_string()),
            errors: args.map(|a| serde_json::json!({ "args": a })),
        },
    )
}

fn bad_request(code: &str, msg: &str) -> Error {
    custom_error(StatusCode::BAD_REQUEST, code, msg, None)
}

/// 400 carrying interpolation data in `errors.args`.
fn bad_request_args(code: &str, msg: &str, args: serde_json::Value) -> Error {
    custom_error(StatusCode::BAD_REQUEST, code, msg, Some(args))
}

fn forbidden(code: &str, msg: &str) -> Error {
    custom_error(StatusCode::FORBIDDEN, code, msg, None)
}
