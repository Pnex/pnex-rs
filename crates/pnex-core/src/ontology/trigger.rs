//! Ontology change events (operational-platform.md D998, pipeline-engine.md
//! D999): the bus frame and the `ontology-trigger` node config. serde-only,
//! wasm32-safe.
//!
//! The server publishes one [`OntologyEvent`] per object change, per end of
//! a link change and per decided proposal, best effort and without replay.
//! The frame names the object, never its properties: a flow reads the
//! current state back with `ontology_query`, so a late event never acts on
//! stale values.

use serde::{Deserialize, Serialize};

use super::schema::valid_key;

/// Valkey pub/sub channel of the org ontology events.
pub fn ontology_channel(org_id: i64) -> String {
    format!("pnex:onto:v1:{org_id}")
}

pub const ON_OBJECT: &str = "object";
pub const ON_LINK: &str = "link";
pub const ON_PROPOSAL: &str = "proposal";
/// Most keys of one list of the config.
pub const KEYS_MAX: usize = 16;

/// One event. `object_id` / `object_type` are the object the event is about
/// (each end of a link, the subject of a proposal).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OntologyEvent {
    /// `object`, `link` or `proposal`.
    pub event: String,
    /// Journal action (`object.create`, `link.close`…) or the decision
    /// (`accepted`, `rejected`) of a proposal.
    pub action: String,
    pub object_id: String,
    pub object_type: String,
    /// Properties whose value changed (object events).
    #[serde(default)]
    pub changed: Vec<String>,
    /// Link type (link events, link proposals).
    #[serde(default)]
    pub link_type: String,
    /// The object at the other end (link events, link proposals).
    #[serde(default)]
    pub other_id: String,
    /// Proposal origin (proposal events).
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub proposal_id: String,
    pub source_ref: String,
    pub ts_ms: i64,
}

/// Configuration of the `ontology-trigger` flow node: one object type and
/// the kinds of events wanted, each narrowed by an optional list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OntologyTriggerConfig {
    #[serde(default)]
    pub object_type: String,
    /// Any of `object`, `link`, `proposal`; empty = `object`.
    #[serde(default)]
    pub on: Vec<String>,
    /// Object events: only when one of these properties changed (empty = any).
    #[serde(default)]
    pub properties: Vec<String>,
    /// Link events and link proposals: only these link types (empty = any).
    #[serde(default)]
    pub link_types: Vec<String>,
    /// Proposal events: only this origin (empty = any).
    #[serde(default)]
    pub origin: String,
    /// Events written by the flow itself are skipped (no loops): its
    /// writes carry `flow:<id>@…` as source. Stamped by the deploy projection.
    #[serde(default)]
    pub skip_own: bool,
}

impl OntologyTriggerConfig {
    fn wants(&self, kind: &str) -> bool {
        if self.on.is_empty() {
            return kind == ON_OBJECT;
        }
        self.on.iter().any(|o| o == kind)
    }

    /// Whether the event is for this node. `own_source` is the source
    /// prefix of the flow's own writes (`flow:<id>@`).
    pub fn matches(&self, ev: &OntologyEvent, own_source: &str) -> bool {
        if ev.object_type != self.object_type || !self.wants(&ev.event) {
            return false;
        }
        if self.skip_own && !own_source.is_empty() && ev.source_ref.starts_with(own_source) {
            return false;
        }
        match ev.event.as_str() {
            ON_OBJECT => {
                self.properties.is_empty() || ev.changed.iter().any(|c| self.properties.contains(c))
            }
            ON_LINK => self.link_types.is_empty() || self.link_types.contains(&ev.link_type),
            ON_PROPOSAL => {
                (self.origin.is_empty() || ev.origin == self.origin)
                    && (self.link_types.is_empty() || self.link_types.contains(&ev.link_type))
            }
            _ => false,
        }
    }

    /// Save validation: an unset node is a valid draft; set keys are well formed.
    pub fn check(&self) -> Option<(&'static str, String)> {
        let bad = |k: &str| !k.is_empty() && !valid_key(k);
        let on_ok = self
            .on
            .iter()
            .all(|o| matches!(o.as_str(), ON_OBJECT | ON_LINK | ON_PROPOSAL));
        let lists_ok = [&self.properties, &self.link_types]
            .iter()
            .all(|l| l.len() <= KEYS_MAX && l.iter().all(|k| valid_key(k)));
        let origin_ok = self.origin.len() <= 128
            && self
                .origin
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/'));
        if bad(&self.object_type) || !on_ok || !lists_ok || !origin_ok || self.on.len() > 3 {
            return Some((
                "ontology_trigger_invalid",
                "invalid type, event kind, property, link type or origin".into(),
            ));
        }
        None
    }

    /// Deploy gate and runtime build: the type is set.
    pub fn check_deployable(&self) -> Option<(&'static str, String)> {
        if self.object_type.is_empty() {
            return Some(("ontology_trigger_unset", "pick an object type".into()));
        }
        self.check()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(event: &str) -> OntologyEvent {
        OntologyEvent {
            event: event.into(),
            action: "object.update".into(),
            object_id: "o1".into(),
            object_type: "person".into(),
            changed: vec!["city".into()],
            link_type: "employs".into(),
            other_id: String::new(),
            origin: "pack.match".into(),
            proposal_id: String::new(),
            source_ref: "manual:4".into(),
            ts_ms: 1,
        }
    }

    #[test]
    fn objects_by_default_narrowed_by_properties() {
        let mut cfg = OntologyTriggerConfig {
            object_type: "person".into(),
            ..Default::default()
        };
        assert!(cfg.matches(&ev(ON_OBJECT), ""));
        assert!(!cfg.matches(&ev(ON_LINK), ""));
        cfg.properties = vec!["name".into()];
        assert!(!cfg.matches(&ev(ON_OBJECT), ""));
        cfg.properties.push("city".into());
        assert!(cfg.matches(&ev(ON_OBJECT), ""));
        cfg.object_type = "organization".into();
        assert!(!cfg.matches(&ev(ON_OBJECT), ""));
    }

    #[test]
    fn links_and_proposals_narrowed_by_type_and_origin() {
        let cfg = OntologyTriggerConfig {
            object_type: "person".into(),
            on: vec![ON_LINK.into(), ON_PROPOSAL.into()],
            link_types: vec!["uses_identifier".into()],
            origin: "pack.match".into(),
            ..Default::default()
        };
        assert!(!cfg.matches(&ev(ON_LINK), ""));
        let mut proposal = ev(ON_PROPOSAL);
        assert!(!cfg.matches(&proposal, ""));
        proposal.link_type = "uses_identifier".into();
        assert!(cfg.matches(&proposal, ""));
        assert!(!cfg.matches(&ev(ON_OBJECT), ""));
    }

    #[test]
    fn own_writes_are_skipped_when_asked() {
        let mut cfg = OntologyTriggerConfig {
            object_type: "person".into(),
            skip_own: true,
            ..Default::default()
        };
        let mut e = ev(ON_OBJECT);
        e.source_ref = "flow:7@3".into();
        assert!(!cfg.matches(&e, "flow:7@"));
        assert!(cfg.matches(&e, "flow:8@"));
        cfg.skip_own = false;
        assert!(cfg.matches(&e, "flow:7@"));
    }

    #[test]
    fn checks() {
        assert!(OntologyTriggerConfig::default().check().is_none());
        assert_eq!(
            OntologyTriggerConfig::default()
                .check_deployable()
                .unwrap()
                .0,
            "ontology_trigger_unset"
        );
        let bad = OntologyTriggerConfig {
            object_type: "person".into(),
            on: vec!["device".into()],
            ..Default::default()
        };
        assert_eq!(bad.check().unwrap().0, "ontology_trigger_invalid");
    }
}
