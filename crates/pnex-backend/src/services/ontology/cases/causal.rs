//! Typed signals of a case (cases-workspace.md D904, L8b): the raw signals
//! of the case (`signals.rs`) turned into `pnex_core::rca::Signal`, plus the
//! device transitions (`device_events`) and the crossings of the declared
//! bounds of member series. The detector `causal_chain@1` (`detect.rs`)
//! reads them; nothing here is stored.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use pnex_core::ontology::api::{SignalItem, SignalsQuery};
use pnex_core::ontology::PropertyKind;
use pnex_core::rca::{Signal, SignalKind};
use pnex_core::resources::KIND_FLOW;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use uuid::Uuid;

use super::super::{links, schema, Result};
use super::membership;
use crate::services::openobserve::Client;

/// Category of a time range read as a maintenance window.
pub const MAINTENANCE_CATEGORY: &str = "maintenance";

fn secs(at: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(at).ok().map(|t| t.timestamp())
}

/// Whether an object change touched a firmware field.
fn firmware_change(before: &serde_json::Value, after: &serde_json::Value) -> bool {
    let props = |v: &serde_json::Value| v.get("properties").cloned().unwrap_or_default();
    let (b, a) = (props(before), props(after));
    a.as_object().into_iter().flatten().any(|(k, v)| {
        let k = k.to_ascii_lowercase();
        (k.contains("firmware") || k.starts_with("fw")) && b.get(&k) != Some(v)
    })
}

/// A raw signal of the case as typed signals (an alert of a flow lands on
/// the flow and on each object linked to it at that instant: what it
/// watches). `neighbours` maps a flow object to those objects.
pub fn typed(item: &SignalItem, neighbours: &HashMap<String, Vec<String>>) -> Vec<Signal> {
    let Some(at) = secs(&item.at) else {
        return Vec::new();
    };
    let one = |kind: SignalKind, id: String, severity: f64, source_ref: String| Signal {
        id,
        at,
        on: item.lane.clone(),
        kind,
        severity,
        source_ref,
    };
    match item.kind.as_str() {
        "link_open" | "link_close" => item
            .link
            .as_ref()
            .map(|l| {
                vec![one(
                    SignalKind::ChangeLink,
                    format!("{}:{}", item.kind, l.id),
                    0.5,
                    format!("link:{}", l.id),
                )]
            })
            .unwrap_or_default(),
        "change" => item
            .change
            .as_ref()
            .map(|c| {
                let kind = if firmware_change(&c.before, &c.after) {
                    SignalKind::ChangeFirmware
                } else {
                    SignalKind::ChangeConfig
                };
                vec![one(
                    kind,
                    format!("change:{}:{}", item.lane, c.at),
                    0.5,
                    c.source_ref.clone(),
                )]
            })
            .unwrap_or_default(),
        "range" => item
            .range
            .as_ref()
            .filter(|r| r.category.as_deref() == Some(MAINTENANCE_CATEGORY))
            .map(|r| {
                vec![one(
                    SignalKind::RangeMaintenance,
                    format!("range:{}", r.id),
                    0.5,
                    format!("range:{}", r.id),
                )]
            })
            .unwrap_or_default(),
        "notification" => {
            let Some(n) = item.notification.as_ref() else {
                return Vec::new();
            };
            let mut on = vec![item.lane.clone()];
            on.extend(neighbours.get(&item.lane).into_iter().flatten().cloned());
            on.into_iter()
                .map(|o| Signal {
                    id: format!("alert:{}:{o}", n.ts_us),
                    at,
                    on: o,
                    kind: SignalKind::Alert,
                    severity: 1.0,
                    source_ref: format!("notification:{}:{}", n.ts_us, item.lane),
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Online, offline and firmware transitions of the member devices.
async fn device_signals(
    db: &DatabaseConnection,
    org_id: i64,
    devices: &HashMap<String, Uuid>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<Signal>> {
    if devices.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = devices.keys().filter_map(|k| k.parse().ok()).collect();
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT id, device_registry_id, at, kind FROM device_events \
             WHERE org_id = $1 AND device_registry_id = ANY($2) AND at BETWEEN $3 AND $4 \
             ORDER BY at, id LIMIT 2001",
            vec![org_id.into(), ids.into(), from.into(), to.into()],
        ))
        .await?;
    let mut out = Vec::new();
    for r in rows {
        let id: i64 = r.try_get("", "id")?;
        let device: i64 = r.try_get("", "device_registry_id")?;
        let at: DateTime<chrono::FixedOffset> = r.try_get("", "at")?;
        let kind: String = r.try_get("", "kind")?;
        let Some(object) = devices.get(&device.to_string()) else {
            continue;
        };
        let kind = match kind.as_str() {
            "online" => SignalKind::DeviceOnline,
            "offline" => SignalKind::DeviceOffline,
            "firmware" => SignalKind::ChangeFirmware,
            _ => continue,
        };
        out.push(Signal {
            id: format!("device:{id}"),
            at: at.timestamp(),
            on: object.to_string(),
            kind,
            severity: if kind == SignalKind::DeviceOffline {
                1.0
            } else {
                0.5
            },
            source_ref: format!("device_event:{id}"),
        });
    }
    Ok(out)
}

/// The first point of each excursion out of `[min, max]`: an edge, not
/// every point outside.
pub fn crossings(points: &[(i64, f64)], min: Option<f64>, max: Option<f64>) -> Vec<(i64, f64)> {
    let out = |v: f64| min.is_some_and(|m| v < m) || max.is_some_and(|m| v > m);
    let mut was_out = false;
    let mut found = Vec::new();
    for &(t, v) in points {
        let now_out = out(v);
        if now_out && !was_out {
            found.push((t, v));
        }
        was_out = now_out;
    }
    found
}

/// Crossings of the declared bounds of the members' series properties.
async fn threshold_signals(
    db: &DatabaseConnection,
    o2: Option<&Client>,
    org_id: i64,
    members: &[crate::models::_entities::objects::Model],
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<Signal>> {
    let s = schema(db, org_id).await?;
    // The smallest window preset reaching back to `from`.
    let back = (Utc::now() - from).num_seconds();
    let Some(&(window, _)) = crate::services::visualization::WINDOWS
        .iter()
        .find(|(_, secs)| *secs >= back)
    else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for o in members {
        let Some(def) = s.object_type(&o.type_key) else {
            continue;
        };
        for p in &def.properties {
            let PropertyKind::Series { min, max, .. } = &p.kind else {
                continue;
            };
            if min.is_none() && max.is_none() {
                continue;
            }
            let Ok(view) = super::super::series::read(db, o2, org_id, &s, o, &p.key, window).await
            else {
                continue;
            };
            let points: Vec<(i64, f64)> = view
                .points
                .iter()
                .filter_map(|pt| Some((secs(&pt.t)?, pt.v)))
                .filter(|(t, _)| *t >= from.timestamp() && *t <= to.timestamp())
                .collect();
            for (t, v) in crossings(&points, *min, *max) {
                out.push(Signal {
                    id: format!("threshold:{}:{}:{t}", o.id, p.key),
                    at: t,
                    on: o.id.to_string(),
                    kind: SignalKind::SeriesThreshold,
                    severity: 0.8,
                    source_ref: format!("series:{}:{}:{v}", o.id, p.key),
                });
            }
        }
    }
    Ok(out)
}

/// Every typed signal of the case over `[from, to]`, by instant.
pub async fn signals(
    db: &DatabaseConnection,
    o2: Option<&Client>,
    org_id: i64,
    case_id: Uuid,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<Signal>> {
    let raw = super::signals::signals(
        db,
        o2,
        org_id,
        case_id,
        &SignalsQuery {
            from: Some(from.to_rfc3339()),
            to: Some(to.to_rfc3339()),
        },
    )
    .await?;
    let m = membership(db, org_id, case_id).await?;
    let ids = m.member_ids();
    let members = links::refs(db, org_id, ids.iter().copied()).await?;
    let member_ids: HashSet<String> = ids.iter().map(Uuid::to_string).collect();
    // The objects each member flow is linked to: what its alerts are about.
    let mut neighbours: HashMap<String, Vec<String>> = HashMap::new();
    for flow in members.values().filter(|o| o.type_key == KIND_FLOW) {
        let around = links::for_object(db, org_id, flow.id, Some(to.into()), false).await?;
        let mut objs: Vec<String> = around
            .iter()
            .map(|l| {
                if l.source.id == flow.id.to_string() {
                    l.target.id.clone()
                } else {
                    l.source.id.clone()
                }
            })
            .filter(|id| member_ids.contains(id))
            .collect();
        objs.sort();
        objs.dedup();
        neighbours.insert(flow.id.to_string(), objs);
    }
    let mut out: Vec<Signal> = raw
        .items
        .iter()
        .flat_map(|i| typed(i, &neighbours))
        .collect();
    let devices: HashMap<String, Uuid> = members
        .values()
        .filter(|o| o.type_key == pnex_core::resources::KIND_DEVICE)
        .map(|o| (o.native_id.clone(), o.id))
        .collect();
    out.extend(device_signals(db, org_id, &devices, from, to).await?);
    let objects: Vec<_> = members.values().cloned().collect();
    out.extend(threshold_signals(db, o2, org_id, &objects, from, to).await?);
    out.sort_by(|a, b| a.at.cmp(&b.at).then(a.id.cmp(&b.id)));
    out.dedup_by(|a, b| a.id == b.id);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossings_are_edges() {
        let pts = vec![
            (0, 10.0),
            (1, 26.0),
            (2, 27.0),
            (3, 20.0),
            (4, 30.0),
            (5, 5.0),
        ];
        assert_eq!(
            crossings(&pts, Some(8.0), Some(25.0)),
            vec![(1, 26.0), (4, 30.0)]
        );
        assert!(crossings(&pts, None, None).is_empty());
    }

    #[test]
    fn firmware_fields_make_a_firmware_change() {
        let b = serde_json::json!({ "properties": { "fw_version": "1.0", "setpoint": 4 } });
        let a = serde_json::json!({ "properties": { "fw_version": "1.1", "setpoint": 4 } });
        assert!(firmware_change(&b, &a));
        let a = serde_json::json!({ "properties": { "fw_version": "1.0", "setpoint": 5 } });
        assert!(!firmware_change(&b, &a));
    }
}
