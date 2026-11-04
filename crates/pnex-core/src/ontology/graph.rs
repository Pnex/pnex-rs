//! Graph algorithms of the ontology (annex A3): pure Rust over a subgraph
//! loaded by the backend, testable without a database.
//! The store side is `services::ontology::graph` in the backend.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use petgraph::algo::{is_cyclic_directed, kosaraju_scc};
use petgraph::graph::{DiGraph, NodeIndex};

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
        self.nearest_path(&[from.to_string()], to)
    }

    /// Shortest path to `to` from the nearest of `starts`, links followed in
    /// both directions; the ids along the path, ends included.
    pub fn nearest_path(&self, starts: &[String], to: &str) -> Option<Vec<String>> {
        let target = *self.index.get(to)?;
        let starts: HashSet<NodeIndex> = starts
            .iter()
            .filter_map(|s| self.index.get(s).copied())
            .collect();
        // Walked from the target, so following the parents gives the path in order.
        let mut parent = HashMap::from([(target, target)]);
        let mut queue = VecDeque::from([target]);
        while let Some(n) = queue.pop_front() {
            if starts.contains(&n) {
                let (mut path, mut at) = (vec![self.graph[n].clone()], n);
                while at != target {
                    at = parent[&at];
                    path.push(self.graph[at].clone());
                }
                return Some(path);
            }
            for next in self.graph.neighbors_undirected(n) {
                parent.entry(next).or_insert_with(|| {
                    queue.push_back(next);
                    n
                });
            }
        }
        None
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
    fn paths() {
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
        assert!(!g.has_cycle());
    }

    #[test]
    fn nearest_path_starts_from_the_closest_start() {
        let g = Subgraph::new(&[e("far", "a"), e("a", "b"), e("b", "end"), e("near", "end")]);
        let starts = ["far", "near", "unknown"].map(String::from);
        assert_eq!(g.nearest_path(&starts, "end").unwrap(), ["near", "end"]);
        assert!(g.nearest_path(&["unknown".to_string()], "end").is_none());
    }

    #[test]
    fn cycles_are_found() {
        let g = Subgraph::new(&[e("a", "b"), e("b", "c"), e("c", "a"), e("c", "d")]);
        assert!(g.has_cycle());
        assert_eq!(g.cycles(), vec![["a", "b", "c"].map(String::from).into()]);
    }
}
