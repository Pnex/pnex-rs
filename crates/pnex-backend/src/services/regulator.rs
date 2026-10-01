//! Cartes de régulation mixtes (control-cards) — sync/cast serveur.
//!
//! La carte décrite dans un flow **déployé** matérialise un desired-state :
//! ce service le recalcule depuis la base (re-projection complète, même
//! doctrine que l'artefact `flows.json`), le persiste dans
//! `regulator_configs`, puis le cast au device connecté
//! (`ServerMsg::ControlConfig`, remplacement complet). Device hors ligne →
//! rien : le cast est **différé à l'announce** (re-push desired-state,
//! même leçon que les cadences Subscribe 2026-09-03).
//!
//! Garde-fous (D13/D17) :
//! - le serveur **ne régule jamais** — il caste une config, jamais une
//!   écriture de pin ;
//! - les pins sont **résolus à chaque cast** depuis les instances/overlay
//!   (les labels d'authoring sont stockés en base, jamais les gpio) — un
//!   pin repassé `digital_in` rend la carte insoluble → skip + warn ;
//! - jamais de `SetMode` automatique : le mode du pin doit déjà être
//!   correct (admission/UI Pins), sinon la carte est sautée ;
//! - une seule régulation par (device, sortie) : plus petit `flow_id`
//!   gagne, les suivantes sont sautées + warn (validate_graph ne voit
//!   qu'un graphe, l'unicité cross-flows ne peut se garantir qu'ici) ;
//! - le sync est **indépendant du runtime de flows** : jamais de 503, un
//!   échec est loggé et ne bloque pas la projection.

use std::collections::HashMap;

use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, TransactionTrait};

use crate::controllers::ws_device;
use crate::models::_entities::{
    device_capability_instances, device_registries, flow_versions, flows, regulator_configs,
};
use pnex_core::{caps, ControlSpec, Mode, ModeOpts, RegConfig, ServerMsg};

/// Plafond de régulations castées à un device (miroir du firmware — buffer
/// fil 1024 o, 4 configs max).
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

    // 2. Résolution par carte : slug device → pk, labels → gpio (modes
    // contrôlés), dédoublonnage par sortie, plafond par device.
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
                "carte de régulation sautée : device inconnu dans l'org"
            );
            continue;
        };

        // Unicité (device, sortie) cross-flows — plus petit flow_id gagne
        // (les cartes arrivent triées par flow_id asc).
        let key = config.actuator_key();
        if used_outputs.contains_key(&(device.id, key.clone())) {
            tracing::warn!(
                flow_id, node_id, device = slug,
                "carte sautée : cette sortie est déjà régulée par un autre flow (plus petit flow_id gagne)"
            );
            continue;
        }
        let count = per_device.entry(device.id).or_insert(0);
        if *count >= MAX_CONFIGS_PER_DEVICE {
            tracing::warn!(
                flow_id,
                node_id,
                device = slug,
                "carte sautée : plafond de {MAX_CONFIGS_PER_DEVICE} régulations par device atteint"
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
        // Timestamps posés explicitement : `insert_many` ne passe pas par
        // le hook `before_save` du wrapper.
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
            .map_err(|e| ModelError::Any(format!("sync regulator : écriture : {e}").into()))?;
    }
    txn.commit()
        .await
        .map_err(|e| ModelError::Any(format!("sync regulator : commit : {e}").into()))?;

    // 4. Cast aux devices touchés (avant ∪ après ∪ extras) et connectés ;
    // hors ligne → rien : le cast est différé à l'announce.
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
                    // Course perdue : le device vient de se déconnecter —
                    // le cast sera rejoué à l'announce.
                    tracing::debug!(
                        device_registry_id = device_id,
                        "cast perdu (device offline)"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    device_registry_id = device_id,
                    "cast régulations impossible : {e}"
                );
            }
        }
    }
    Ok(())
}

/// Régulations d'un device, **résolues à la volée** (labels → gpio, modes
/// contrôlés au point unique `caps::validate`) — utilisée par le cast
/// immédiat et par le re-cast à l'announce. Ordre déterministe (flow_id,
/// node_id) : le firmware applique les 4 slots dans cet ordre.
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
        .map_err(|e| ModelError::Any(format!("régulations illisibles : {e}").into()))?;

    let Some(device) = device_registries::Entity::find_by_id(device_registry_id)
        .one(db)
        .await
        .map_err(|e| ModelError::Any(format!("device introuvable : {e}").into()))?
    else {
        return Ok(Vec::new());
    };
    let (pins, soc) = resolve_pins(db, &device).await;

    let mut specs = Vec::with_capacity(rows.len());
    for row in rows {
        let Ok(config) = serde_json::from_value::<RegConfig>(row.config.clone()) else {
            tracing::warn!(
                device_registry_id, node_id = %row.node_id,
                "config de régulation illisible — ignorée (redéployer le flow)"
            );
            continue;
        };
        match build_spec(&pins, soc, row.node_id.clone(), &config) {
            Ok(spec) => specs.push(spec),
            Err(reason) => {
                tracing::warn!(
                    device_registry_id, node_id = %row.node_id, device = %device.device_id,
                    "carte de régulation insoluble : {reason}"
                );
            }
        }
    }
    specs.truncate(MAX_CONFIGS_PER_DEVICE);
    Ok(specs)
}

// ─────────────────────────────── Résolution ───────────────────────────────

/// Ligne matérialisée (avant cast).
struct RegulatorRow {
    org_id: i64,
    device_registry_id: i64,
    flow_id: i64,
    node_id: String,
    kind: String,
    config: serde_json::Value,
}

/// Label → (gpio, mode) pour un device : instances persistées **puis**
/// overlay en complément (parité du pinout de l'éditeur). L'instance gagne
/// (elle porte le mode admis) ; un pin overlay-only vaut `digital_in`/`adc_in`
/// (jamais une sortie : une sortie doit être admise).
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

/// Carte d'authoring → `ControlSpec` fil, pins résolus et validés au point
/// unique (`caps::validate`). Erreur = raison lisible du skip (jamais de
/// `SetMode` automatique, jamais de pin au hasard).
fn build_spec(
    pins: &HashMap<String, (u16, Mode)>,
    soc: pnex_core::Soc,
    node_id: String,
    config: &RegConfig,
) -> Result<ControlSpec, String> {
    let sensor = pins
        .get(config.sensor_pin().trim().to_ascii_lowercase().as_str())
        .ok_or_else(|| format!("pin capteur « {} » introuvable", config.sensor_pin()))?;
    let actuator = pins
        .get(config.actuator_pin().trim().to_ascii_lowercase().as_str())
        .ok_or_else(|| format!("pin actionneur « {} » introuvable", config.actuator_pin()))?;
    if sensor.0 == actuator.0 {
        return Err("le pin capteur et le pin actionneur se résolvent au même gpio".into());
    }
    if !matches!(sensor.1, Mode::DigitalIn | Mode::AdcIn) {
        return Err(format!(
            "le pin capteur « {} » n'est pas une entrée (mode actuel : {:?})",
            config.sensor_pin(),
            sensor.1
        ));
    }
    if actuator.1 != Mode::DigitalOut {
        return Err(format!(
            "le pin actionneur « {} » n'est pas une sortie admise digital_out (mode actuel : {:?})",
            config.actuator_pin(),
            actuator.1
        ));
    }
    // Point unique de validation silicium (admission + push) — le safe_state
    // de la carte est la contrainte de repos de la sortie.
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
    .map_err(|e| format!("sortie « {} » invalide : {e:?}", config.actuator_pin()))?;

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

/// Mode colonne (`analog_in` convention base) → Mode fil — même convention
/// que `ws_device::str_to_mode`.
fn str_to_mode(s: &str) -> Mode {
    match s {
        "digital_out" => Mode::DigitalOut,
        "pwm_out" => Mode::PwmOut,
        "analog_in" => Mode::AdcIn,
        _ => Mode::DigitalIn,
    }
}
