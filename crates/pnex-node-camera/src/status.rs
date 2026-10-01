//! Node status publication (camera-video.md D103): camera and vision nodes
//! report what they are doing — and why frames are dropped — on the engine
//! debug channel, tagged [`pnex_core::vision::NODE_STATUS_FORMAT`], under
//! their raw canvas id (same matching as the `pnex-display` probe).

use edgelink_core::runtime::debug_channel::DebugMessage;
use edgelink_core::runtime::model::FlowsElement;
use edgelink_core::runtime::nodes::FlowNodeBehavior;
use pnex_core::vision::{NodeStatus, NODE_STATUS_FORMAT};

/// Heartbeat period of the status of a running node.
pub const STATUS_PERIOD: std::time::Duration = std::time::Duration::from_secs(10);

/// Publishes one status of `node` (no-op when the engine is gone).
pub fn publish_status<N: FlowNodeBehavior + ?Sized>(
    node: &N,
    canvas_id: &str,
    status: &NodeStatus,
) {
    let Some(engine) = node.engine() else {
        return;
    };
    let msg = serde_json::to_value(status).unwrap_or(serde_json::Value::Null);
    let name = node.name();
    engine.debug_channel().send(DebugMessage {
        id: canvas_id.to_string(),
        name: if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        },
        msg,
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
