//! Read-only diagnostics of the assistant (D142, layer 3): the knowledge
//! cards give the typical cause, these tools give the facts. Org-scoped
//! (the org comes from the principal, never from the model); no secret,
//! token or credential ever appears in an output (R4, R16).

use loco_rs::config::Config;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde_json::{json, Value};

use super::tools::internal;

use crate::models::_entities::{
    device_capability_instances, device_registries, flow_versions, flows, ota_assignments,
};

/// Debug/display messages are cut to this length in a diagnosis.
const MSG_MAX_CHARS: usize = 300;
/// Pin modes that produce telemetry once subscribed.
const INPUT_MODES: &[&str] = &["digital_in", "adc_in"];

fn cut(v: &Value) -> Value {
    let text = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    match text.char_indices().nth(MSG_MAX_CHARS) {
        Some((i, _)) => Value::String(format!("{}…", &text[..i])),
        None => v.clone(),
    }
}

/// Presence, pins (mode, subscription, last value), firmware and OTA of
/// one device, plus hints pointing at knowledge cards.
pub async fn device(
    db: &sea_orm::DatabaseConnection,
    config: Option<&Config>,
    org_id: i64,
    device_id: i64,
) -> Result<Value, super::tools::ToolError> {
    let Some(dev) = device_registries::Entity::find_by_id(device_id)
        .filter(device_registries::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map_err(internal("reading device"))?
    else {
        return Err(format!("device #{device_id} not found in this organization").into());
    };
    let seen = crate::services::device_liveness::seen_of(db, dev.id)
        .await
        .ok();
    let pins = device_capability_instances::Entity::find()
        .filter(device_capability_instances::Column::DeviceRegistryId.eq(dev.id))
        .order_by_asc(device_capability_instances::Column::Gpio)
        .all(db)
        .await
        .map_err(internal("reading pins"))?;
    let last = match config {
        Some(c) => crate::controllers::ws_device::last_values(c, dev.id).await,
        None => Default::default(),
    };
    let subscribed_ms = |p: &device_capability_instances::Model| {
        p.config
            .as_ref()
            .and_then(|c| c.get("interval_ms"))
            .and_then(Value::as_u64)
            .filter(|ms| *ms > 0)
    };
    let pins_out: Vec<Value> = pins
        .iter()
        .map(|p| {
            json!({
                "label": p.label,
                "gpio": p.gpio,
                "mode": p.mode,
                "enabled": p.enabled,
                "subscribed_every_ms": subscribed_ms(p),
                "last_value": last.get(&p.gpio).map(cut),
            })
        })
        .collect();
    let ota = ota_assignments::Entity::find()
        .filter(ota_assignments::Column::DeviceRegistryId.eq(dev.id))
        .order_by_desc(ota_assignments::Column::Id)
        .one(db)
        .await
        .map_err(internal("reading OTA"))?;

    let connected = seen.map(|s| s.connected);
    let mut hints: Vec<&str> = Vec::new();
    if !dev.active {
        hints.push("device inactive: it is not admitted to send data");
    }
    if connected == Some(false) {
        hints.push("device offline: no session currently open (power, WiFi, server address)");
    }
    let inputs: Vec<_> = pins
        .iter()
        .filter(|p| INPUT_MODES.contains(&p.mode.as_str()))
        .collect();
    if !inputs.is_empty() && inputs.iter().all(|p| subscribed_ms(p).is_none()) {
        hints.push("no input pin is subscribed: no telemetry is sent (see card troubleshooting-no-telemetry)");
    }
    if let Some(o) = &ota {
        if o.state == crate::services::ota::ST_FAILED {
            hints.push("last OTA update failed (see ota.error)");
        }
    }
    Ok(json!({
        "device": {
            "id": dev.id,
            "slug": dev.device_id,
            "active": dev.active,
            "firmware_version": dev.fw_version,
            "soc": dev.soc,
        },
        "presence": {
            "connected": connected,
            "last_seen": seen.and_then(|s| s.last_seen).map(|t| t.to_rfc3339()),
        },
        "pins": pins_out,
        "ota": ota.map(|o| json!({
            "state": o.state,
            "progress": o.progress,
            "target_version": o.target_version,
            "error": o.error,
            "updated_at": o.updated_at.to_rfc3339(),
        })),
        "hints": hints,
    }))
}

/// Status of one flow: saved vs deployed version, engine state, last
/// engine error, latest message of each debug/display node.
pub async fn flow(
    db: &sea_orm::DatabaseConnection,
    config: Option<&Config>,
    org_id: i64,
    flow_id: i64,
) -> Result<Value, super::tools::ToolError> {
    let Some(row) = flows::Entity::find_by_id(flow_id)
        .filter(flows::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map_err(internal("reading flow"))?
    else {
        return Err(format!("flow #{flow_id} not found in this organization").into());
    };
    let latest = flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(row.id))
        .order_by_desc(flow_versions::Column::VersionNumber)
        .one(db)
        .await
        .map_err(internal("reading versions"))?
        .map(|v| v.version_number);
    let deployed = match row.deployed_version_id {
        Some(id) => flow_versions::Entity::find_by_id(id)
            .one(db)
            .await
            .map_err(internal("reading deployed version"))?
            .map(|v| v.version_number),
        None => None,
    };
    let (engine, nodes) = match config {
        Some(c) => {
            let status = crate::services::flow_cluster::runtime_status(db, c, org_id, row.id).await;
            let entries =
                crate::services::flow_cluster::debug_entries(db, c, org_id, row.id, None).await;
            let nodes: Vec<Value> = entries
                .iter()
                .map(|e| {
                    json!({
                        "node_id": e.node_id,
                        "name": e.name,
                        "source": e.source,
                        "at": e.ts,
                        "last_message": cut(&e.msg),
                    })
                })
                .collect();
            (
                json!({
                    "engine_status": status.engine_status,
                    "last_error": status.last_error,
                }),
                nodes,
            )
        }
        None => (Value::Null, Vec::new()),
    };
    let mut hints: Vec<&str> = Vec::new();
    if row.status == pnex_core::FLOW_STATUS_DEPLOYED && deployed.is_some() && deployed != latest {
        hints.push("the running version is older than the last saved one: redeploy from the flow editor to run the latest changes (see card troubleshooting-flow-node-not-running)");
    }
    if row.status == pnex_core::FLOW_STATUS_DRAFT {
        hints.push(
            "never deployed: a draft does not run until the user deploys it in the flow editor",
        );
    }
    if engine["last_error"].is_string() {
        hints.push("the engine reported an error for this flow (see engine.last_error)");
    }
    Ok(json!({
        "flow": { "id": row.id, "name": row.name, "status": row.status },
        "versions": { "latest_saved": latest, "deployed": deployed },
        "engine": engine,
        "nodes": nodes,
        "hints": hints,
    }))
}
