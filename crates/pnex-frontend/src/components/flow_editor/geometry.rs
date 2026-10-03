//! Géométrie du canevas de l'éditeur de flows : fonctions **pures** (testées)
//! et la seule touche DOM, la mesure de l'origine du canevas `canvas_rect`
//! (web-sys).
//!
//! L'SVG n'a **pas** de `view_box` : sans lui, l'unité utilisateur de l'SVG =
//! 1 px CSS de l'élément, donc la conversion client→graphe est
//! `(client − origine − pan) / zoom`. L'origine est mesurée au début de
//! chaque geste (un reflow par geste, négligeable).

use dioxus_i18n::t;
use pnex_core::{FlowGraph, FlowNode, FlowNodeKind, Position};

/// Largeur et hauteur du rendu d'un nœud (px graphe).
pub const NODE_W: f64 = 170.0;
/// Hauteur **minimale** d'un nœud — les nœuds multi-ports grandissent
/// (voir `node_height`).
pub const NODE_H: f64 = 48.0;
/// Pas de la grille et du snap de position.
pub const GRID: f64 = 20.0;
/// Bornes du zoom.
pub const ZOOM_MIN: f64 = 0.4;
pub const ZOOM_MAX: f64 = 2.0;

/// Couleurs par kind (attributs SVG bruts — parité `visualisation.rs`).
pub const INJECT_FILL: &str = "#ecfdf5";
pub const INJECT_STROKE: &str = "#10b981";
pub const VALUE_FILL: &str = "#fefce8";
pub const VALUE_STROKE: &str = "#eab308";
/// Nœud device (ambre), calc (vert) et metric (rose) — Phase 6.
pub const DEVICE_FILL: &str = "#fffbeb";
pub const DEVICE_STROKE: &str = "#f59e0b";
pub const CALC_FILL: &str = "#f0fdf4";
pub const CALC_STROKE: &str = "#22c55e";
pub const METRIC_FILL: &str = "#fdf2f8";
pub const METRIC_STROKE: &str = "#ec4899";
// CoolProp : ambre (thermodynamique / chaleur).
pub const COOLPROP_FILL: &str = "#fffbeb";
pub const COOLPROP_STROKE: &str = "#f59e0b";
/// Nœud display (cyan) — sonde du panneau de debug.
pub const DISPLAY_FILL: &str = "#ecfeff";
pub const DISPLAY_STROKE: &str = "#06b6d4";
/// Cartes de régulation mixtes : TT heat/cool (orange chaud/froid) et PID
/// (indigo) — mêmes familles que device (ambre) mais distinctes au regard.
pub const REG_TT_FILL: &str = "#fff7ed";
pub const REG_TT_STROKE: &str = "#f97316";
pub const REG_PID_FILL: &str = "#eef2ff";
pub const REG_PID_STROKE: &str = "#6366f1";
pub const DEBUG_FILL: &str = "#f5f3ff";
pub const DEBUG_STROKE: &str = "#8b5cf6";
/// Nœud notify (violet doux) — bus de notification (D49).
pub const NOTIFY_FILL: &str = "#faf5ff";
pub const NOTIFY_STROKE: &str = "#a855f7";
/// Nœud http-fetch (sky) — collecte web C1a (amendement edge-model
/// 2026-09-15) ; cyan déjà pris par display.
pub const HTTP_FETCH_FILL: &str = "#f0f9ff";
pub const HTTP_FETCH_STROKE: &str = "#0ea5e9";
/// Nœuds JSON split/merge (cyan/teal) — famille transformation de données.
pub const JSON_SPLIT_FILL: &str = "#ecfeff";
pub const JSON_SPLIT_STROKE: &str = "#0891b2";
pub const JSON_MERGE_FILL: &str = "#f0fdfa";
pub const JSON_MERGE_STROKE: &str = "#0d9488";
// Camera nodes (D78): rose source, red recorder.
pub const CAMERA_FILL: &str = "#fff1f2";
pub const CAMERA_STROKE: &str = "#f43f5e";
pub const VIDEO_RECORD_FILL: &str = "#fef2f2";
pub const VIDEO_RECORD_STROKE: &str = "#ef4444";
// Vision (fuchsia) and event log (slate) nodes (D83/D84).
pub const VISION_FILL: &str = "#fdf4ff";
pub const VISION_STROKE: &str = "#d946ef";
pub const EVENT_LOG_FILL: &str = "#f8fafc";
pub const EVENT_LOG_STROKE: &str = "#64748b";
// Shared memory nodes (Valkey): lime writer, green reader.
pub const MEMORY_WRITE_FILL: &str = "#f7fee7";
pub const MEMORY_WRITE_STROKE: &str = "#65a30d";
pub const CONTROL_SOURCE_FILL: &str = "#eff6ff";
pub const CONTROL_SOURCE_STROKE: &str = "#2563eb";
pub const MEMORY_READ_FILL: &str = "#f0fdf4";
pub const MEMORY_READ_STROKE: &str = "#16a34a";
// Predictive nodes: red anomaly, violet forecast.
pub const ANOMALY_FILL: &str = "#fef2f2";
pub const ANOMALY_STROKE: &str = "#dc2626";
pub const FORECAST_FILL: &str = "#f5f3ff";
pub const FORECAST_STROKE: &str = "#7c3aed";
pub const RED_FILL: &str = "#f9fafb";
pub const RED_STROKE: &str = "#6b7280";
/// Nœud en violation (surlignage d'erreur).
pub const VIOLATION_STROKE: &str = "#dc2626";
/// Nœud sélectionné.
pub const SELECTED_STROKE: &str = "#2563eb";
/// Couleur des câbles.
pub const WIRE_STROKE: &str = "#94a3b8";

/// Arrondit à la grille (positions snapées → rendu stable).
pub fn snap(value: f64) -> f64 {
    (value / GRID).round() * GRID
}

/// Pas vertical minimal entre deux ports d'un même nœud (px graphe) : sous
/// ce pas les libellés se chevauchent — le nœud grandit plutôt que de
/// compresser (école Node-RED multi-sorties).
pub const PORT_PITCH: f64 = 20.0;

/// Port counts of a node: (outputs, decorative input anchors). Device-read:
/// one port per pin + the device-name port; device-write: the passthrough
/// output + the device-name port, and one input anchor per output pin
/// (solid dots; the wire lands on the closest row — see `port_in_near`);
/// function nodes: one labeled output per declared output (min 1, runtime
/// parity) and one input anchor per declared input; other kinds are
/// single-port.
pub fn port_counts_of(node: &FlowNode) -> (usize, usize) {
    match &node.kind {
        FlowNodeKind::DeviceRead { config } => (config.pins.len() + 1, 0),
        FlowNodeKind::DeviceWrite { config } => (2, config.pins.len()),
        FlowNodeKind::PnexFunction { config } => (config.outputs.len().max(1), config.inputs.len()),
        FlowNodeKind::PnexNotify { config } => (1, config.template_vars.len() + 1),
        FlowNodeKind::JsonSplit { config } => (config.keys.len().max(1), 0),
        FlowNodeKind::JsonMerge { config } => (1, config.inputs.len()),
        FlowNodeKind::CoolProp { config } => (config.port_count(), 2),
        // Memory read: object port + one port per key; write: passthrough.
        FlowNodeKind::MemoryRead { config } => (config.port_count(), 0),
        // Control source: event-driven, one port per listened control.
        FlowNodeKind::ControlSource { config } => (config.port_count().max(1), 0),
        // Anomaly: detail + boolean state; forecast: detail + breach + ETA.
        FlowNodeKind::Anomaly { .. } => (pnex_core::predictive::ANOMALY_PORT_COUNT, 0),
        FlowNodeKind::Forecast { .. } => (pnex_core::predictive::FORECAST_PORT_COUNT, 0),
        // Camera source: event-driven, one frame-reference output. Video
        // record: one output per written segment.
        FlowNodeKind::CameraSource { .. } | FlowNodeKind::VideoRecord { .. } => (1, 0),
        // Vision detect: one detections output; event log: passthrough.
        FlowNodeKind::VisionDetect { .. } | FlowNodeKind::EventLog { .. } => (1, 0),
        _ => (1, 0),
    }
}

/// Hauteur d'un nœud : grandit avec le nombre de ports (le max des deux
/// bords commande) pour garder `PORT_PITCH` px entre ports — jamais sous
/// `NODE_H`.
pub fn node_height(out_ports: usize, in_anchors: usize) -> f64 {
    let n = out_ports.max(in_anchors).max(1);
    (((n + 1) as f64) * PORT_PITCH).max(NODE_H)
}

/// Hauteur du rendu d'un nœud du graphe.
pub fn node_height_for(node: &FlowNode) -> f64 {
    let (out, in_anchors) = port_counts_of(node);
    node_height(out, in_anchors)
}

/// Natural pin order key: `D2` sorts before `D10` — (alpha prefix, numeric
/// suffix); labels without digits keep lexical order under their prefix.
pub fn pin_sort_key(label: &str) -> (String, u32) {
    match label.find(|c: char| c.is_ascii_digit()) {
        Some(pos) => {
            let (prefix, suffix) = label.split_at(pos);
            (prefix.to_string(), suffix.parse().unwrap_or(u32::MAX))
        }
        None => (label.to_string(), 0),
    }
}

/// Pins in natural order (`D0, D1, D2, D9, D10`) — stable on duplicates.
pub fn sorted_pins(pins: &[String]) -> Vec<String> {
    let mut out: Vec<String> = pins.to_vec();
    out.sort_by_key(|p| pin_sort_key(p));
    out
}

/// Output port labels of the anomaly / forecast nodes, in port order
/// (mirror of the runtime fan-out in `pnex-node-predict`).
pub fn predict_output_labels(kind: &FlowNodeKind) -> Vec<String> {
    match kind {
        FlowNodeKind::Anomaly { .. } => vec![
            t!("flows-predict-port-detail").to_string(),
            t!("flows-anomaly-port-flag").to_string(),
        ],
        FlowNodeKind::Forecast { .. } => vec![
            t!("flows-predict-port-detail").to_string(),
            t!("flows-forecast-port-breach").to_string(),
            t!("flows-forecast-port-eta").to_string(),
        ],
        _ => Vec::new(),
    }
}

/// Input row labels of a notify node, in anchor order: the permanent
/// **boolean gate** row `trigger` first (expects the output of a logic
/// function deciding whether the notification is sent), then the picked
/// template vars in declared order. Row 0 is always `trigger`, even with
/// no template picked.
pub fn notify_input_labels(config: &pnex_core::NotifyNodeConfig) -> Vec<String> {
    let mut labels = Vec::with_capacity(config.template_vars.len() + 1);
    labels.push(pnex_core::NOTIFY_TRIGGER_PIN.to_string());
    labels.extend(config.template_vars.iter().cloned());
    labels
}

/// Input anchor rows of a node: y offsets **relative** to the node, one per
/// input row — device-write: per configured output pin (natural sorted
/// order); function node: per declared input, **declared order** (no sort).
/// Same distribution as the device-read output ports. Empty for other kinds
/// — their input anchor stays mid-height (`port_in`).
pub fn input_anchor_rows(node: &FlowNode, h: f64) -> Vec<f64> {
    match &node.kind {
        FlowNodeKind::DeviceWrite { config } => {
            let pins = sorted_pins(&config.pins);
            let n = pins.len().max(1);
            (0..pins.len())
                .map(|i| h * (i + 1) as f64 / (n + 1) as f64)
                .collect()
        }
        FlowNodeKind::PnexFunction { config } => {
            let n = config.inputs.len().max(1);
            (0..config.inputs.len())
                .map(|i| h * (i + 1) as f64 / (n + 1) as f64)
                .collect()
        }
        FlowNodeKind::PnexNotify { config } => {
            let rows = config.template_vars.len() + 1;
            let n = rows.max(1);
            (0..rows)
                .map(|i| h * (i + 1) as f64 / (n + 1) as f64)
                .collect()
        }
        FlowNodeKind::JsonMerge { config } => {
            let n = config.inputs.len().max(1);
            (0..config.inputs.len())
                .map(|i| h * (i + 1) as f64 / (n + 1) as f64)
                .collect()
        }
        FlowNodeKind::CoolProp { .. } => vec![h / 3.0, h * 2.0 / 3.0],
        _ => Vec::new(),
    }
}

/// Row labels of the input side of a node, in the **same order** as
/// `input_anchor_rows`: device-write → natural-sorted pins; function node →
/// declared input names; notify node → `trigger` gate row + picked template
/// var names (see `notify_input_labels`). Empty for other kinds (single
/// unlabeled anchor).
pub fn input_row_labels(node: &FlowNode) -> Vec<String> {
    match &node.kind {
        FlowNodeKind::DeviceWrite { config } => sorted_pins(&config.pins),
        FlowNodeKind::PnexFunction { config } => {
            config.inputs.iter().map(|i| i.name.clone()).collect()
        }
        FlowNodeKind::PnexNotify { config } => notify_input_labels(config),
        FlowNodeKind::JsonMerge { config } => config.inputs.clone(),
        // Rows are named after the input keys (stable across pair changes).
        FlowNodeKind::CoolProp { config } => vec![config.v1_key.clone(), config.v2_key.clone()],
        _ => Vec::new(),
    }
}

/// Opaque rewire labels of the memory-read output ports: the object port,
/// then one per key.
pub fn memory_read_port_ids(keys: &[String]) -> Vec<String> {
    std::iter::once("*all".to_string())
        .chain(keys.iter().cloned())
        .collect()
}

/// Opaque rewire labels of the CoolProp output ports, in port order: the
/// all-outputs object, one per output id, then the phase port.
pub fn coolprop_port_ids(config: &pnex_core::CoolPropConfig) -> Vec<String> {
    let mut ids = Vec::with_capacity(config.port_count());
    ids.push("*all".to_string());
    ids.extend(config.outputs.iter().cloned());
    if config.include_phase {
        ids.push("*phase".to_string());
    }
    ids
}

/// Short `symbol unit` label of a CoolProp quantity (`T °C`, `h kJ/kg`);
/// quantities outside the catalogue show their raw CoolProp name.
pub fn coolprop_quantity_label(id: &str, unit_id: &str) -> String {
    match pnex_core::thermo_quantity(id) {
        Some(q) => {
            let unit = q.dimension.unit(unit_id).map(|u| u.symbol).unwrap_or("SI");
            format!("{} {unit}", q.symbol)
        }
        None => id.to_string(),
    }
}

/// Output port labels of a function node, in declared order (port i =
/// outputs[i], runtime parity via `output_count` = max(1, len)). Empty
/// declarations → one unnamed port (`[""]`), keeping the label-free look and
/// port-0 wires.
pub fn function_output_labels(outputs: &[pnex_core::FunctionOutput]) -> Vec<String> {
    if outputs.is_empty() {
        return vec![String::new()];
    }
    outputs.iter().map(|o| o.name.clone()).collect()
}

/// Output port labels of a json-split node, in declared order (port i =
/// keys[i], projection parity `outputs` = max(1, len)). No declared key →
/// one unnamed port (`[""]`) — the legacy single-port look.
pub fn split_output_labels(keys: &[String]) -> Vec<String> {
    if keys.is_empty() {
        return vec![String::new()];
    }
    keys.to_vec()
}

/// Ancres de ports : entrée au milieu du bord gauche, sortie au milieu du
/// bord droit (`h` = hauteur du nœud, voir `node_height_for`).
pub fn port_in(pos: Position, h: f64) -> (f64, f64) {
    (pos.x, pos.y + h / 2.0)
}

/// Effective input anchor for a wire landing on `node`: for a device-write
/// or a function node, the input row closest to `near_y` (absolute y of the
/// source anchor) — the wire visually lands on a row instead of mid-height,
/// between two rows; mid-height otherwise.
pub fn port_in_near(pos: Position, node: &FlowNode, h: f64, near_y: f64) -> (f64, f64) {
    let rows = input_anchor_rows(node, h);
    match rows
        .into_iter()
        .map(|rel| pos.y + rel)
        .min_by(|a, b| (a - near_y).abs().total_cmp(&(b - near_y).abs()))
    {
        Some(y) => (pos.x, y),
        None => port_in(pos, h),
    }
}

/// Landing anchor of a **plain** (un-annotated) wire into `node`: a node
/// with declared input rows claims NOTHING — the wire lands mid-height
/// (stable, never jumps between rows as nodes move; the unbound wire is
/// flagged by validation and must be re-dropped on a row). Row-less nodes
/// keep the nearest-row fallback (same point anyway).
pub fn plain_wire_landing(pos: Position, node: &FlowNode, h: f64, near_y: f64) -> (f64, f64) {
    if input_row_labels(node).is_empty() {
        port_in_near(pos, node, h, near_y)
    } else {
        port_in(pos, h)
    }
}

/// Ancre du port de sortie `port` sur un nœud à `count` ports, hauteur `h`
/// — répartis verticalement sur le bord droit.
pub fn port_out_at(pos: Position, port: usize, count: usize, h: f64) -> (f64, f64) {
    let n = count.max(1);
    (
        pos.x + NODE_W,
        pos.y + h * (port + 1) as f64 / (n + 1) as f64,
    )
}

/// Port de **sortie** le plus proche de `y` — miroir de `port_in_near` pour
/// le câblage inverse : un drop sur le corps d'un nœud multi-ports (device-
/// read) vise la rangée de port la plus proche du curseur, 0 en mono-port.
pub fn port_out_near(pos: Position, count: usize, h: f64, y: f64) -> usize {
    let n = count.max(1);
    (0..n)
        .min_by(|a, b| {
            (port_out_at(pos, *a, n, h).1 - y)
                .abs()
                .total_cmp(&(port_out_at(pos, *b, n, h).1 - y).abs())
        })
        .unwrap_or(0)
}

/// Courbe de Bézier d'un câble (a = ancre sortie source, b = ancre entrée
/// cible) — poignées horizontales, style Node-RED.
pub fn wire_path(a: (f64, f64), b: (f64, f64)) -> String {
    let dx = ((b.0 - a.0) / 2.0).max(40.0);
    format!(
        "M {} {} C {} {} {} {} {} {}",
        a.0,
        a.1,
        a.0 + dx,
        a.1,
        b.0 - dx,
        b.1,
        b.0,
        b.1
    )
}

/// Nœud sous un point graphe (hit-test bbox, du dernier dessiné vers le
/// premier — l'ordre du Vec fait l'ordre z).
pub fn node_at(graph: &FlowGraph, p: (f64, f64)) -> Option<&FlowNode> {
    graph.nodes.iter().rev().find(|node| {
        let Some(pos) = node.position else {
            return false;
        };
        let h = node_height_for(node);
        p.0 >= pos.x && p.0 <= pos.x + NODE_W && p.1 >= pos.y && p.1 <= pos.y + h
    })
}

/// Position de départ en cascade : un pas de grille par nœud déjà présent,
/// replié modulo 6 — les nouveaux nœuds ne s'empilent jamais exactement.
pub fn cascade_origin(graph: &FlowGraph, center: (f64, f64)) -> Position {
    let step = graph.nodes.len() as f64;
    Position {
        x: snap(center.0 + (step % 6.0) * GRID),
        y: snap(center.1 + (step % 6.0) * GRID),
    }
}

/// Fills missing positions (graphs created outside the editor, e.g. by the AI
/// assistant) with a left-to-right layered layout: column = depth of the node
/// in the wiring (longest path from a source), rows stacked with each node's
/// real height so multi-port nodes never overlap. Positioned nodes are kept.
pub fn ensure_positions(graph: &mut FlowGraph) {
    if graph.nodes.iter().all(|n| n.position.is_some()) {
        return;
    }
    let index: std::collections::HashMap<&str, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    // Longest path over a topological order (Kahn): nodes caught in a cycle
    // keep the depth reached through their acyclic predecessors.
    let edges: Vec<Vec<usize>> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            n.outputs
                .iter()
                .flat_map(|w| &w.targets)
                .filter_map(|t| index.get(t.as_str()).copied())
                .filter(|&t| t != i)
                .collect()
        })
        .collect();
    let mut indegree = vec![0usize; graph.nodes.len()];
    for targets in &edges {
        for &t in targets {
            indegree[t] += 1;
        }
    }
    let mut depth = vec![0usize; graph.nodes.len()];
    let mut queue: std::collections::VecDeque<usize> = (0..graph.nodes.len())
        .filter(|&i| indegree[i] == 0)
        .collect();
    while let Some(i) = queue.pop_front() {
        for &t in &edges[i] {
            depth[t] = depth[t].max(depth[i] + 1);
            indegree[t] -= 1;
            if indegree[t] == 0 {
                queue.push_back(t);
            }
        }
    }
    let mut next_y: std::collections::HashMap<usize, f64> = Default::default();
    for (i, node) in graph.nodes.iter_mut().enumerate() {
        if node.position.is_none() {
            let h = node_height_for(node);
            let y = next_y.entry(depth[i]).or_insert(80.0);
            node.position = Some(Position {
                x: 80.0 + depth[i] as f64 * (NODE_W + 90.0),
                y: *y,
            });
            *y += h + 40.0;
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub fn canvas_rect() -> Option<(f64, f64, f64, f64)> {
    let document = web_sys::window()?.document()?;
    let element = document.get_element_by_id("flow-canvas")?;
    let rect = element.get_bounding_client_rect();
    Some((rect.left(), rect.top(), rect.width(), rect.height()))
}

/// Cible native : pas de canvas (l'éditeur ne tourne qu'en CSR web pour
/// l'instant) — les gestes ne démarrent jamais sans rect.
#[cfg(not(target_arch = "wasm32"))]
pub fn canvas_rect() -> Option<(f64, f64, f64, f64)> {
    None
}

/// Convertit une position client (px écran) en position graphe.
pub fn to_graph(
    client: (f64, f64),
    rect: (f64, f64, f64, f64),
    pan: (f64, f64),
    zoom: f64,
) -> (f64, f64) {
    (
        (client.0 - rect.0 - pan.0) / zoom,
        (client.1 - rect.1 - pan.1) / zoom,
    )
}

/// Nouveau zoom appliqué vers un point fixe de l'écran (le curseur) :
/// `pan₂ = s − k·(s − pan₁)` avec `k = zoom₂ / zoom₁`.
pub fn zoom_pan_towards(
    pan: (f64, f64),
    zoom: f64,
    new_zoom: f64,
    screen: (f64, f64),
) -> (f64, f64) {
    let k = new_zoom / zoom;
    (
        screen.0 - k * (screen.0 - pan.0),
        screen.1 - k * (screen.1 - pan.1),
    )
}

/// Width of the inspector panel overlaying the right of the canvas
/// (`w-80` = 20rem).
pub const INSPECTOR_W: f64 = 320.0;
/// Room kept between a revealed output port and the inspector edge.
const REVEAL_MARGIN: f64 = 32.0;

/// Horizontal pan that brings the right edge (output ports) of a node at
/// graph x `node_x` back into the part of the canvas left uncovered by the
/// inspector. `None` = already visible, or the uncovered area is too narrow
/// to hold the node (small screens: panning would only hide its left side).
pub fn pan_to_reveal_node(node_x: f64, pan_x: f64, zoom: f64, canvas_w: f64) -> Option<f64> {
    let visible_right = canvas_w - INSPECTOR_W - REVEAL_MARGIN;
    if visible_right < (NODE_W * zoom) + REVEAL_MARGIN {
        return None;
    }
    let node_right = pan_x + (node_x + NODE_W) * zoom;
    (node_right > visible_right).then(|| pan_x - (node_right - visible_right))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::{FlowNode, FlowNodeKind, InjectConfig};

    #[test]
    fn reveal_pans_a_node_hidden_under_the_inspector() {
        // 1200 px canvas: the inspector covers x >= 880.
        assert_eq!(pan_to_reveal_node(100.0, 0.0, 1.0, 1200.0), None);
        let pan = pan_to_reveal_node(900.0, 0.0, 1.0, 1200.0).unwrap();
        let right = pan + 900.0 + NODE_W;
        assert!((right - (1200.0 - INSPECTOR_W - REVEAL_MARGIN)).abs() < 1e-9);
        // Too narrow to hold the node next to the inspector: no pan.
        assert_eq!(pan_to_reveal_node(900.0, 0.0, 1.0, 400.0), None);
    }

    fn inject(id: &str, x: f64, y: f64) -> FlowNode {
        FlowNode {
            id: id.into(),
            name: None,
            position: Some(Position { x, y }),
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::Inject {
                config: InjectConfig {
                    once_delay_secs: Some(1.0),
                    ..Default::default()
                },
            },
        }
    }

    #[test]
    fn snap_sur_la_grille() {
        assert_eq!(snap(7.0), 0.0);
        assert_eq!(snap(13.0), 20.0);
        assert_eq!(snap(-7.0), 0.0);
        assert_eq!(snap(160.0), 160.0);
    }

    #[test]
    fn ancres_de_ports() {
        let pos = Position { x: 100.0, y: 60.0 };
        assert_eq!(port_in(pos, NODE_H), (100.0, 84.0));
        // The output anchor is computed via `port_out_at` (multi-port aware).
        assert_eq!(port_out_at(pos, 0, 1, NODE_H), (270.0, 84.0));
    }

    #[test]
    fn fil_nu_nuit_jamais_une_rangee() {
        // Un fil nu sur un nœud à lignes nommées atterrit à mi-hauteur :
        // jamais de « rangée la plus proche » (le fil sautait de port au
        // moindre déplacement). Point stable, indépendant de near_y.
        let pos = Position { x: 100.0, y: 60.0 };
        let merge = FlowNode {
            id: "mg".into(),
            name: None,
            position: Some(pos),
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::JsonMerge {
                config: pnex_core::JsonMergeConfig {
                    default_key: "value".into(),
                    inputs: vec!["temp".into(), "hum".into()],
                },
            },
        };
        let h = node_height_for(&merge);
        assert_eq!(
            plain_wire_landing(pos, &merge, h, 10.0),
            plain_wire_landing(pos, &merge, h, 10_000.0),
            "mi-hauteur stable"
        );
        assert_eq!(
            plain_wire_landing(pos, &merge, h, 10.0),
            (100.0, 60.0 + h / 2.0),
            "jamais sur une rangée"
        );
    }

    #[test]
    fn ports_multiples_repartis() {
        let pos = Position { x: 100.0, y: 60.0 };
        // 2 ports : nœud grandi à 60 px (3 × PORT_PITCH), pas de 20 px ;
        // port 1 = « nom du device » du device-read.
        let h = node_height(2, 0);
        assert_eq!(h, 60.0);
        assert_eq!(port_out_at(pos, 0, 2, h), (270.0, 80.0));
        assert_eq!(port_out_at(pos, 1, 2, h), (270.0, 100.0));
        // 1 port = comportement mono-port historique.
        assert_eq!(port_out_at(pos, 0, 1, NODE_H), (270.0, 84.0));
    }

    #[test]
    fn hauteur_dynamique_selon_ports() {
        // Mono-port : hauteur historique.
        assert_eq!(node_height(1, 0), NODE_H);
        // Grandit d'un pas par port supplémentaire, jamais sous NODE_H.
        assert_eq!(node_height(2, 0), 60.0);
        assert_eq!(node_height(6, 0), 140.0);
        // Le bord le plus peuplé commande (ancres device-write).
        assert_eq!(node_height(1, 5), 120.0);
        assert!(node_height(12, 0) >= 13.0 * PORT_PITCH);
    }

    #[test]
    fn comptes_de_ports_par_kind() {
        use pnex_core::{DeviceReadConfig, DeviceWriteConfig};
        let read = FlowNode {
            id: "r".into(),
            name: None,
            position: None,
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::DeviceRead {
                config: DeviceReadConfig {
                    device_id: "d".into(),
                    pins: vec!["D0".into(), "D1".into()],
                    window_secs: 60.0,
                },
            },
        };
        assert_eq!(port_counts_of(&read), (3, 0)); // 2 pins + « nom du device »
        assert_eq!(node_height_for(&read), 80.0);
        let write = FlowNode {
            id: "w".into(),
            name: None,
            position: None,
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::DeviceWrite {
                config: DeviceWriteConfig {
                    device_id: "d".into(),
                    ..Default::default()
                },
            },
        };
        // DeviceWrite sans pin : passthrough + port « nom du device » ; le
        // pas vient des ancres sinon.
        assert_eq!(port_counts_of(&write), (2, 0));
        assert_eq!(node_height_for(&write), 60.0);
    }

    #[test]
    fn natural_pin_order() {
        // D2 before D10 — plain lexical sort would put D10 first.
        let pins: Vec<String> = ["D10", "D2", "D9", "D1", "A7"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(sorted_pins(&pins), vec!["A7", "D1", "D2", "D9", "D10"]);
    }

    /// Function node builder for tests: declared inputs/outputs by name.
    fn function_node(id: &str, inputs: &[&str], outputs: &[&str]) -> FlowNode {
        let finput = |name: &str| pnex_core::FunctionInput {
            name: name.into(),
            ty: pnex_core::FunctionType::Number,
            default: None,
            desc: None,
        };
        let foutput = |name: &str| pnex_core::FunctionOutput {
            name: name.into(),
            ty: pnex_core::FunctionType::Number,
            desc: None,
        };
        FlowNode {
            id: id.into(),
            name: None,
            position: Some(Position { x: 0.0, y: 0.0 }),
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::PnexFunction {
                config: pnex_core::FunctionNodeConfig {
                    function_id: 1,
                    function_name: "f".into(),
                    version_number: 1,
                    language: pnex_core::FunctionLanguage::Js,
                    inputs: inputs.iter().map(|n| finput(n)).collect(),
                    outputs: outputs.iter().map(|n| foutput(n)).collect(),
                },
            },
        }
    }

    #[test]
    fn function_port_counts_and_rows() {
        let f = function_node("f", &["a", "b"], &["x", "y"]);
        // 2 labeled output ports; 2 input anchors; the taller side drives
        // the height (same pitch rule as devices).
        assert_eq!(port_counts_of(&f), (2, 2));
        assert_eq!(node_height_for(&f), 60.0);
        // Input rows in **declared order** (a then b) — no natural sort.
        let h = node_height_for(&f);
        assert_eq!(input_anchor_rows(&f, h), vec![20.0, 40.0]);
        assert_eq!(input_row_labels(&f), vec!["a".to_string(), "b".to_string()]);
        // A wire landing between rows snaps to the nearest one (rows at
        // h/3 = 20 and 2h/3 = 40; 32 is closer to 40 — the "b" row).
        let near = port_in_near(f.position.unwrap(), &f, h, 32.0);
        assert_eq!(near.1, 40.0);
        // Single input, single output: mono-port height, one row.
        let mono = function_node("f1", &["a"], &["x"]);
        assert_eq!(port_counts_of(&mono), (1, 1));
        assert_eq!(node_height_for(&mono), NODE_H);
        // No declared inputs → no rows, the single mid-height anchor stays.
        let no_in = function_node("f2", &[], &["x"]);
        assert_eq!(port_counts_of(&no_in), (1, 0));
        assert!(input_anchor_rows(&no_in, NODE_H).is_empty());
        assert!(input_row_labels(&no_in).is_empty());
    }

    #[test]
    fn notify_port_counts_and_rows() {
        let notify = |vars: &[&str]| FlowNode {
            id: "nt".into(),
            name: None,
            position: Some(Position { x: 0.0, y: 0.0 }),
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::PnexNotify {
                config: pnex_core::NotifyNodeConfig {
                    template_vars: vars.iter().map(|s| (*s).to_string()).collect(),
                    ..Default::default()
                },
            },
        };
        // One anchor per template var (declared order) + the permanent
        // `trigger` gate row on top, same pitch rule as functions.
        let two = notify(&["seuil", "value"]);
        assert_eq!(port_counts_of(&two), (1, 3));
        let h = node_height_for(&two);
        assert_eq!(input_anchor_rows(&two, h), vec![20.0, 40.0, 60.0]);
        assert_eq!(
            input_row_labels(&two),
            vec![
                "trigger".to_string(),
                "seuil".to_string(),
                "value".to_string()
            ]
        );
        let near = port_in_near(two.position.unwrap(), &two, h, 52.0);
        assert_eq!(near.1, 60.0, "snaps to the closest row");
        // No template picked → the trigger gate row alone (mid-height).
        let bare = notify(&[]);
        assert_eq!(port_counts_of(&bare), (1, 1));
        let h_bare = node_height_for(&bare);
        assert_eq!(input_anchor_rows(&bare, h_bare), vec![h_bare / 2.0]);
        assert_eq!(input_row_labels(&bare), vec!["trigger".to_string()]);
    }

    #[test]
    fn function_output_labels_empty_when_none() {
        // No declared outputs → one unnamed port ([""] convention): the
        // runtime still exposes port 0 (output_count = max(1, len)).
        assert_eq!(function_output_labels(&[]), vec![String::new()]);
        let outs = vec![
            pnex_core::FunctionOutput {
                name: "x".into(),
                ty: pnex_core::FunctionType::Number,
                desc: None,
            },
            pnex_core::FunctionOutput {
                name: "y".into(),
                ty: pnex_core::FunctionType::Number,
                desc: None,
            },
        ];
        assert_eq!(
            function_output_labels(&outs),
            vec!["x".to_string(), "y".to_string()]
        );
    }

    #[test]
    fn write_anchor_rows_and_snap() {
        use pnex_core::DeviceWriteConfig;
        let mut write = FlowNode {
            id: "w".into(),
            name: None,
            position: Some(Position { x: 300.0, y: 100.0 }),
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::DeviceWrite {
                config: DeviceWriteConfig {
                    device_id: "d".into(),
                    pins: vec!["D2".into(), "D3".into(), "D10".into(), "D9".into()],
                    ..Default::default()
                },
            },
        };
        let h = node_height_for(&write);
        // 4 anchors: 5 × PORT_PITCH = 100 px, evenly distributed. Natural
        // sorted order (D9 before D10): row 0 = D2, last row = D10.
        let rows = input_anchor_rows(&write, h);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0], 20.0);
        assert_eq!(rows[3], 80.0);
        let pins = sorted_pins(&[
            "D2".to_string(),
            "D3".to_string(),
            "D10".to_string(),
            "D9".to_string(),
        ]);
        assert_eq!(pins[pins.len() - 1], "D10");
        // Snap: absolute rows at 120/140/160/180 (pos.y = 100) — a source at
        // y=55 lands on the closest row (120), never mid-height (150).
        let pos = write.position.unwrap();
        let near = port_in_near(pos, &write, h, 55.0);
        assert_eq!(near.0, 300.0);
        assert_eq!(near.1, 120.0);
        // A source BELOW the node lands on the last row (180), not the
        // first — distances are absolute, not signed.
        let below = port_in_near(pos, &write, h, 260.0);
        assert_eq!(below.1, 180.0);
        // A node without rows (inject) keeps mid-height.
        let inj = inject("i", 0.0, 0.0);
        assert_eq!(
            port_in_near(inj.position.unwrap(), &inj, NODE_H, 999.0),
            port_in(inj.position.unwrap(), NODE_H)
        );
        // A single pin: row = mid-height (historical behavior).
        write.kind = FlowNodeKind::DeviceWrite {
            config: DeviceWriteConfig {
                device_id: "d".into(),
                pins: vec!["D2".into()],
                ..Default::default()
            },
        };
        let h1 = node_height_for(&write);
        assert_eq!(input_anchor_rows(&write, h1), vec![h1 / 2.0]);
        assert_eq!(
            port_in_near(write.position.unwrap(), &write, h1, 0.0),
            (300.0, 100.0 + h1 / 2.0)
        );
    }

    #[test]
    fn port_sortie_la_plus_proche() {
        // device-read 2 ports à (100, 60), h = 60 : ports à y 80 et 100.
        let pos = Position { x: 100.0, y: 60.0 };
        let h = node_height(2, 0);
        assert_eq!(port_out_near(pos, 2, h, 82.0), 0);
        assert_eq!(port_out_near(pos, 2, h, 98.0), 1);
        // Mono-port : toujours 0, quel que soit y.
        assert_eq!(port_out_near(pos, 1, NODE_H, 999.0), 0);
    }

    #[test]
    fn path_bezier() {
        let path = wire_path((0.0, 0.0), (200.0, 100.0));
        assert!(path.starts_with("M 0 0 C "), "{path}");
        assert!(path.ends_with("200 100"), "{path}");
    }

    #[test]
    fn hit_test_topmost() {
        let mut graph = FlowGraph {
            nodes: vec![inject("a", 0.0, 0.0), inject("b", 20.0, 20.0)],
        };
        assert_eq!(
            node_at(&graph, (30.0, 30.0)).map(|n| n.id.as_str()),
            Some("b")
        );
        assert_eq!(
            node_at(&graph, (5.0, 5.0)).map(|n| n.id.as_str()),
            Some("a")
        );
        assert_eq!(node_at(&graph, (400.0, 400.0)), None);
        // Sans position : jamais atteint.
        graph.nodes.push(FlowNode {
            id: "c".into(),
            name: None,
            position: None,
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::Debug {
                config: Default::default(),
            },
        });
        assert_eq!(node_at(&graph, (90.0, 90.0)), None);
    }

    #[test]
    fn conversion_client_vers_graphe() {
        let rect = (10.0, 20.0, 800.0, 600.0);
        let pan = (100.0, 50.0);
        assert_eq!(to_graph((110.0, 70.0), rect, pan, 1.0), (0.0, 0.0));
        assert_eq!(to_graph((210.0, 170.0), rect, pan, 2.0), (50.0, 50.0));
    }

    #[test]
    fn zoom_conserve_le_point_sous_curseur() {
        let pan = (100.0, 50.0);
        let zoom = 1.0;
        let new_zoom = 2.0;
        let screen = (300.0, 250.0);
        let new_pan = zoom_pan_towards(pan, zoom, new_zoom, screen);
        // Le point écran reste sur le même point graphe.
        let before = to_graph(screen, (0.0, 0.0, 0.0, 0.0), pan, zoom);
        let after = to_graph(screen, (0.0, 0.0, 0.0, 0.0), new_pan, new_zoom);
        assert!((before.0 - after.0).abs() < 1e-9);
        assert!((before.1 - after.1).abs() < 1e-9);
    }

    #[test]
    fn positions_manquantes_remplies() {
        let mut graph = FlowGraph {
            nodes: vec![inject("a", 0.0, 0.0)],
        };
        graph.nodes.push(FlowNode {
            id: "b".into(),
            name: None,
            position: None,
            outputs: vec![],
            inputs: vec![],
            kind: FlowNodeKind::Debug {
                config: Default::default(),
            },
        });
        ensure_positions(&mut graph);
        assert!(graph.nodes[1].position.is_some());
    }

    #[test]
    fn missing_positions_are_laid_out_by_depth() {
        let node = |id: &str, targets: &[&str]| FlowNode {
            id: id.into(),
            name: None,
            position: None,
            outputs: if targets.is_empty() {
                vec![]
            } else {
                vec![pnex_core::FlowWiring {
                    port: 0,
                    targets: targets.iter().map(|t| t.to_string()).collect(),
                }]
            },
            inputs: vec![],
            kind: FlowNodeKind::Debug {
                config: Default::default(),
            },
        };
        // a → b → c, a → c (c at depth 2), d isolated.
        let mut graph = FlowGraph {
            nodes: vec![
                node("a", &["b", "c"]),
                node("b", &["c"]),
                node("c", &[]),
                node("d", &[]),
            ],
        };
        ensure_positions(&mut graph);
        let x = |i: usize| graph.nodes[i].position.unwrap().x;
        assert!(
            x(0) < x(1) && x(1) < x(2),
            "columns follow the wiring depth"
        );
        assert_eq!(x(3), x(0), "isolated node stays in the first column");
        let (ya, yd) = (
            graph.nodes[0].position.unwrap().y,
            graph.nodes[3].position.unwrap().y,
        );
        assert!(yd > ya, "same column: stacked, not overlapping");

        // A cycle (b ↔ c) terminates and still positions every node.
        let mut cyclic = FlowGraph {
            nodes: vec![node("a", &["b"]), node("b", &["c"]), node("c", &["b"])],
        };
        ensure_positions(&mut cyclic);
        assert!(cyclic.nodes.iter().all(|n| n.position.is_some()));
    }
}
