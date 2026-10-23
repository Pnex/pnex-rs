//! Ontology actions (D183): invocation of an action declared on a type.
//!
//! The server checks (live object, type write role, parameters), then
//! publishes an `ActionEvent` on the org channel where the `action-trigger`
//! nodes of the deployed flows listen, and journals the invocation in the
//! object provenance (`object_changes`, action `action.invoke`). It never
//! acts itself (D123): an action without a deployed flow is refused.

use std::time::Duration;

use pnex_core::ontology::action::{action_channel, ActionEvent, ActionInvocation, ActionListener};
use pnex_core::ontology::ActionDef;
use sea_orm::{DatabaseConnection, DbErr};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::changes::{self, Change};
use super::{objects, role_allows, schema, Actor, OntologyError, Result};

/// Hard bound of the Valkey publish.
const VALKEY_TIMEOUT: Duration = Duration::from_secs(2);
/// Provenance action of an invocation.
pub const ACTION_INVOKE: &str = "action.invoke";

/// Deployed flows holding a trigger for `(type_key, action)`, oldest first.
pub async fn listeners(
    db: &DatabaseConnection,
    org_id: i64,
    type_key: &str,
    action: &str,
) -> Result<Vec<ActionListener>> {
    let deployed = crate::controllers::flows::deployed_flows_with_versions(db, org_id)
        .await
        .map_err(|e| OntologyError::Db(DbErr::Custom(e.to_string())))?;
    let wanted = (type_key.to_string(), action.to_string());
    Ok(deployed
        .into_iter()
        .filter(|(_, v)| {
            serde_json::from_value::<pnex_core::FlowGraph>(v.graph.clone())
                .is_ok_and(|g| pnex_core::action_refs_of(&g).contains(&wanted))
        })
        .map(|(f, v)| ActionListener {
            flow_id: f.id,
            flow_name: f.name,
            version: v.version_number,
        })
        .collect())
}

/// A checked invocation: live object, declared action, valid parameters.
pub struct Checked {
    pub object: crate::models::_entities::objects::Model,
    pub action: ActionDef,
    pub write_role: pnex_core::ontology::WriteRole,
    pub params: Map<String, Value>,
}

/// Checks an invocation without sending it (shared by the HTTP route and
/// the assistant's proposals).
pub async fn check(
    db: &DatabaseConnection,
    org_id: i64,
    object_id: Uuid,
    action_key: &str,
    params: &Map<String, Value>,
) -> Result<Checked> {
    let row = objects::find(db, org_id, object_id).await?;
    if row.valid_to.is_some() {
        return Err(OntologyError::NotFound);
    }
    let s = schema(db, org_id).await?;
    let def = s
        .object_type(&row.type_key)
        .ok_or(OntologyError::NotFound)?;
    let action = def
        .action(action_key)
        .ok_or(OntologyError::NotFound)?
        .clone();
    // Parameters are checked like properties (refs must name live objects
    // of the org, R1).
    let as_props = pnex_core::ontology::ObjectTypeDef {
        properties: action.params.clone(),
        ..def.clone()
    };
    let params = objects::check_properties(db, org_id, &as_props, params)
        .await
        .map_err(|e| match e {
            OntologyError::Invalid { field, token } => {
                super::invalid(format!("params.{field}"), token)
            }
            e => e,
        })?;
    Ok(Checked {
        object: row,
        action,
        write_role: def.write_role,
        params,
    })
}

/// Checks then dispatches one invocation. `by` is the display name carried
/// to the flow.
pub async fn invoke(
    db: &DatabaseConnection,
    valkey: Option<redis::aio::ConnectionManager>,
    org_id: i64,
    actor: &Actor,
    by: &str,
    object_id: Uuid,
    action_key: &str,
    params: &Map<String, Value>,
) -> Result<ActionInvocation> {
    let Checked {
        object: row,
        write_role,
        params,
        ..
    } = check(db, org_id, object_id, action_key, params).await?;
    if !role_allows(&actor.role, write_role) {
        return Err(OntologyError::WriteForbidden);
    }
    let flows = listeners(db, org_id, &row.type_key, action_key).await?;
    if flows.is_empty() {
        return Err(OntologyError::ActionNoFlow);
    }
    let event = ActionEvent {
        invocation_id: Uuid::new_v4(),
        object_id,
        object_type: row.type_key.clone(),
        object_title: row.title.clone(),
        action: action_key.to_string(),
        params,
        by: Some(by.to_string()),
        ts_ms: chrono::Utc::now().timestamp_millis(),
    };
    publish(valkey, org_id, &event).await?;
    let out = ActionInvocation {
        invocation_id: event.invocation_id,
        flows,
    };
    changes::submit(Change {
        org_id,
        action: ACTION_INVOKE.into(),
        object_id: object_id.to_string(),
        type_key: row.type_key,
        source_ref: actor.source_ref.clone(),
        before: Value::Null.into(),
        after: serde_json::json!({
            "action": event.action,
            "invocation_id": event.invocation_id,
            "params": event.params,
            "flows": out.flows,
        })
        .into(),
    });
    Ok(out)
}

async fn publish(
    conn: Option<redis::aio::ConnectionManager>,
    org_id: i64,
    event: &ActionEvent,
) -> Result<()> {
    let Some(mut conn) = conn else {
        return Err(OntologyError::ActionUnavailable);
    };
    let frame = serde_json::to_string(event).map_err(|_| OntologyError::ActionUnavailable)?;
    let op = redis::AsyncCommands::publish::<_, _, i64>(&mut conn, action_channel(org_id), frame);
    match tokio::time::timeout(VALKEY_TIMEOUT, op).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "action publish: valkey error");
            Err(OntologyError::ActionUnavailable)
        }
        Err(_) => {
            tracing::warn!("action publish: valkey timeout");
            Err(OntologyError::ActionUnavailable)
        }
    }
}
