//! Node status of the calc and device-write nodes (D103 school, O66/O69):
//! a rejected message is shown on the canvas badge and in the debug panel,
//! not only in the server logs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use edgelink_core::runtime::debug_channel::DebugMessage;
use edgelink_core::runtime::model::FlowsElement;
use edgelink_core::runtime::nodes::FlowNodeBehavior;
use pnex_core::vision::{NodeStatus, NodeStatusLevel, NODE_STATUS_FORMAT};

/// Same heartbeat as the camera/vision nodes: the backend drops a status
/// after 60 s, so a steady state is re-sent at most every 10 s.
const REPUBLISH: Duration = Duration::from_secs(10);

/// Passed/rejected counters and the last published state of one node.
#[derive(Debug)]
pub(crate) struct StatusBoard {
    /// Canvas id of the node (badge matching); empty = never published.
    canvas_id: String,
    ok_code: &'static str,
    rejected_code: &'static str,
    passed: AtomicU64,
    rejected: AtomicU64,
    last: Mutex<Option<(NodeStatusLevel, Instant)>>,
}

impl StatusBoard {
    pub(crate) fn new(
        canvas_id: String,
        ok_code: &'static str,
        rejected_code: &'static str,
    ) -> Self {
        Self {
            canvas_id,
            ok_code,
            rejected_code,
            passed: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            last: Mutex::new(None),
        }
    }

    pub(crate) fn passed<N: FlowNodeBehavior + ?Sized>(&self, node: &N) {
        self.passed.fetch_add(1, Ordering::Relaxed);
        self.publish(node, NodeStatusLevel::Ok, None);
    }

    /// `detail` is the runtime diagnostic, shown verbatim (English).
    pub(crate) fn rejected<N: FlowNodeBehavior + ?Sized>(&self, node: &N, detail: String) {
        self.rejected.fetch_add(1, Ordering::Relaxed);
        self.publish(node, NodeStatusLevel::Warn, Some(detail));
    }

    fn publish<N: FlowNodeBehavior + ?Sized>(
        &self,
        node: &N,
        level: NodeStatusLevel,
        detail: Option<String>,
    ) {
        if self.canvas_id.is_empty() {
            return;
        }
        {
            let mut last = self.last.lock().expect("status lock");
            // A rejection is always shown at once (its detail may differ);
            // a steady OK only refreshes the badge before it expires.
            let due = match *last {
                Some((prev, at)) => {
                    prev != level || level != NodeStatusLevel::Ok || at.elapsed() >= REPUBLISH
                }
                None => true,
            };
            if !due {
                return;
            }
            *last = Some((level, Instant::now()));
        }
        let code = if level == NodeStatusLevel::Ok {
            self.ok_code
        } else {
            self.rejected_code
        };
        let mut status = NodeStatus::new(level, code)
            .stat("passed", self.passed.load(Ordering::Relaxed))
            .stat("rejected", self.rejected.load(Ordering::Relaxed));
        status.detail = detail;
        let Some(engine) = node.engine() else {
            return;
        };
        let name = node.name();
        engine.debug_channel().send(DebugMessage {
            id: self.canvas_id.clone(),
            name: (!name.is_empty()).then(|| name.to_string()),
            msg: serde_json::to_value(&status).unwrap_or(serde_json::Value::Null),
            property: Some("payload".into()),
            format: Some(NODE_STATUS_FORMAT.into()),
            path: node
                .flow()
                .map(|f| f.get_path())
                .unwrap_or_else(|| "global".to_string()),
            topic: None,
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
            msgid: None,
        });
    }
}
