use super::*;

/// Current gesture (flat state machine — a single active variant).
#[derive(Clone, PartialEq)]
pub(crate) enum Interaction {
    /// No gesture.
    Idle,
    /// Background pan: client delta since the start of the gesture.
    Panning {
        start_client: (f64, f64),
        start_pan: (f64, f64),
    },
    /// Dragging a node: `grab` = cursor↔node origin offset, measured at the
    /// start of the gesture.
    Dragging {
        id: String,
        grab: (f64, f64),
        rect: (f64, f64, f64, f64),
    },
    /// Wiring from an output port (or **reverse**, from an input
    /// anchor): cursor in graph coords + hovered target (bbox hit-test on
    /// every move). In reverse, the hovered target becomes the **source** —
    /// `pin` targets the input row of a device-write or a function
    /// (None elsewhere).
    Wiring {
        from_id: String,
        port: usize,
        cursor: (f64, f64),
        hover_target: Option<String>,
        reverse: bool,
        pin: Option<String>,
    },
}

/// Runtime action requested from the toolbar (Stop / Start / Restart) —
/// a single `runtime_busy` busy signal for all three.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum RuntimeAction {
    Stop,
    Start,
    Restart,
}

/// Bundle of `Copy` signals shared between the editor and its sub-components.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct EditorCx {
    /// Graph being edited (what Save will send).
    pub(crate) graph: Signal<FlowGraph>,
    /// Dirty baseline: graph of the last saved version.
    pub(crate) saved_graph: Signal<FlowGraph>,
    /// Number of the last version known server-side.
    pub(crate) saved_version: Signal<i64>,
    /// Selected node (inspector + Delete key).
    pub(crate) selected_node: Signal<Option<String>>,
    /// Current gesture.
    pub(crate) interaction: Signal<Interaction>,
    pub(crate) pan: Signal<(f64, f64)>,
    pub(crate) zoom: Signal<f64>,
    /// Current violations (local or received in a server 400).
    pub(crate) violations: Signal<Vec<FlowViolation>>,
    /// **Staleness** violations (client-only, never persisted): pin
    /// switched to output, pin gone from the pinout, device deleted, inactive or
    /// offline — the graph is structurally valid but its read
    /// will return nothing (until the device is back).
    pub(crate) stale: Signal<Vec<FlowViolation>>,
    /// Last value published per Display node (canvas id → badge) — the
    /// live badge under the node. Cleared if the engine is stopped (the values
    /// of a stopped engine are no longer true).
    pub(crate) display_values: Signal<std::collections::HashMap<String, debug::DisplayBadge>>,
    /// Display probe expanded in pretty view: (node id, pretty JSON) — opened
    /// by clicking the canvas live badge, closed on a click outside the modal
    /// or when the engine stops (the values of a stopped engine are no
    /// longer true).
    pub(crate) expanded_display: Signal<Option<(String, String)>>,
}

impl EditorCx {
    /// Graph mutation through a pure reducer of `state.rs` — `mut self`
    /// because `Signal::with_mut` requires `&mut` (and `EditorCx` is `Copy`).
    pub(crate) fn update_graph(mut self, f: impl FnOnce(&mut FlowGraph)) {
        self.graph.with_mut(|graph| {
            f(graph);
            // json-split auto tracking: every graph mutation (editing
            // a Value/Merge, wiring/unwiring) regenerates the keys — hence
            // the number of ports — of the splits in auto mode. Idempotent:
            // nothing moves when the upstream exposes nothing new.
            state::sync_split_keys_auto(graph);
        });
    }

    /// Violations located on a given node (validation + staleness).
    pub(crate) fn violations_of(&self, node_id: &str) -> Vec<FlowViolation> {
        self.violations
            .read()
            .iter()
            .chain(self.stale.read().iter())
            .filter(|v| v.node_id.as_deref() == Some(node_id))
            .cloned()
            .collect()
    }
}
