//! Deployment topology: where this instance runs and which server
//! processes form it (API, flow workers, both) — read from the shared
//! state every process already publishes (device bus subscriptions,
//! flow worker heartbeats, the placement controller lease).

use std::collections::BTreeMap;

use loco_rs::app::AppContext;
use pnex_core::ComponentStatus;
use sea_orm::EntityTrait;

use super::{component, text_metric, DEGRADED, NOT_CONFIGURED, OK, PROBE_TIMEOUT};
use crate::models::_entities::flow_leases;
use crate::services::flow::FlowSettings;
use crate::services::flow_cluster::{store, ClusterSettings};
use crate::services::last_cache::ValkeySettings;

/// Node roles, as UI tokens (`system-cluster-<token>`).
const ROLE_API: &str = "api";
const ROLE_FLOWS: &str = "flows";
const ROLE_BOTH: &str = "api_flows";

/// Runtime environment of this process.
fn runtime() -> &'static str {
    if std::env::var_os("KUBERNETES_SERVICE_HOST").is_some() {
        "kubernetes"
    } else if std::path::Path::new("/.dockerenv").exists() {
        "docker"
    } else {
        "host"
    }
}

/// Node name of a bus or lease identity (`<host>:<pid>:<ms>`, `api:<host>:…`).
fn node_name(identity: &str) -> &str {
    let identity = identity.strip_prefix("api:").unwrap_or(identity);
    identity.split(':').next().unwrap_or(identity)
}

/// Roles from what each node publishes: every server process subscribes to
/// the device bus, flow workers also heartbeat. When some server process
/// runs no flows, the roles are split (API pods vs dedicated flow workers);
/// otherwise every node does both.
fn roles(servers: &[String], flow_workers: &[String]) -> BTreeMap<String, &'static str> {
    let split = servers.iter().any(|s| !flow_workers.contains(s));
    let mut out = BTreeMap::new();
    for s in servers {
        out.insert(s.clone(), if split { ROLE_API } else { ROLE_BOTH });
    }
    for w in flow_workers {
        out.insert(w.clone(), if split { ROLE_FLOWS } else { ROLE_BOTH });
    }
    out
}

/// Server processes subscribed to the device command bus (D107).
async fn bus_nodes(ctx: &AppContext) -> Option<Vec<String>> {
    let url = ValkeySettings::from_config(&ctx.config).and_then(|s| s.url)?;
    let probe = async {
        let client = redis::Client::open(url.as_str()).ok()?;
        let mut conn = client.get_multiplexed_async_connection().await.ok()?;
        redis::cmd("PUBSUB")
            .arg("CHANNELS")
            .arg(crate::services::device_bus::pod_channel("*"))
            .query_async::<Vec<String>>(&mut conn)
            .await
            .ok()
    };
    let channels = tokio::time::timeout(PROBE_TIMEOUT, probe).await.ok()??;
    let prefix = crate::services::device_bus::pod_channel("");
    let mut nodes: Vec<String> = channels
        .iter()
        .filter_map(|c| c.strip_prefix(prefix.as_str()))
        .map(|id| node_name(id).to_string())
        .collect();
    nodes.sort();
    nodes.dedup();
    Some(nodes)
}

// shortcut: queue workers (firmware builds, stitch) publish no presence and
// are not listed; add a heartbeat to them when this card needs them.
pub(super) async fn probe(ctx: &AppContext) -> ComponentStatus {
    let mut out = component("topology", OK);
    out.metrics.push(text_metric("cluster_runtime", runtime()));
    if !FlowSettings::from_config(&ctx.config).enabled {
        out.status = NOT_CONFIGURED.into();
        out.detail = Some("flow engine disabled (settings.flow.enabled = false)".into());
        return out;
    }
    let ttl = ClusterSettings::from_config(&ctx.config).worker_ttl_ms;
    let flow_workers: Vec<String> = store::workers(&ctx.db)
        .await
        .unwrap_or_default()
        .iter()
        .filter(|w| store::is_alive(w, ttl))
        .map(|w| node_name(&w.id).to_string())
        .collect();
    let servers = bus_nodes(ctx).await.unwrap_or_default();
    let nodes = roles(&servers, &flow_workers);
    out.metrics.push(text_metric(
        "cluster_mode",
        if nodes.len() > 1 {
            "clustered"
        } else {
            "single"
        },
    ));
    let controller = flow_leases::Entity::find_by_id("flow-placement".to_string())
        .one(&ctx.db)
        .await
        .ok()
        .flatten()
        .filter(|l| l.expires_at > chrono::Utc::now())
        .map(|l| node_name(&l.holder).to_string());
    if let Some(name) = &controller {
        out.metrics
            .push(text_metric("cluster_controller", name.clone()));
    }
    for (name, role) in &nodes {
        let mut m = text_metric("cluster_node", *role);
        m.label = Some(name.clone());
        out.metrics.push(m);
    }
    if flow_workers.is_empty() {
        out.status = DEGRADED.into();
        out.detail = Some("no live flow worker: deployed flows are not running".into());
    } else if controller.is_none() {
        out.status = DEGRADED.into();
        out.detail = Some("no placement controller holds the lease".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn node_name_strips_process_suffix() {
        assert_eq!(node_name("pnex-api-x:1:1792"), "pnex-api-x");
        assert_eq!(node_name("api:pnex-api-x:1792"), "pnex-api-x");
        assert_eq!(node_name("solo"), "solo");
    }

    #[test]
    fn roles_split_or_embedded() {
        // Every server runs flows: embedded topology.
        let r = roles(&names(&["a", "b"]), &names(&["a", "b"]));
        assert!(r.values().all(|role| *role == ROLE_BOTH));
        // API-only pods next to dedicated flow workers.
        let r = roles(&names(&["a", "w"]), &names(&["w"]));
        assert_eq!(r["a"], ROLE_API);
        assert_eq!(r["w"], ROLE_FLOWS);
        // A worker missing from the bus (Valkey hiccup) is still listed.
        let r = roles(&names(&["a"]), &names(&["w"]));
        assert_eq!(r.len(), 2);
    }
}
