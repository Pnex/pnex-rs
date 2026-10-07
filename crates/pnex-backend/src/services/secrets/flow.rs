//! Flow graphs as vault consumers (secrets.md lot S5, http-fetch node).
//!
//! A secret field of a node is a [`SecretSlot`]. On save every typed
//! value becomes a reference to the node field's dedicated secret
//! `flow/<flow id>/<node id>/<field>`, and every picked reference must
//! belong to the org: stored graphs, `GET /flows`, `flows.json` and the
//! cluster wire never carry a value. Usages of a flow = references of its
//! latest version plus its deployed one (a deployed version keeps its
//! secrets undeletable while it runs).

use pnex_core::{FlowGraph, FlowNodeKind, SecretConsumerKind, SecretFieldInput, SecretSlot};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder};
use uuid::Uuid;

use super::binding::{self, Binding};
use super::crypto::Keyring;
use super::store::{self, StoreError, Writer};
use crate::models::_entities::{flow_versions, flows};

/// Name prefix of every secret dedicated to a flow.
pub fn dedicated_prefix(flow_id: i64) -> String {
    format!("flow/{flow_id}/")
}

fn dedicated_name(flow_id: i64, node_id: &str, field: &str) -> String {
    format!("{}{node_id}/{field}", dedicated_prefix(flow_id))
}

/// Who saves a graph: the vault writes it may do.
pub struct GraphWriter<'a> {
    /// `None` = typed values are refused (assistant writes).
    pub ring: Option<&'a Keyring>,
    pub user_id: Option<i64>,
    /// Owner/admin (D117): may type a value.
    pub can_write_secrets: bool,
}

/// Every secret slot of a graph, as `(node_id/field, slot)`.
fn slots_mut(graph: &mut FlowGraph) -> Vec<(String, &mut SecretSlot)> {
    let mut out = Vec::new();
    for node in &mut graph.nodes {
        if let FlowNodeKind::HttpFetch { config } = &mut node.kind {
            for (field, slot) in config.secret_slots_mut() {
                out.push((format!("{}/{field}", node.id), slot));
            }
        }
    }
    out
}

/// Vault references of a graph, as `(node_id/field, secret id)`.
pub fn graph_refs(graph: &FlowGraph) -> Vec<(String, Uuid)> {
    let mut out = Vec::new();
    for node in &graph.nodes {
        if let FlowNodeKind::HttpFetch { config } = &node.kind {
            for (field, slot) in config.secret_slots() {
                if let Some(id) = slot.secret_id() {
                    out.push((format!("{}/{field}", node.id), id));
                }
            }
        }
    }
    out
}

/// R9 bindings of a graph: `(node_id/field, secret id, destination)` of
/// every vault reference.
pub fn graph_bindings(graph: &FlowGraph) -> Vec<Binding> {
    let mut out = Vec::new();
    for node in &graph.nodes {
        if let FlowNodeKind::HttpFetch { config } = &node.kind {
            for (field, slot) in config.secret_slots() {
                if let Some(id) = slot.secret_id() {
                    out.push((
                        format!("{}/{field}", node.id),
                        id,
                        config.secret_destination(field),
                    ));
                }
            }
        }
    }
    out
}

/// Bindings `flow_id` already holds: its latest and deployed versions
/// (nothing for a flow being created).
async fn held_bindings<C: ConnectionTrait>(
    db: &C,
    flow_id: i64,
) -> Result<Vec<Binding>, StoreError> {
    let mut graphs = Vec::new();
    if let Some(v) = flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(flow_id))
        .order_by_desc(flow_versions::Column::VersionNumber)
        .one(db)
        .await?
    {
        graphs.push(v.graph);
    }
    let deployed = flows::Entity::find_by_id(flow_id)
        .one(db)
        .await?
        .and_then(|f| f.deployed_version_id);
    if let Some(id) = deployed {
        if let Some(v) = flow_versions::Entity::find_by_id(id).one(db).await? {
            graphs.push(v.graph);
        }
    }
    Ok(graphs
        .into_iter()
        .filter_map(|g| serde_json::from_value::<FlowGraph>(g).ok())
        .flat_map(|g| graph_bindings(&g))
        .collect())
}

/// Rewrites the secret slots of `graph` into vault references: typed
/// values go to their dedicated secret (owner/admin with a keyring only),
/// picked references must belong to `org_id`. Without
/// `can_write_secrets`, every reference must already be held by this flow
/// on the same field toward the same destination (R9, [`binding`]).
///
/// [`binding`]: super::binding
pub async fn store_graph_secrets<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    flow_id: i64,
    graph: &mut FlowGraph,
    by: &GraphWriter<'_>,
) -> Result<(), StoreError> {
    let wanted = graph_bindings(graph);
    if !by.can_write_secrets && !wanted.is_empty() {
        let held = held_bindings(db, flow_id).await?;
        binding::check(Some(&held), &wanted)?;
    }
    let writer = Writer {
        org_id: Some(org_id),
        user_id: by.user_id,
    };
    for (path, slot) in slots_mut(graph) {
        match slot.clone() {
            SecretSlot::Unset => {}
            SecretSlot::Ref(id) => {
                store::find(db, Some(org_id), id).await?;
            }
            SecretSlot::Value(value) => {
                let Some(ring) = by.ring else {
                    return Err(StoreError::WriteForbidden);
                };
                let (node_id, field) = path.split_once('/').unwrap_or((path.as_str(), ""));
                let id = store::resolve_field(
                    db,
                    ring,
                    writer,
                    by.can_write_secrets,
                    &SecretFieldInput::Value { value },
                    &dedicated_name(flow_id, node_id, field),
                )
                .await?;
                *slot = SecretSlot::Ref(id);
            }
        }
    }
    Ok(())
}

/// Rewrites the usages of a flow: references of its latest version, plus
/// those of its deployed version (prefixed `deployed/` when they differ).
pub async fn sync_usages<C: ConnectionTrait>(db: &C, flow_id: i64) -> Result<(), StoreError> {
    let Some(flow) = flows::Entity::find_by_id(flow_id).one(db).await? else {
        return Ok(());
    };
    let latest = flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(flow_id))
        .order_by_desc(flow_versions::Column::VersionNumber)
        .one(db)
        .await?;
    let refs_of = |v: &flow_versions::Model| {
        serde_json::from_value::<FlowGraph>(v.graph.clone())
            .map(|g| graph_refs(&g))
            .unwrap_or_default()
    };
    let mut refs: Vec<(String, Uuid)> = latest.as_ref().map(refs_of).unwrap_or_default();
    let deployed_id = flow.deployed_version_id;
    if deployed_id.is_some() && deployed_id != latest.as_ref().map(|v| v.id) {
        if let Some(v) = flow_versions::Entity::find_by_id(deployed_id.unwrap_or_default())
            .one(db)
            .await?
        {
            for (field, id) in refs_of(&v) {
                if !refs.iter().any(|(f, i)| *f == field && *i == id) {
                    refs.push((format!("deployed/{field}"), id));
                }
            }
        }
    }
    store::set_usages(db, SecretConsumerKind::Flow, &flow_id.to_string(), &refs).await
}

/// Drops the usages of a deleted flow and its dedicated secrets.
pub async fn release<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    flow_id: i64,
) -> Result<(), StoreError> {
    store::release_consumer(
        db,
        Some(org_id),
        SecretConsumerKind::Flow,
        &flow_id.to_string(),
        &dedicated_prefix(flow_id),
    )
    .await
}

/// Whether a deployed graph references `secret_id` from a node field.
pub fn graph_uses(graph: &FlowGraph, secret_id: Uuid) -> bool {
    graph_refs(graph).iter().any(|(_, id)| *id == secret_id)
}
