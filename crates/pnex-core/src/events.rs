//! Events contract (camera-video.md D84) — JSON events stored as
//! OpenObserve **logs**, never in the relational database. Pure, wasm-safe.
//!
//! - [`event_stream_name`]: user label → O2 logs stream (`ev_<slug>`);
//! - [`EventLevel`]: severity codes;
//! - [`EventLogConfig`]: config of the `event-log` flow node;
//! - [`EventRecord`]: API DTO of one stored event.

use serde::{Deserialize, Serialize};

/// Prefix of every PNEX event stream in O2 (keeps them apart from any other
/// logs stream of the org).
pub const EVENT_STREAM_PREFIX: &str = "ev_";
/// Default stream when the node leaves the name empty.
pub const DEFAULT_EVENT_STREAM: &str = "ev_events";
/// Max length of the user part of a stream name.
pub const EVENT_STREAM_MAX_LEN: usize = 48;

/// User label → O2 stream name: lowercase, `[a-z0-9_]`, other characters
/// folded to `_`, prefixed `ev_`. Empty → [`DEFAULT_EVENT_STREAM`];
/// `None` when the label is longer than [`EVENT_STREAM_MAX_LEN`] or has no
/// usable character.
pub fn event_stream_name(label: &str) -> Option<String> {
    let label = label.trim();
    let label = label.strip_prefix(EVENT_STREAM_PREFIX).unwrap_or(label);
    if label.is_empty() {
        return Some(DEFAULT_EVENT_STREAM.to_string());
    }
    if label.chars().count() > EVENT_STREAM_MAX_LEN {
        return None;
    }
    let mut slug = String::with_capacity(label.len());
    for c in label.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('_') {
            slug.push('_');
        }
    }
    let slug = slug.trim_matches('_');
    if slug.is_empty() {
        return None;
    }
    Some(format!("{EVENT_STREAM_PREFIX}{slug}"))
}

/// Severity of an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventLevel {
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl EventLevel {
    pub const ALL: [EventLevel; 4] = [Self::Debug, Self::Info, Self::Warn, Self::Error];

    pub fn wire(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.wire() == s)
    }
}

/// Configuration of the `event-log` flow node: writes `msg.payload` (any
/// JSON) as one event in the org's O2 logs stream.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EventLogConfig {
    /// Stream label (empty = `ev_events`).
    #[serde(default)]
    pub stream: String,
    #[serde(default)]
    pub level: EventLevel,
    /// Optional short message; `{{topic}}` is not templated — the node
    /// copies `msg.topic` into the `topic` field of the event.
    #[serde(default)]
    pub message: String,
}

impl EventLogConfig {
    /// Structural check shared by the save validation and the runtime build.
    pub fn check(&self) -> Option<(&'static str, String)> {
        if event_stream_name(&self.stream).is_none() {
            return Some((
                "event_stream_invalid",
                format!(
                    "stream name must contain letters or digits and be at most {EVENT_STREAM_MAX_LEN} characters"
                ),
            ));
        }
        if self.message.chars().count() > 500 {
            return Some((
                "event_message_too_long",
                "message is limited to 500 characters".into(),
            ));
        }
        None
    }
}

/// Event written through `/internal/flow/event` (runtime → backend).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventInput {
    pub org_id: i64,
    /// Final stream name (`ev_…`), already normalized by the node.
    pub stream: String,
    pub level: EventLevel,
    #[serde(default)]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_id: Option<i64>,
    #[serde(default)]
    pub node_id: String,
    /// Free JSON (stored serialized: O2 flattens nested objects).
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// One stored event (`GET /api/v1/events`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventRecord {
    /// Unix microseconds (O2 `_timestamp`).
    pub ts_us: i64,
    pub stream: String,
    pub level: String,
    #[serde(default)]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_id: Option<i64>,
    #[serde(default)]
    pub node_id: String,
    /// Parsed back from the stored JSON string.
    #[serde(default)]
    pub payload: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_names() {
        assert_eq!(event_stream_name("").as_deref(), Some("ev_events"));
        assert_eq!(
            event_stream_name("Person detected!").as_deref(),
            Some("ev_person_detected")
        );
        assert_eq!(event_stream_name("ev_doors").as_deref(), Some("ev_doors"));
        assert_eq!(event_stream_name("--").as_deref(), None);
        assert_eq!(event_stream_name(&"a".repeat(49)), None);
    }

    #[test]
    fn config_check() {
        assert!(EventLogConfig::default().check().is_none());
        let bad = EventLogConfig {
            stream: "!!".into(),
            ..Default::default()
        };
        assert_eq!(bad.check().unwrap().0, "event_stream_invalid");
    }

    #[test]
    fn level_wire() {
        for l in EventLevel::ALL {
            assert_eq!(EventLevel::from_wire(l.wire()), Some(l));
        }
    }
}
