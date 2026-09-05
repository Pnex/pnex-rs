//! Géométrie du canevas de l'éditeur de flows : fonctions **pures** (testées)
//! et la seule touche DOM, la mesure de l'origine du canevas `canvas_rect`
//! (web-sys).
//!
//! L'SVG n'a **pas** de `view_box` : sans lui, l'unité utilisateur de l'SVG =
//! 1 px CSS de l'élément, donc la conversion client→graphe est
//! `(client − origine − pan) / zoom`. L'origine est mesurée au début de
//! chaque geste (un reflow par geste, négligeable).

use pnex_core::{FlowGraph, FlowNode, Position};

/// Largeur et hauteur du rendu d'un nœud (px graphe).
pub const NODE_W: f64 = 170.0;
pub const NODE_H: f64 = 48.0;
/// Pas de la grille et du snap de position.
pub const GRID: f64 = 20.0;
/// Bornes du zoom.
pub const ZOOM_MIN: f64 = 0.4;
pub const ZOOM_MAX: f64 = 2.0;

/// Couleurs par kind (attributs SVG bruts — parité `visualisation.rs`).
pub const INJECT_FILL: &str = "#ecfdf5";
pub const INJECT_STROKE: &str = "#10b981";
pub const SQL_FILL: &str = "#eff6ff";
pub const SQL_STROKE: &str = "#3b82f6";
/// Nœud device (ambre), calc (vert) et metric (rose) — Phase 6.
pub const DEVICE_FILL: &str = "#fffbeb";
pub const DEVICE_STROKE: &str = "#f59e0b";
pub const CALC_FILL: &str = "#f0fdf4";
pub const CALC_STROKE: &str = "#22c55e";
pub const METRIC_FILL: &str = "#fdf2f8";
pub const METRIC_STROKE: &str = "#ec4899";
/// Nœud display (cyan) — sonde du panneau de debug.
pub const DISPLAY_FILL: &str = "#ecfeff";
pub const DISPLAY_STROKE: &str = "#06b6d4";
pub const DEBUG_FILL: &str = "#f5f3ff";
pub const DEBUG_STROKE: &str = "#8b5cf6";
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

/// Ancres de ports : entrée au milieu du bord gauche, sortie au milieu du
/// bord droit.
pub fn port_in(pos: Position) -> (f64, f64) {
    (pos.x, pos.y + NODE_H / 2.0)
}

pub fn port_out(pos: Position) -> (f64, f64) {
    (pos.x + NODE_W, pos.y + NODE_H / 2.0)
}

/// Courbe de Bézier d'un câble (a = ancre sortie source, b = ancre entrée
/// cible) — poignées horizontales, style Node-RED.
pub fn wire_path(a: (f64, f64), b: (f64, f64)) -> String {
    let dx = ((b.0 - a.0) / 2.0).max(40.0);
    format!("M {} {} C {} {} {} {} {} {}", a.0, a.1, a.0 + dx, a.1, b.0 - dx, b.1, b.0, b.1)
}

/// Nœud sous un point graphe (hit-test bbox, du dernier dessiné vers le
/// premier — l'ordre du Vec fait l'ordre z).
pub fn node_at(graph: &FlowGraph, p: (f64, f64)) -> Option<&FlowNode> {
    graph
        .nodes
        .iter()
        .rev()
        .find(|node| {
            let Some(pos) = node.position else {
                return false;
            };
            p.0 >= pos.x
                && p.0 <= pos.x + NODE_W
                && p.1 >= pos.y
                && p.1 <= pos.y + NODE_H
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

/// Complète les positions manquantes (graphes créés hors éditeur) : colonne
/// déterministe à gauche du canevas.
pub fn ensure_positions(graph: &mut FlowGraph) {
    let mut next_y = 80.0;
    for node in &mut graph.nodes {
        if node.position.is_none() {
            node.position = Some(Position { x: 80.0, y: next_y });
            next_y += NODE_H + 40.0;
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
pub fn zoom_pan_towards(pan: (f64, f64), zoom: f64, new_zoom: f64, screen: (f64, f64)) -> (f64, f64) {
    let k = new_zoom / zoom;
    (
        screen.0 - k * (screen.0 - pan.0),
        screen.1 - k * (screen.1 - pan.1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::{FlowNode, FlowNodeKind, InjectConfig};

    fn inject(id: &str, x: f64, y: f64) -> FlowNode {
        FlowNode {
            id: id.into(),
            name: None,
            position: Some(Position { x, y }),
            outputs: vec![],
            kind: FlowNodeKind::Inject {
                config: InjectConfig { once_delay_secs: Some(1.0), ..Default::default() },
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
        assert_eq!(port_in(pos), (100.0, 84.0));
        assert_eq!(port_out(pos), (270.0, 84.0));
    }

    #[test]
    fn path_bezier() {
        let path = wire_path((0.0, 0.0), (200.0, 100.0));
        assert!(path.starts_with("M 0 0 C "), "{path}");
        assert!(path.ends_with("200 100"), "{path}");
    }

    #[test]
    fn hit_test_topmost() {
        let mut graph = FlowGraph::default();
        graph.nodes = vec![inject("a", 0.0, 0.0), inject("b", 20.0, 20.0)];
        assert_eq!(node_at(&graph, (30.0, 30.0)).map(|n| n.id.as_str()), Some("b"));
        assert_eq!(node_at(&graph, (5.0, 5.0)).map(|n| n.id.as_str()), Some("a"));
        assert_eq!(node_at(&graph, (400.0, 400.0)), None);
        // Sans position : jamais atteint.
        graph.nodes.push(FlowNode {
            id: "c".into(),
            name: None,
            position: None,
            outputs: vec![],
            kind: FlowNodeKind::Debug { config: Default::default() },
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
        let mut graph = FlowGraph::default();
        graph.nodes = vec![inject("a", 0.0, 0.0)];
        graph.nodes.push(FlowNode {
            id: "b".into(),
            name: None,
            position: None,
            outputs: vec![],
            kind: FlowNodeKind::Debug { config: Default::default() },
        });
        ensure_positions(&mut graph);
        assert!(graph.nodes[1].position.is_some());
    }
}
