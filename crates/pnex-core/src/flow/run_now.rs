//! Running a deployed flow now: a person fires one of its Inject triggers
//! by hand, without waiting for its interval or cron.
//!
//! The backend checks the request, then publishes a [`RunNowEvent`] on
//! [`run_now_channel`]. The projection gives each Inject node a companion
//! (`pnex-inject-now`) wired like it, which emits the Inject's message when
//! the event names its flow and node.

use serde::{Deserialize, Serialize};

/// Valkey channel of the org's run-now events.
pub fn run_now_channel(org_id: i64) -> String {
    format!("pnex:run-now:v1:{org_id}")
}

/// Id suffix of the companion of an Inject node in the runtime artifact.
pub const RUN_NOW_SUFFIX: &str = "__now";

/// One request to fire an Inject node now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunNowEvent {
    pub flow_id: i64,
    /// Canvas id of the Inject node.
    pub node_id: String,
    #[serde(default)]
    pub by: Option<String>,
    pub ts_ms: i64,
}

impl RunNowEvent {
    pub fn matches(&self, flow_id: i64, node_id: &str) -> bool {
        self.flow_id == flow_id && self.node_id == node_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_names_one_node_of_one_flow() {
        let ev = RunNowEvent {
            flow_id: 54,
            node_id: "tick".into(),
            by: None,
            ts_ms: 0,
        };
        assert!(ev.matches(54, "tick"));
        assert!(!ev.matches(55, "tick"));
        assert!(!ev.matches(54, "page"));
        assert_eq!(run_now_channel(3), "pnex:run-now:v1:3");
    }
}
