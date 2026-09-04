//! État et réducteurs purs de l'éditeur de flows — toutes les mutations du
//! graphe passent par ces fonctions pures (testées), jamais in-line dans le
//! RSX.

use pnex_core::{
    DebugConfig, FlowGraph, FlowNode, FlowNodeKind, FlowWiring, InjectConfig, PnexSqlConfig,
    Position,
};

/// Entrée de palette : kind + libellés i18n + couleur d'icône.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaletteKind {
    Inject,
    PnexSql,
    Debug,
    Red,
}

/// Crée un nœud neuf d'un kind de palette, avec sa config par défaut :
/// inject a un déclencheur initial (`once_delay_secs`, sinon violation
/// `no_trigger`), pnex-sql une requête valide (la config est **requise** en
/// serde), red un type vide (viole volontairement `bad_red_node` jusqu'à la
/// saisie — le bandeau guide l'utilisateur).
pub fn make_node(id: &str, kind: PaletteKind, pos: Position) -> FlowNode {
    FlowNode {
        id: id.to_string(),
        name: None,
        position: Some(pos),
        outputs: vec![],
        kind: match kind {
            PaletteKind::Inject => FlowNodeKind::Inject {
                config: InjectConfig { once_delay_secs: Some(1.0), ..Default::default() },
            },
            PaletteKind::PnexSql => FlowNodeKind::PnexSql {
                config: PnexSqlConfig { query: "SELECT 1".into(), params: vec![] },
            },
            PaletteKind::Debug => FlowNodeKind::Debug { config: DebugConfig::default() },
            PaletteKind::Red => FlowNodeKind::Red { type_name: String::new(), config: Default::default() },
        },
    }
}

/// Prochain id libre : `n{max(suffixes numériques)+1}` — jamais de collision
/// même après plusieurs ajouts/suppressions.
pub fn next_node_id(graph: &FlowGraph) -> String {
    let mut max = 0u32;
    for node in &graph.nodes {
        if let Some(n) = node.id.strip_prefix('n') {
            if let Ok(value) = n.parse::<u32>() {
                max = max.max(value);
            }
        }
    }
    format!("n{}", max + 1)
}

fn wiring_mut(outputs: &mut Vec<FlowWiring>, port: usize) -> &mut FlowWiring {
    match outputs.iter().position(|w| w.port == port) {
        Some(index) => &mut outputs[index],
        None => {
            outputs.push(FlowWiring { port, targets: vec![] });
            outputs.last_mut().expect("wiring poussé")
        }
    }
}

/// Déplace un nœud (drag) — no-op si l'id est inconnu.
pub fn move_node(graph: &mut FlowGraph, id: &str, pos: Position) {
    if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == id) {
        node.position = Some(pos);
    }
}

/// Câble depuis `(from, port)` vers `to` — idempotent.
pub fn add_target(graph: &mut FlowGraph, from: &str, port: usize, to: &str) {
    let Some(source) = graph.nodes.iter_mut().find(|n| n.id == from) else {
        return;
    };
    let targets = &mut wiring_mut(&mut source.outputs, port).targets;
    if !targets.iter().any(|t| t == to) {
        targets.push(to.to_string());
    }
}

/// Coupe le câble `(from, port) → to` (s'il existe).
pub fn remove_target(graph: &mut FlowGraph, from: &str, port: usize, to: &str) {
    if let Some(source) = graph.nodes.iter_mut().find(|n| n.id == from) {
        for w in &mut source.outputs {
            if w.port == port {
                w.targets.retain(|t| t != to);
            }
        }
        source.outputs.retain(|w| !w.targets.is_empty());
    }
}

/// Supprime un nœud **et** tous les câbles entrants qui le visaient.
pub fn remove_node(graph: &mut FlowGraph, id: &str) {
    graph.nodes.retain(|n| n.id != id);
    for node in &mut graph.nodes {
        for w in &mut node.outputs {
            w.targets.retain(|t| t != id);
        }
        node.outputs.retain(|w| !w.targets.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph_with_two() -> FlowGraph {
        let mut graph = FlowGraph::default();
        graph.nodes = vec![
            make_node("n1", PaletteKind::Inject, Position { x: 0.0, y: 0.0 }),
            make_node("n2", PaletteKind::Debug, Position { x: 200.0, y: 0.0 }),
        ];
        graph
    }

    #[test]
    fn ids_uniques_meme_apres_suppression() {
        let mut graph = graph_with_two();
        assert_eq!(next_node_id(&graph), "n3");
        remove_node(&mut graph, "n2");
        // n2 supprimé : l'id est réutilisable, aucun conflit possible.
        assert_eq!(next_node_id(&graph), "n2");
    }

    #[test]
    fn make_node_inject_a_un_declencheur() {
        let node = make_node("n9", PaletteKind::Inject, Position { x: 0.0, y: 0.0 });
        match &node.kind {
            FlowNodeKind::Inject { config } => {
                assert!(config.once_delay_secs.is_some());
                assert_eq!(next_node_id(&FlowGraph { nodes: vec![node] }), "n10");
            }
            other => panic!("inject attendu, reçu {other:?}"),
        }
    }

    #[test]
    fn make_node_sql_est_valide() {
        let node = make_node("n1", PaletteKind::PnexSql, Position { x: 0.0, y: 0.0 });
        match &node.kind {
            FlowNodeKind::PnexSql { config } => assert!(!config.query.is_empty()),
            other => panic!("pnex_sql attendu, reçu {other:?}"),
        }
        assert!(pnex_core::validate_graph(&FlowGraph { nodes: vec![node] }).is_empty());
    }

    #[test]
    fn make_node_red_viole_volontairement() {
        let node = make_node("n1", PaletteKind::Red, Position { x: 0.0, y: 0.0 });
        let violations = pnex_core::validate_graph(&FlowGraph { nodes: vec![node] });
        assert!(violations.iter().any(|v| v.code == "bad_red_node"));
    }

    #[test]
    fn cablage_add_et_remove() {
        let mut graph = graph_with_two();
        add_target(&mut graph, "n1", 0, "n2");
        add_target(&mut graph, "n1", 0, "n2"); // idempotent
        assert_eq!(graph.nodes[0].outputs[0].targets, vec!["n2".to_string()]);
        remove_target(&mut graph, "n1", 0, "n2");
        assert!(graph.nodes[0].outputs.is_empty());
        // Câblage d'une source inconnue : no-op défensif.
        add_target(&mut graph, "ghost", 0, "n2");
        assert!(graph.nodes[1].outputs.is_empty());
    }

    #[test]
    fn suppression_node_nettoie_les_entrees() {
        let mut graph = graph_with_two();
        add_target(&mut graph, "n1", 0, "n2");
        remove_node(&mut graph, "n2");
        assert_eq!(graph.nodes.len(), 1);
        assert!(graph.nodes[0].outputs.is_empty());
    }
}

