//! Org controls (D125–D127, `docs/architecture/surfaces-controls.md`) —
//! value store and listeners.
//!
//! A surface writes the value of a control; the server validates it
//! (`ControlSpec::accepts`), stores the `ControlEvent` under
//! `pnex:ctl:v1:{org}:{id}` (no TTL: the last command persists) and
//! publishes it on the org channel, where the `control-source` nodes of the deployed flows
//! listen. The server never sends anything to a device from here (D128).
//!
//! Unlike the memory read path, a write is not degraded silently: without
//! Valkey the user must know the command was not sent.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use loco_rs::prelude::*;
use redis::aio::ConnectionManager;
use uuid::Uuid;

use pnex_core::ui_control::{
    control_channel, control_value_key, ControlEvent, ControlListener, ControlValue,
    CONTROL_WRITE_MIN_INTERVAL_MS,
};

/// Hard bound of one Valkey round trip.
const VALKEY_TIMEOUT: Duration = Duration::from_secs(2);
/// O2 event stream of the control writes (audit: who, when, what, where).
pub const CONTROL_EVENT_STREAM: &str = "ev_controls";

/// Why a write did not go through.
#[derive(Debug, PartialEq, Eq)]
pub enum WriteError {
    /// Same control written less than `CONTROL_WRITE_MIN_INTERVAL_MS` ago.
    RateLimited,
    /// Valkey not configured, unreachable or timed out.
    Unavailable,
}

/// Rate-limit marker of one control (`SET NX PX`, cross-pod).
fn rate_key(org_id: i64, control_id: Uuid) -> String {
    format!("{}:rl", control_value_key(org_id, control_id))
}

/// Stores and publishes one value (rate limit first, then one pipeline
/// `SET` + `PUBLISH`).
pub async fn write_value(
    conn: Option<ConnectionManager>,
    org_id: i64,
    event: &ControlEvent,
) -> Result<(), WriteError> {
    let Some(mut conn) = conn else {
        return Err(WriteError::Unavailable);
    };
    // The stored entry is the whole event (key included): the node replays
    // it at start (`emit_on_start`) with the same shape as a live frame.
    let frame = serde_json::to_string(event).map_err(|_| WriteError::Unavailable)?;
    let stored = frame.clone();
    let op = async move {
        let fresh: Option<String> = redis::cmd("SET")
            .arg(rate_key(org_id, event.control_id))
            .arg(1)
            .arg("NX")
            .arg("PX")
            .arg(CONTROL_WRITE_MIN_INTERVAL_MS)
            .query_async(&mut conn)
            .await?;
        if fresh.is_none() {
            return Ok(false);
        }
        redis::pipe()
            .set(control_value_key(org_id, event.control_id), stored)
            .ignore()
            .publish(control_channel(org_id), frame)
            .ignore()
            .query_async::<()>(&mut conn)
            .await?;
        Ok::<_, redis::RedisError>(true)
    };
    match tokio::time::timeout(VALKEY_TIMEOUT, op).await {
        Ok(Ok(true)) => Ok(()),
        Ok(Ok(false)) => Err(WriteError::RateLimited),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "control write: valkey error");
            Err(WriteError::Unavailable)
        }
        Err(_) => {
            tracing::warn!("control write: valkey timeout");
            Err(WriteError::Unavailable)
        }
    }
}

/// Last commanded values (`None` = never written or unreadable). Degraded
/// read: Valkey down = every value `None`, never an error.
pub async fn read_values(
    conn: Option<ConnectionManager>,
    org_id: i64,
    ids: &[Uuid],
) -> BTreeMap<Uuid, Option<ControlValue>> {
    let mut out: BTreeMap<Uuid, Option<ControlValue>> = ids.iter().map(|id| (*id, None)).collect();
    let (Some(mut conn), false) = (conn, ids.is_empty()) else {
        return out;
    };
    let keys: Vec<String> = ids
        .iter()
        .map(|id| control_value_key(org_id, *id))
        .collect();
    let op = async move {
        let raw: Vec<Option<String>> = redis::AsyncCommands::mget(&mut conn, &keys).await?;
        Ok::<_, redis::RedisError>(raw)
    };
    if let Ok(Ok(raw)) = tokio::time::timeout(VALKEY_TIMEOUT, op).await {
        for (id, raw) in ids.iter().zip(raw) {
            let value = raw
                .and_then(|r| serde_json::from_str::<ControlEvent>(&r).ok())
                .map(|e| e.value);
            out.insert(*id, value);
        }
    }
    out
}

/// Deletes the stored value of a removed control (best effort: an orphan
/// value is harmless, no node can list a deleted control at deploy).
pub async fn forget_value(conn: Option<ConnectionManager>, org_id: i64, control_id: Uuid) {
    let Some(mut conn) = conn else {
        return;
    };
    let op = async move {
        redis::AsyncCommands::del::<_, ()>(&mut conn, control_value_key(org_id, control_id)).await
    };
    let _ = tokio::time::timeout(VALKEY_TIMEOUT, op).await;
}

/// Deployed flows listening to each control of the org (`control-source`
/// nodes of their deployed version), oldest flow first.
pub async fn listeners_by_control(
    ctx: &AppContext,
    org_id: i64,
) -> Result<HashMap<Uuid, Vec<ControlListener>>> {
    let deployed = crate::controllers::flows::deployed_flows_with_versions(&ctx.db, org_id).await?;
    let mut out: HashMap<Uuid, Vec<ControlListener>> = HashMap::new();
    for (flow, version) in deployed {
        let Ok(graph) = serde_json::from_value::<pnex_core::FlowGraph>(version.graph) else {
            continue;
        };
        for id in pnex_core::control_refs_of(&graph) {
            out.entry(id).or_default().push(ControlListener {
                flow_id: flow.id,
                flow_name: flow.name.clone(),
            });
        }
    }
    Ok(out)
}

/// Audit trail of one write in the O2 events stream (D86 school), fired
/// in the background: the write never waits for OpenObserve.
pub fn record_write_event(ctx: &AppContext, org_id: i64, event: &ControlEvent) {
    let ctx = ctx.clone();
    let ev = pnex_core::events::EventInput {
        org_id,
        stream: CONTROL_EVENT_STREAM.to_string(),
        level: pnex_core::events::EventLevel::Info,
        message: format!("control {} = {}", event.key, event.value.v),
        topic: Some(event.key.clone()),
        flow_id: None,
        node_id: String::new(),
        payload: serde_json::json!({
            "control_id": event.control_id,
            "key": event.key,
            "value": event.value.v,
            "by": event.value.by,
            "via": event.value.via,
        }),
    };
    tokio::spawn(async move {
        if let Err(e) = crate::services::events::record(&ctx, &ev).await {
            tracing::debug!(error = %e, "control write: audit event not recorded");
        }
    });
}
