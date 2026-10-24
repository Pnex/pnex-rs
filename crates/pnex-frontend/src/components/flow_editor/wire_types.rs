//! Data type carried by each output port, shown as a colour on the port and
//! a small label on every wire (O70: a non-developer must see what flows
//! where). Mirror of the runtime fan-out of each node; passthrough ports
//! take the type of what enters the node.

use dioxus_i18n::t;
use pnex_core::{FlowGraph, FlowNode, FlowNodeKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DataType {
    Number,
    Bool,
    Text,
    Object,
    Video,
    Audio,
    Detections,
    Any,
}

impl DataType {
    pub(crate) fn of_value(v: &serde_json::Value) -> Self {
        match v {
            serde_json::Value::Number(_) => Self::Number,
            serde_json::Value::Bool(_) => Self::Bool,
            serde_json::Value::String(_) => Self::Text,
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => Self::Object,
            serde_json::Value::Null => Self::Any,
        }
    }

    /// Port and wire colour (numbered scales are mirrored by dark.css).
    pub(crate) fn color(self) -> &'static str {
        match self {
            Self::Number => "var(--color-indigo-500, #6366f1)",
            Self::Bool => "var(--color-amber-500, #f59e0b)",
            Self::Text => "var(--color-emerald-500, #10b981)",
            Self::Object => "var(--color-violet-500, #8b5cf6)",
            Self::Video => "var(--color-rose-500, #f43f5e)",
            Self::Audio => "var(--color-cyan-500, #06b6d4)",
            Self::Detections => "var(--color-orange-500, #f97316)",
            Self::Any => "var(--color-slate-400, #94a3b8)",
        }
    }

    /// Short wire label; empty for `Any` (nothing useful to say).
    pub(crate) fn label(self) -> String {
        match self {
            Self::Number => t!("flow-type-number").to_string(),
            Self::Bool => t!("flow-type-bool").to_string(),
            Self::Text => t!("flow-type-text").to_string(),
            Self::Object => t!("flow-type-object").to_string(),
            Self::Video => t!("flow-type-video").to_string(),
            Self::Audio => t!("flow-type-audio").to_string(),
            Self::Detections => t!("flow-type-detections").to_string(),
            Self::Any => String::new(),
        }
    }
}

/// What a port emits: a fixed type, or whatever entered the node.
enum PortType {
    Fixed(DataType),
    Passthrough,
}

fn port_type(node: &FlowNode, port: usize) -> PortType {
    use DataType::*;
    use PortType::{Fixed, Passthrough};
    match &node.kind {
        // Null payload = the injection timestamp (ms).
        FlowNodeKind::Inject { config } if config.payload.is_null() => Fixed(Number),
        FlowNodeKind::Inject { config } => Fixed(DataType::of_value(&config.payload)),
        FlowNodeKind::DeviceRead { config } if port < config.pins.len() => Fixed(Number),
        FlowNodeKind::DeviceRead { .. } => Fixed(Text),
        FlowNodeKind::DeviceWrite { .. } if port == 0 => Passthrough,
        FlowNodeKind::DeviceWrite { .. } => Fixed(Text),
        FlowNodeKind::Calc { .. } => Fixed(Number),
        FlowNodeKind::Value { config } => Fixed(DataType::of_value(&config.value)),
        FlowNodeKind::CoolProp { .. } if port == 0 => Fixed(Object),
        FlowNodeKind::CoolProp { config } if port <= config.outputs.len() => Fixed(Number),
        FlowNodeKind::CoolProp { .. } => Fixed(Text),
        FlowNodeKind::PnexFunction { config } => match config.outputs.get(port).map(|o| o.ty) {
            Some(pnex_core::FunctionType::Number) => Fixed(Number),
            Some(pnex_core::FunctionType::String) => Fixed(Text),
            Some(pnex_core::FunctionType::Bool) => Fixed(Bool),
            _ => Fixed(Any),
        },
        FlowNodeKind::JsonMerge { .. } => Fixed(Object),
        FlowNodeKind::CameraSource { .. } => Fixed(Video),
        FlowNodeKind::MediaSource { .. } => Fixed(Audio),
        FlowNodeKind::VisionDetect { .. } => Fixed(Detections),
        FlowNodeKind::VisionTrack { .. } if port == 0 => Fixed(Detections),
        FlowNodeKind::VideoRecord { .. }
        | FlowNodeKind::VisionTrack { .. }
        | FlowNodeKind::VisionZone { .. }
        | FlowNodeKind::VisionCaption { .. }
        | FlowNodeKind::MediaFuse { .. }
        | FlowNodeKind::LlmText { .. }
        | FlowNodeKind::PagePdf { .. }
        | FlowNodeKind::PageWrite { .. }
        | FlowNodeKind::ActionTrigger { .. }
        | FlowNodeKind::OntologyQuery { .. }
        | FlowNodeKind::ObjectUpsert { .. }
        | FlowNodeKind::LinkUpsert { .. }
        | FlowNodeKind::Ingest { .. }
        | FlowNodeKind::SiteRecipe { .. }
        | FlowNodeKind::WebSearch { .. }
        | FlowNodeKind::OrgResolve { .. }
        | FlowNodeKind::WebVisit { .. }
        | FlowNodeKind::SiteCrawl { .. }
        | FlowNodeKind::Weather { .. } => Fixed(Object),
        FlowNodeKind::MemoryRead { .. } if port == 0 => Fixed(Object),
        FlowNodeKind::ControlSource { .. } => Fixed(Number),
        FlowNodeKind::Anomaly { .. } | FlowNodeKind::Forecast { .. } if port == 0 => Fixed(Object),
        FlowNodeKind::Anomaly { .. } | FlowNodeKind::Forecast { .. } if port == 1 => Fixed(Bool),
        FlowNodeKind::Forecast { .. } => Fixed(Number),
        FlowNodeKind::Display { .. }
        | FlowNodeKind::Metric { .. }
        | FlowNodeKind::EventLog { .. }
        | FlowNodeKind::MemoryWrite { .. }
        | FlowNodeKind::TopicClassify { .. }
        | FlowNodeKind::TextDecide { .. }
        | FlowNodeKind::PnexNotify { .. } => Passthrough,
        _ => Fixed(Any),
    }
}

/// Type emitted by `port` of `node_id`; a passthrough follows the first
/// wire entering the node (bounded: a cycle resolves to `Any`).
pub(crate) fn output_type(graph: &FlowGraph, node_id: &str, port: usize) -> DataType {
    let mut current = (node_id.to_string(), port);
    for _ in 0..16 {
        let Some(node) = graph.nodes.iter().find(|n| n.id == current.0) else {
            return DataType::Any;
        };
        match port_type(node, current.1) {
            PortType::Fixed(t) => return t,
            PortType::Passthrough => {
                let Some(source) = incoming(graph, &node.id) else {
                    return DataType::Any;
                };
                current = source;
            }
        }
    }
    DataType::Any
}

/// First `(source node, port)` wired into `target`.
fn incoming(graph: &FlowGraph, target: &str) -> Option<(String, usize)> {
    graph.nodes.iter().find_map(|n| {
        n.outputs
            .iter()
            .find(|w| w.targets.iter().any(|t| t == target))
            .map(|w| (n.id.clone(), w.port))
            .or_else(|| {
                graph
                    .nodes
                    .iter()
                    .find(|t| t.id == target)
                    .and_then(|t| t.inputs.iter().find(|i| i.from == n.id))
                    .map(|i| (n.id.clone(), i.from_port))
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(json: serde_json::Value) -> FlowGraph {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn passthrough_takes_the_upstream_type() {
        let g = graph(serde_json::json!({"nodes": [
            {"id": "a", "kind": "inject", "config": {"repeat_secs": 5, "payload": true},
             "outputs": [{"port": 0, "targets": ["b"]}]},
            {"id": "b", "kind": "display", "config": null, "outputs": [{"port": 0, "targets": ["c"]}]},
            {"id": "c", "kind": "debug", "config": {}},
            {"id": "k", "kind": "calc", "config": {"expression": "1"}},
        ]}));
        assert_eq!(output_type(&g, "a", 0), DataType::Bool);
        assert_eq!(output_type(&g, "b", 0), DataType::Bool);
        assert_eq!(output_type(&g, "k", 0), DataType::Number);
    }

    #[test]
    fn a_passthrough_cycle_ends_as_any() {
        let g = graph(serde_json::json!({"nodes": [
            {"id": "x", "kind": "display", "config": null, "outputs": [{"port": 0, "targets": ["y"]}]},
            {"id": "y", "kind": "display", "config": null, "outputs": [{"port": 0, "targets": ["x"]}]},
        ]}));
        assert_eq!(output_type(&g, "x", 0), DataType::Any);
    }
}
