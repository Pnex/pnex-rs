//! Ontology actions (D183): the bus frame and the `action-trigger` node
//! config. serde-only, wasm32-safe.
//!
//! The server checks an invocation (role, parameters), publishes one
//! [`ActionEvent`] on the org channel and journals it in the object
//! provenance; the `action-trigger`
//! nodes of the deployed flows listening to that type and action emit it.
//! The server never acts itself (D123).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::schema::valid_key;

/// Valkey pub/sub channel of the org action invocations.
pub fn action_channel(org_id: i64) -> String {
    format!("pnex:act:v1:{org_id}")
}

/// Pub/sub frame of one invocation; object fields are denormalized so the
/// flow needs no lookup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionEvent {
    pub invocation_id: Uuid,
    pub object_id: Uuid,
    pub object_type: String,
    pub object_title: String,
    pub action: String,
    /// Checked against the action parameters (`check_values`).
    #[serde(default)]
    pub params: Map<String, Value>,
    /// Display name of the invoking user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    pub ts_ms: i64,
}

impl ActionEvent {
    pub fn matches(&self, cfg: &ActionTriggerConfig) -> bool {
        self.object_type == cfg.object_type && self.action == cfg.action
    }
}

/// Configuration of the `action-trigger` flow node: one type, one action.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionTriggerConfig {
    #[serde(default)]
    pub object_type: String,
    #[serde(default)]
    pub action: String,
}

impl ActionTriggerConfig {
    /// Save validation: an unset node is a valid draft; a set key must be
    /// well formed. Existence in the org schema is checked at deploy.
    pub fn check(&self) -> Option<(&'static str, String)> {
        let bad = |k: &str| !k.is_empty() && !valid_key(k);
        if bad(&self.object_type) || bad(&self.action) {
            return Some((
                "action_trigger_invalid",
                "invalid type or action key".into(),
            ));
        }
        None
    }

    /// Deploy gate and runtime build: both keys set.
    pub fn check_deployable(&self) -> Option<(&'static str, String)> {
        if self.object_type.is_empty() || self.action.is_empty() {
            return Some((
                "action_trigger_unset",
                "pick an object type and an action".into(),
            ));
        }
        self.check()
    }
}

/// Invocation result, returned to the caller and journaled in the object
/// provenance (`object_changes`, action `action.invoke`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionInvocation {
    pub invocation_id: Uuid,
    /// Deployed flows holding a matching trigger at invocation time.
    pub flows: Vec<ActionListener>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionListener {
    pub flow_id: i64,
    pub flow_name: String,
    pub version: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_config_draft_and_deploy_gates() {
        let draft = ActionTriggerConfig::default();
        assert!(draft.check().is_none());
        assert_eq!(draft.check_deployable().unwrap().0, "action_trigger_unset");
        let bad = ActionTriggerConfig {
            object_type: "Pump".into(),
            action: "x".into(),
        };
        assert_eq!(bad.check().unwrap().0, "action_trigger_invalid");
        let ok = ActionTriggerConfig {
            object_type: "pump".into(),
            action: "start_maintenance".into(),
        };
        assert!(ok.check_deployable().is_none());
    }

    #[test]
    fn event_matches_type_and_action_only() {
        let cfg = ActionTriggerConfig {
            object_type: "pump".into(),
            action: "start_maintenance".into(),
        };
        let mut ev = ActionEvent {
            invocation_id: Uuid::nil(),
            object_id: Uuid::nil(),
            object_type: "pump".into(),
            object_title: "P12".into(),
            action: "start_maintenance".into(),
            params: Map::new(),
            by: None,
            ts_ms: 0,
        };
        assert!(ev.matches(&cfg));
        ev.object_type = "machine".into();
        assert!(!ev.matches(&cfg));
    }
}
