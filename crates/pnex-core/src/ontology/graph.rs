//! Graph algorithms of the ontology (annex A3): pure Rust over a subgraph
//! loaded by the backend (recursive CTEs), testable without a database.
//! The store side is `services::ontology::graph` in the backend.

use std::collections::{BTreeSet, HashMap};

use petgraph::algo::{astar, is_cyclic_directed, kosaraju_scc};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::{Bfs, EdgeRef, Reversed};

/// A directed edge `source → target` labelled with its link type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub source: String,
    pub target: String,
    pub link_type: String,
}

/// An in-memory subgraph, nodes keyed by object id.
#[derive(Debug, Default)]
pub struct Subgraph {
    graph: DiGraph<String, String>,
    index: HashMap<String, NodeIndex>,
}

impl Subgraph {
    pub fn new(edges: &[Edge]) -> Self {
        let mut g = Self::default();
        for e in edges {
            let a = g.node(&e.source);
            let b = g.node(&e.target);
            g.graph.add_edge(a, b, e.link_type.clone());
        }
        g
    }

    fn node(&mut self, id: &str) -> NodeIndex {
        if let Some(i) = self.index.get(id) {
            return *i;
        }
        let i = self.graph.add_node(id.to_string());
        self.index.insert(id.to_string(), i);
        i
    }

    /// Shortest path (fewest links) from `from` to `to`, links followed in
    /// both directions; the ids along the path, ends included.
    pub fn shortest_path(&self, from: &str, to: &str) -> Option<Vec<String>> {
        let (&a, &b) = (self.index.get(from)?, self.index.get(to)?);
        // Undirected view: add the reverse of every edge.
        let mut und = self.graph.clone();
        let reverse: Vec<_> = self
            .graph
            .edge_references()
            .map(|e| (e.target(), e.source(), e.weight().clone()))
            .collect();
        for (s, t, w) in reverse {
            und.add_edge(s, t, w);
        }
        let (_, path) = astar(&und, a, |n| n == b, |_| 1u32, |_| 0)?;
        Some(path.into_iter().map(|n| self.graph[n].clone()).collect())
    }

    /// Everything reachable downstream of `from` (cascading impact), `from`
    /// excluded; `upstream` follows links backwards (what feeds `from`).
    pub fn impact(&self, from: &str, upstream: bool) -> BTreeSet<String> {
        let Some(&start) = self.index.get(from) else {
            return BTreeSet::new();
        };
        let mut out = BTreeSet::new();
        if upstream {
            let rev = Reversed(&self.graph);
            let mut bfs = Bfs::new(rev, start);
            while let Some(n) = bfs.next(rev) {
                out.insert(self.graph[n].clone());
            }
        } else {
            let mut bfs = Bfs::new(&self.graph, start);
            while let Some(n) = bfs.next(&self.graph) {
                out.insert(self.graph[n].clone());
            }
        }
        out.remove(from);
        out
    }

    pub fn has_cycle(&self) -> bool {
        is_cyclic_directed(&self.graph)
    }

    /// Strongly connected components with more than one node (the cycles).
    pub fn cycles(&self) -> Vec<BTreeSet<String>> {
        kosaraju_scc(&self.graph)
            .into_iter()
            .filter(|c| c.len() > 1)
            .map(|c| c.into_iter().map(|n| self.graph[n].clone()).collect())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(s: &str, t: &str) -> Edge {
        Edge {
            source: s.into(),
            target: t.into(),
            link_type: "feeds".into(),
        }
    }

    #[test]
    fn paths_and_impact() {
        // pump → valve → tank ; sensor → pump ; tank → line
        let g = Subgraph::new(&[
            e("pump", "valve"),
            e("valve", "tank"),
            e("sensor", "pump"),
            e("tank", "line"),
        ]);
        assert_eq!(
            g.shortest_path("sensor", "line").unwrap(),
            ["sensor", "pump", "valve", "tank", "line"]
        );
        // Undirected: line back to sensor.
        assert_eq!(g.shortest_path("line", "sensor").unwrap().len(), 5);
        assert!(g.shortest_path("pump", "nowhere").is_none());
        assert_eq!(
            g.impact("pump", false),
            ["line", "tank", "valve"].map(String::from).into()
        );
        assert_eq!(
            g.impact("tank", true),
            ["pump", "sensor", "valve"].map(String::from).into()
        );
        assert!(!g.has_cycle());
    }

    #[test]
    fn cycles_are_found() {
        let g = Subgraph::new(&[e("a", "b"), e("b", "c"), e("c", "a"), e("c", "d")]);
        assert!(g.has_cycle());
        assert_eq!(g.cycles(), vec![["a", "b", "c"].map(String::from).into()]);
    }
}
