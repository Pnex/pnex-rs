//! Mixed regulation cards (control-cards) — server sync/cast.
//!
//! The card described in a **deployed** flow materialises a desired-state:
//! this service recomputes it from the database (full re-projection, same
//! doctrine as the `flows.json` artifact), persists it in
//! `regulator_configs`, then casts it to the connected device
//! (`ServerMsg::ControlConfig`, full replacement). Device offline →
//! nothing: the cast is **deferred to the announce** (desired-state re-push,
//! same lesson as the Subscribe cadences 2026-09-03).
//!
//! Safeguards (D13/D17):
//! - the server **never regulates** — it casts a config, never a
//!   pin write;
//! - pins are **resolved at each cast** from the instances/overlay
//!   (authoring labels are stored in the database, never the gpios) — a
//!   pin switched back to `digital_in` makes the card unresolvable → skip + warn;
//! - never an automatic `SetMode`: the pin's mode must already be
//!   correct (admission/Pins UI), otherwise the card is skipped;
//! - a single regulation per (device, output): the smallest `flow_id`
//!   wins, the following ones are skipped + warn (validate_graph only sees
//!   one graph, cross-flow uniqueness can only be guaranteed here);
//! - the sync is **independent of the flow runtime**: never a 503, a
//!   failure is logged and does not block the projection.

use std::collections::HashMap;

use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, TransactionTrait};

use crate::controllers::ws_device;
use crate::models::_entities::{
    device_capability_instances, device_registries, flow_versions, flows, regulator_configs,
};
use pnex_core::{caps, ControlSpec, Mode, ModeOpts, RegConfig, ServerMsg};

/// Cap on regulations cast to a device (mirrors the firmware — wire
/// buffer 1024 B, 4 configs max).
pub(crate) const MAX_CONFIGS_PER_DEVICE: usize = 4;

// ─────────────────────────────── Sync & cast ───────────────────────────────

/// Recomputes the regulation set of the deployed flows of ONE org,
/// rewrites that org's `regulator_configs` rows, then casts to the
/// affected connected devices. Database errors only (never runtime).
///
/// Cards can only target devices of their own org, so the projection is
/// partitioned by org: a deploy never rewrites other tenants' rows.
/// Read + compute + rewrite run in one transaction under a per-org
/// advisory lock (Postgres), so two concurrent deploys of the org (two
/// pods) serialize and the second one recomputes from the first one's
/// committed state — no lost update.
///
/// `extra_devices`: ADDITIONAL devices to cast — deleting a flow cascades
/// its projection rows BEFORE the sync; without this hint the device of the
/// deleted flow would never receive its clear (`[]`).
pub(crate) async fn sync_and_cast_with(
    db: &DatabaseConnection,
    org_id: i64,
    extra_devices: &[i64],
) -> Result<()> {
    let txn = db
        .begin()
        .await
        .map_err(|e| ModelError::Any(format!("regulator sync: transaction: {e}").into()))?;
    crate::services::db_lock::xact_lock(&txn, crate::services::db_lock::ns::REGULATOR, org_id)
        .await
        .map_err(|e| ModelError::Any(format!("regulator sync: lock: {e}").into()))?;

    // 1. Cards carried by the deployed flows of the org, in flow_id order
    // (the smallest flow_id wins output conflicts). Two queries: flows,
    // then their deployed versions in one `IN`.
    let mut cards: Vec<(i64, i64, String, RegConfig)> = Vec::new(); // (flow_id, org_id, node_id, config)
    let deployed = flows::Entity::find()
        .filter(flows::Column::OrgId.eq(org_id))
        .filter(flows::Column::Status.eq(pnex_core::FLOW_STATUS_DEPLOYED))
        .order_by_asc(flows::Column::Id)
        .all(&txn)
        .await
        .map_err(|e| ModelError::Any(format!("regulator sync: flows unreadable: {e}").into()))?;
    let version_ids: Vec<i64> = deployed
        .iter()
        .filter_map(|f| f.deployed_version_id)
        .collect();
    let mut versions: HashMap<i64, flow_versions::Model> = if version_ids.is_empty() {
        HashMap::new()
    } else {
        flow_versions::Entity::find()
            .filter(flow_versions::Column::Id.is_in(version_ids))
            .all(&txn)
            .await
            .map_err(|e| {
                ModelError::Any(format!("regulator sync: versions unreadable: {e}").into())
            })?
            .into_iter()
            .map(|v| (v.id, v))
            .collect()
    };
    for f in &deployed {
        let Some(version) = f.deployed_version_id.and_then(|id| versions.remove(&id)) else {
            continue;
        };
        let Ok(graph) = serde_json::from_value::<pnex_core::FlowGraph>(version.graph) else {
            continue;
        };
        for (node_id, config) in pnex_core::regulator_configs_of(&graph) {
            cards.push((f.id, f.org_id, node_id, config));
        }
    }

    // Devices referenced by the cards: one query for the org.
    let mut slugs: Vec<String> = cards
        .iter()
        .map(|(_, _, _, c)| c.device_id().to_string())
        .collect();
    slugs.sort_unstable();
    slugs.dedup();
    let mut device_cache: HashMap<(i64, String), Option<device_registries::Model>> = HashMap::new();
    if !slugs.is_empty() {
        for d in device_registries::Entity::find()
            .filter(device_registries::Column::OrgId.eq(org_id))
            .filter(device_registries::Column::DeviceId.is_in(slugs.clone()))
            .all(&txn)
            .await
            .map_err(|e| {
                ModelError::Any(format!("regulator sync: devices unreadable: {e}").into())
            })?
        {
            device_cache.insert((d.org_id, d.device_id.clone()), Some(d));
        }
    }

    // 2. Per-card resolution: device slug → pk, labels → gpio (modes
    // checked), dedup per output, cap per device.
    let mut resolved: Vec<RegulatorRow> = Vec::new();
    let mut used_outputs: HashMap<(i64, String), i64> = HashMap::new(); // (device_pk, actuator_key) → flow_id
    let mut per_device: HashMap<i64, usize> = HashMap::new();
    for (flow_id, org_id, node_id, config) in &cards {
        let slug = config.device_id();
        let device = device_cache
            .get(&(*org_id, slug.to_string()))
            .cloned()
            .flatten();
        let Some(device) = device else {
            tracing::warn!(
                flow_id,
                node_id,
                device = slug,
                "regulation card skipped: device unknown in the org"
            );
            continue;
        };

        // Cross-flow (device, output) uniqueness — smallest flow_id wins
        // (cards arrive sorted by flow_id asc).
        let key = config.actuator_key();
        if used_outputs.contains_key(&(device.id, key.clone())) {
            tracing::warn!(
                flow_id, node_id, device = slug,
                "card skipped: this output is already regulated by another flow (smallest flow_id wins)"
            );
            continue;
        }
        let count = per_device.entry(device.id).or_insert(0);
        if *count >= MAX_CONFIGS_PER_DEVICE {
            tracing::warn!(
                flow_id,
                node_id,
                device = slug,
                "card skipped: cap of {MAX_CONFIGS_PER_DEVICE} regulations per device reached"
            );
            continue;
        }

        resolved.push(RegulatorRow {
            org_id: *org_id,
            device_registry_id: device.id,
            flow_id: *flow_id,
            node_id: node_id.clone(),
            kind: config.kind_str().to_string(),
            config: serde_json::to_value(config).unwrap_or(serde_json::Value::Null),
        });
        used_outputs.insert((device.id, key), *flow_id);
        *count += 1;
    }

    // 3. Rewrite of the org's rows (projection materialization), in the
    // same transaction as the reads above.
    let previous_devices: Vec<i64> = regulator_configs::Entity::find()
        .filter(regulator_configs::Column::OrgId.eq(org_id))
        .all(&txn)
        .await
        .map_err(|e| ModelError::Any(format!("regulator sync: read failed: {e}").into()))?
        .iter()
        .map(|r| r.device_registry_id)
        .collect();
    regulator_configs::Entity::delete_many()
        .filter(regulator_configs::Column::OrgId.eq(org_id))
        .exec(&txn)
        .await
        .map_err(|e| ModelError::Any(format!("sync regulator : purge : {e}").into()))?;
    if !resolved.is_empty() {
        // Timestamps set explicitly: `insert_many` does not go through
        // the wrapper's `before_save` hook.
        let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
        let rows: Vec<regulator_configs::ActiveModel> = resolved
            .iter()
            .map(|r| regulator_configs::ActiveModel {
                node_id: Set(r.node_id.clone()),
                kind: Set(r.kind.clone()),
                config: Set(r.config.clone()),
                org_id: Set(r.org_id),
                device_registry_id: Set(r.device_registry_id),
                flow_id: Set(r.flow_id),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            })
            .collect();
        regulator_configs::Entity::insert_many(rows)
            .exec(&txn)
            .await
            .map_err(|e| ModelError::Any(format!("sync regulator: write: {e}").into()))?;
    }
    txn.commit()
        .await
        .map_err(|e| ModelError::Any(format!("sync regulator : commit : {e}").into()))?;

    // 4. Cast to the affected devices (before ∪ after ∪ extras) that are connected;
    // offline → nothing: the cast is deferred to the announce.
    let mut touched = previous_devices;
    touched.extend(extra_devices.iter().copied());
    touched.extend(resolved.iter().map(|r| r.device_registry_id));
    touched.sort_unstable();
    touched.dedup();
    for device_id in touched {
        if !ws_device::is_connected(device_id).await {
            continue;
        }
        match configs_for_device(db, device_id).await {
            Ok(specs) => {
                let pushed = ws_device::push_command(
                    device_id,
                    ServerMsg::ControlConfig {
                        cmd_id: uuid::Uuid::new_v4().simple().to_string(),
                        configs: specs,
                    },
                )
                .await;
                if !pushed {
                    // Lost race: the device just disconnected —
                    // the cast will be replayed at the announce.
                    tracing::debug!(device_registry_id = device_id, "cast lost (device offline)");
                }
            }
            Err(e) => {
                tracing::warn!(
                    device_registry_id = device_id,
                    "regulation cast failed: {e}"
                );
            }
        }
    }
    Ok(())
}

/// A device's regulations, **resolved on the fly** (labels → gpio, modes
/// checked at the single point `caps::validate`) — used by the immediate
/// cast and by the re-cast at announce. Deterministic order (flow_id,
/// node_id): the firmware applies the 4 slots in that order.
pub(crate) async fn configs_for_device(
    db: &DatabaseConnection,
    device_registry_id: i64,
) -> Result<Vec<ControlSpec>> {
    let rows = regulator_configs::Entity::find()
        .filter(regulator_configs::Column::DeviceRegistryId.eq(device_registry_id))
        .order_by_asc(regulator_configs::Column::FlowId)
        .order_by_asc(regulator_configs::Column::NodeId)
        .all(db)
        .await
        .map_err(|e| ModelError::Any(format!("unreadable regulations: {e}").into()))?;

    let Some(device) = device_registries::Entity::find_by_id(device_registry_id)
        .one(db)
        .await
        .map_err(|e| ModelError::Any(format!("device not found: {e}").into()))?
    else {
        return Ok(Vec::new());
    };
    let (pins, soc) = resolve_pins(db, &device).await;

    let mut specs = Vec::with_capacity(rows.len());
    for row in rows {
        let Ok(config) = serde_json::from_value::<RegConfig>(row.config.clone()) else {
            tracing::warn!(
                device_registry_id, node_id = %row.node_id,
                "unreadable regulation config — ignored (redeploy the flow)"
            );
            continue;
        };
        match build_spec(&pins, soc, row.node_id.clone(), &config) {
            Ok(spec) => specs.push(spec),
            Err(reason) => {
                tracing::warn!(
                    device_registry_id, node_id = %row.node_id, device = %device.device_id,
                    "unresolvable regulation card: {reason}"
                );
            }
        }
    }
    specs.truncate(MAX_CONFIGS_PER_DEVICE);
    Ok(specs)
}

// ─────────────────────────────── Resolution ───────────────────────────────

/// Materialised row (before cast).
struct RegulatorRow {
    org_id: i64,
    device_registry_id: i64,
    flow_id: i64,
    node_id: String,
    kind: String,
    config: serde_json::Value,
}

/// Label → (gpio, mode) for a device: persisted instances **then**
/// overlay as a complement (parity with the editor's pinout). The instance wins
/// (it carries the admitted mode); an overlay-only pin counts as `digital_in`/`adc_in`
/// (never an output: an output must be admitted).
async fn resolve_pins(
    db: &DatabaseConnection,
    device: &device_registries::Model,
) -> (HashMap<String, (u16, Mode)>, pnex_core::Soc) {
    let mut pins: HashMap<String, (u16, Mode)> = HashMap::new();
    let rows = device_capability_instances::Entity::find()
        .filter(device_capability_instances::Column::DeviceRegistryId.eq(device.id))
        .all(db)
        .await
        .unwrap_or_default();
    for r in &rows {
        if !r.enabled {
            continue;
        }
        let mode = str_to_mode(&r.mode);
        pins.insert(r.label.trim().to_ascii_lowercase(), (r.gpio as u16, mode));
    }
    let mut soc = pnex_core::Soc::Esp8266;
    if let Ok((overlay, board_soc)) = crate::services::provisioning::load_overlay(db, device).await
    {
        soc = board_soc;
        for p in overlay.pins {
            let label = p.label.trim().to_ascii_lowercase();
            if pins.contains_key(&label) {
                continue;
            }
            let mode = match p.kind {
                pnex_core::PinKind::Analog => Mode::AdcIn,
                pnex_core::PinKind::Digital => Mode::DigitalIn,
            };
            pins.insert(label, (p.gpio, mode));
        }
    }
    (pins, soc)
}

/// Authoring card → wire `ControlSpec`, pins resolved and validated at the single
/// point (`caps::validate`). Error = readable skip reason (never an
/// automatic `SetMode`, never a random pin).
fn build_spec(
    pins: &HashMap<String, (u16, Mode)>,
    soc: pnex_core::Soc,
    node_id: String,
    config: &RegConfig,
) -> Result<ControlSpec, String> {
    let sensor = pins
        .get(config.sensor_pin().trim().to_ascii_lowercase().as_str())
        .ok_or_else(|| format!("sensor pin \"{}\" not found", config.sensor_pin()))?;
    let actuator = pins
        .get(config.actuator_pin().trim().to_ascii_lowercase().as_str())
        .ok_or_else(|| format!("actuator pin \"{}\" not found", config.actuator_pin()))?;
    if sensor.0 == actuator.0 {
        return Err("the sensor pin and the actuator pin resolve to the same gpio".into());
    }
    if !matches!(sensor.1, Mode::DigitalIn | Mode::AdcIn) {
        return Err(format!(
            "the sensor pin \"{}\" is not an input (current mode: {:?})",
            config.sensor_pin(),
            sensor.1
        ));
    }
    if actuator.1 != Mode::DigitalOut {
        return Err(format!(
            "actuator pin \"{}\" is not an allowed digital_out output (current mode: {:?})",
            config.actuator_pin(),
            actuator.1
        ));
    }
    // Single silicon validation point (admission + push) — the card's
    // safe_state is the output's rest constraint.
    let safe = config.safe_state();
    caps::validate(
        soc,
        actuator.0,
        Mode::DigitalOut,
        &ModeOpts {
            pullup: None,
            safe_state: Some(safe),
        },
    )
    .map_err(|e| format!("invalid output \"{}\": {e:?}", config.actuator_pin()))?;

    let (deadband, kp, ki, kd, cycle_time_secs, min_on_secs, min_off_secs) = match config {
        RegConfig::TtHeat(c) => (c.deadband, 0.0, 0.0, 0.0, 0, c.min_on_secs, c.min_off_secs),
        RegConfig::TtCool(c) => (c.deadband, 0.0, 0.0, 0.0, 0, c.min_on_secs, c.min_off_secs),
        RegConfig::Pid(c) => (0.0, c.kp, c.ki, c.kd, c.cycle_time_secs, 0, 0),
    };
    Ok(ControlSpec {
        node_id,
        sensor_gpio: sensor.0,
        sensor_mode: sensor.1,
        out_gpio: actuator.0,
        kind: config.kind_str().to_string(),
        setpoint: config.setpoint(),
        deadband,
        kp,
        ki,
        kd,
        cycle_time_secs,
        min_on_secs,
        min_off_secs,
        sample_ms: config.sample_ms(),
        data_timeout_secs: config.data_timeout_secs(),
        safe_state: safe,
    })
}

/// Column mode (`analog_in` database convention) → wire Mode — same convention
/// as `ws_device::str_to_mode`.
fn str_to_mode(s: &str) -> Mode {
    match s {
        "digital_out" => Mode::DigitalOut,
        "pwm_out" => Mode::PwmOut,
        "analog_in" => Mode::AdcIn,
        _ => Mode::DigitalIn,
    }
}
