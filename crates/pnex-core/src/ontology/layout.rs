//! Deterministic force layout of a case graph (cases-workspace.md K31):
//! Fruchterman–Reingold on a fixed number of iterations, seeded by a string
//! (the case id), so the same graph always draws the same. Nodes with a
//! position already (placed earlier, or moved by hand) stay put: adding a
//! node only places the newcomers, next to the neighbours they link to.
//! Nodes stay inside a frame shaped like the panel (classic FR): a loose
//! node no longer flies off and squeezes the rest when the drawing is fitted,
//! and the spacing follows `√(area / n)`, airy when few, tight when many.
//! Pure and wasm-safe; O(n²) per iteration, bounded by the caller's cap.

/// A node: its stable key, and its position when it must not move.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutNode {
    pub key: String,
    pub fixed: Option<(f64, f64)>,
}

/// Side of the square of the same area as the layout frame.
pub const SIDE: f64 = 1000.0;
const ITERATIONS: usize = 300;

/// FNV-1a 64: a stable hash of `seed` and `key`, two coordinates in [0, 1).
fn hashed(seed: &str, key: &str) -> (f64, f64) {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in seed.bytes().chain([0]).chain(key.bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    let a = (h >> 32) as f64 / u32::MAX as f64;
    let b = (h & 0xffff_ffff) as f64 / u32::MAX as f64;
    (a, b)
}

/// Positions of `nodes` (same order) linked by `edges` (index pairs), in a
/// frame of the panel's `aspect` (width / height).
pub fn force_layout(
    nodes: &[LayoutNode],
    edges: &[(usize, usize)],
    seed: &str,
    aspect: f64,
) -> Vec<(f64, f64)> {
    let n = nodes.len();
    if n == 0 {
        return Vec::new();
    }
    let a = aspect.clamp(0.4, 3.0).sqrt();
    // The frame: the panel's shape, widened to cover nodes already placed
    // (a saved arrangement may reach beyond it).
    let (mut x0, mut y0, mut x1, mut y1) = (0.0, 0.0, SIDE * a, SIDE / a);
    for (x, y) in nodes.iter().filter_map(|n| n.fixed) {
        (x0, x1) = (f64::min(x0, x), f64::max(x1, x));
        (y0, y1) = (f64::min(y0, y), f64::max(y1, y));
    }
    let (fw, fh) = (x1 - x0, y1 - y0);
    let k = (fw * fh / n as f64).sqrt() * 0.75;
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(a, b) in edges.iter().filter(|(a, b)| a != b && *a < n && *b < n) {
        adj[a].push(b);
        adj[b].push(a);
    }
    // Start: fixed nodes where they are; a newcomer next to its placed
    // neighbours, else at a hashed spot.
    let mut pos: Vec<(f64, f64)> = vec![(0.0, 0.0); n];
    for (i, node) in nodes.iter().enumerate() {
        let (hx, hy) = hashed(seed, &node.key);
        pos[i] = node.fixed.unwrap_or_else(|| {
            let placed: Vec<(f64, f64)> = adj[i].iter().filter_map(|j| nodes[*j].fixed).collect();
            if placed.is_empty() {
                (x0 + hx * fw, y0 + hy * fh)
            } else {
                let (sx, sy) = placed
                    .iter()
                    .fold((0.0, 0.0), |(a, b), p| (a + p.0, b + p.1));
                let m = placed.len() as f64;
                (sx / m + (hx - 0.5) * k, sy / m + (hy - 0.5) * k)
            }
        });
    }
    if nodes.iter().all(|n| n.fixed.is_some()) {
        return pos;
    }
    let center = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    for it in 0..ITERATIONS {
        let temp = SIDE / 10.0 * (1.0 - it as f64 / ITERATIONS as f64) + 0.5;
        let mut disp = vec![(0.0f64, 0.0f64); n];
        for i in 0..n {
            for j in i + 1..n {
                let (mut dx, mut dy) = (pos[i].0 - pos[j].0, pos[i].1 - pos[j].1);
                let mut d2 = dx * dx + dy * dy;
                if d2 < 1e-6 {
                    // Coincident: a deterministic nudge apart.
                    let (hx, hy) = hashed(&nodes[i].key, &nodes[j].key);
                    (dx, dy) = (hx - 0.5 + 1e-3, hy - 0.5);
                    d2 = dx * dx + dy * dy;
                }
                let f = k * k / d2;
                disp[i].0 += dx * f;
                disp[i].1 += dy * f;
                disp[j].0 -= dx * f;
                disp[j].1 -= dy * f;
            }
        }
        for (i, list) in adj.iter().enumerate() {
            for &j in list.iter().filter(|j| **j > i) {
                let (dx, dy) = (pos[i].0 - pos[j].0, pos[i].1 - pos[j].1);
                let d = (dx * dx + dy * dy).sqrt().max(1e-3);
                let f = d / k;
                disp[i].0 -= dx * f;
                disp[i].1 -= dy * f;
                disp[j].0 += dx * f;
                disp[j].1 += dy * f;
            }
        }
        for i in 0..n {
            if nodes[i].fixed.is_some() {
                continue;
            }
            // A pull to the centre keeps loose parts off the frame's edges.
            disp[i].0 += (center.0 - pos[i].0) * 0.06;
            disp[i].1 += (center.1 - pos[i].1) * 0.06;
            let len = (disp[i].0 * disp[i].0 + disp[i].1 * disp[i].1).sqrt();
            if len > 0.0 {
                let step = len.min(temp);
                pos[i].0 = (pos[i].0 + disp[i].0 / len * step).clamp(x0, x1);
                pos[i].1 = (pos[i].1 + disp[i].1 / len * step).clamp(y0, y1);
            }
        }
    }
    // Last, nodes still too close part. Labels run sideways: two nodes need
    // twice the room side by side as one above the other.
    let min_d = k * 0.5;
    for _ in 0..60 {
        let mut moved = false;
        for i in 0..n {
            for j in i + 1..n {
                let (dx, dy) = (pos[j].0 - pos[i].0, pos[j].1 - pos[i].1);
                let d = ((dx / 2.0).powi(2) + dy * dy).sqrt();
                if d >= min_d {
                    continue;
                }
                moved = true;
                let (ux, uy) = if d > 1e-6 {
                    let l = (dx * dx + dy * dy).sqrt();
                    (dx / l, dy / l)
                } else {
                    let (hx, hy) = hashed(&nodes[i].key, &nodes[j].key);
                    let l = ((hx - 0.5).powi(2) + (hy - 0.5).powi(2)).sqrt().max(1e-6);
                    ((hx - 0.5) / l, (hy - 0.5) / l)
                };
                let (fi, fj) = (nodes[i].fixed.is_none(), nodes[j].fixed.is_none());
                let share = (min_d - d) / if fi && fj { 2.0 } else { 1.0 };
                if fi {
                    pos[i].0 = (pos[i].0 - ux * share).clamp(x0, x1);
                    pos[i].1 = (pos[i].1 - uy * share).clamp(y0, y1);
                }
                if fj {
                    pos[j].0 = (pos[j].0 + ux * share).clamp(x0, x1);
                    pos[j].1 = (pos[j].1 + uy * share).clamp(y0, y1);
                }
            }
        }
        if !moved {
            break;
        }
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;

    fn free(keys: &[&str]) -> Vec<LayoutNode> {
        keys.iter()
            .map(|k| LayoutNode {
                key: k.to_string(),
                fixed: None,
            })
            .collect()
    }

    fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
        ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
    }

    #[test]
    fn same_graph_same_drawing() {
        let nodes = free(&["e1", "a", "b", "e2", "c"]);
        let edges = [(0, 1), (0, 2), (3, 2), (3, 4)];
        let p1 = force_layout(&nodes, &edges, "case-1", 1.0);
        assert_eq!(p1, force_layout(&nodes, &edges, "case-1", 1.0));
        assert_ne!(p1, force_layout(&nodes, &edges, "case-2", 1.0));
        assert!(p1.iter().all(|(x, y)| x.is_finite() && y.is_finite()));
    }

    /// Same vectors as `crates/pnex-frontend/js/graph_layout.test.js`: the
    /// Sigma graph seeds its layout with the JS port of `hashed`.
    #[test]
    fn hashed_matches_the_js_port() {
        let close = |got: (f64, f64), want: (f64, f64)| {
            assert!(
                (got.0 - want.0).abs() < 1e-12 && (got.1 - want.1).abs() < 1e-12,
                "{got:?}"
            );
        };
        close(
            hashed("case-1", "obj-1"),
            (0.4513241717245719, 0.6161696809847302),
        );
        close(
            hashed("0192f0c4-case", "été"),
            (0.5870898609485221, 0.002698675497132045),
        );
        close(hashed("", ""), (0.6851156532496949, 0.5234637184821683));
    }

    #[test]
    fn linked_nodes_sit_closer_than_unlinked_ones() {
        let nodes = free(&["e1", "a", "b", "e2", "c", "d"]);
        let edges = [(0, 1), (0, 2), (3, 4), (3, 5)];
        let p = force_layout(&nodes, &edges, "s", 1.0);
        let linked = (dist(p[0], p[1]) + dist(p[0], p[2]) + dist(p[3], p[4])) / 3.0;
        let apart = (dist(p[1], p[4]) + dist(p[2], p[5])) / 2.0;
        assert!(linked < apart, "{linked} vs {apart}");
    }

    #[test]
    fn placed_nodes_stay_and_newcomers_join_their_neighbours() {
        let mut nodes = free(&["e1", "a", "b"]);
        let p = force_layout(&nodes, &[(0, 1), (0, 2)], "s", 1.0);
        for (n, xy) in nodes.iter_mut().zip(&p) {
            n.fixed = Some(*xy);
        }
        nodes.push(LayoutNode {
            key: "c".into(),
            fixed: None,
        });
        nodes.push(LayoutNode {
            key: "far".into(),
            fixed: None,
        });
        let q = force_layout(&nodes, &[(0, 1), (0, 2), (0, 3)], "s", 1.0);
        assert_eq!(&q[..3], &p[..]);
        assert!(dist(q[3], q[0]) < dist(q[4], q[0]), "{q:?}");
    }

    #[test]
    fn a_chain_folds_instead_of_stretching() {
        let keys: Vec<String> = (0..9).map(|i| format!("n{i}")).collect();
        let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        let edges: Vec<(usize, usize)> = (0..8).map(|i| (i, i + 1)).collect();
        let p = force_layout(&free(&refs), &edges, "chain", 1.0);
        let (xs, ys): (Vec<f64>, Vec<f64>) = p.iter().copied().unzip();
        let span = |v: &[f64]| {
            v.iter().cloned().fold(f64::MIN, f64::max) - v.iter().cloned().fold(f64::MAX, f64::min)
        };
        let (w, h) = (span(&xs), span(&ys));
        assert!(w.max(h) / w.min(h) < 2.5, "{w} x {h}");
    }

    /// A case-like graph: events, a hub on most of them, isolated pieces.
    fn case_like() -> (Vec<LayoutNode>, Vec<(usize, usize)>) {
        let mut keys: Vec<String> = (0..10).map(|i| format!("event-{i}")).collect();
        keys.extend((0..25).map(|i| format!("object-{i}")));
        let mut edges = Vec::new();
        for e in 0..10 {
            edges.push((e, 10)); // the hub
            edges.push((e, 11 + e % 6));
            edges.push((e, 17 + e));
        }
        let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        (free(&refs), edges)
    }

    #[test]
    fn loose_nodes_stay_in_the_frame_and_nodes_keep_apart() {
        let (nodes, edges) = case_like();
        for aspect in [1.0, 2.0] {
            let p = force_layout(&nodes, &edges, "case", aspect);
            let a = aspect.sqrt();
            let (fw, fh) = (SIDE * a, SIDE / a);
            assert!(p
                .iter()
                .all(|(x, y)| (0.0..=fw).contains(x) && (0.0..=fh).contains(y)));
            // Spread over the frame (no clump in the middle)...
            let (xs, ys): (Vec<f64>, Vec<f64>) = p.iter().copied().unzip();
            let span = |v: &[f64]| {
                v.iter().cloned().fold(f64::MIN, f64::max)
                    - v.iter().cloned().fold(f64::MAX, f64::min)
            };
            assert!(span(&xs) > fw * 0.6 && span(&ys) > fh * 0.6, "{aspect}");
            // ...and no two nodes on top of each other.
            let k = (SIDE * SIDE / p.len() as f64).sqrt();
            for i in 0..p.len() {
                for j in i + 1..p.len() {
                    assert!(dist(p[i], p[j]) > k * 0.25, "{aspect}: {i} {j}");
                }
            }
        }
    }

    #[test]
    fn a_hub_on_every_event_does_not_clump() {
        // 34 events all on one hub, each with one of 20 other objects.
        let mut keys: Vec<String> = (0..34).map(|i| format!("e{i}")).collect();
        keys.push("hub".into());
        keys.extend((0..20).map(|i| format!("o{i}")));
        let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        let edges: Vec<(usize, usize)> =
            (0..34).flat_map(|e| [(e, 34), (e, 35 + e % 20)]).collect();
        let p = force_layout(&free(&refs), &edges, "star", 1.8);
        let k = (SIDE * SIDE / p.len() as f64).sqrt();
        let mut worst = f64::MAX;
        for i in 0..p.len() {
            for j in i + 1..p.len() {
                worst = worst.min(dist(p[i], p[j]) / k);
            }
        }
        assert!(worst > 0.25, "{worst}");
    }

    #[test]
    fn newcomers_spread_over_a_wide_saved_arrangement() {
        // Two nodes saved far apart (an old, wider layout), ten newcomers.
        let mut nodes = vec![
            LayoutNode {
                key: "a".into(),
                fixed: Some((-3000.0, -2000.0)),
            },
            LayoutNode {
                key: "b".into(),
                fixed: Some((4000.0, 3000.0)),
            },
        ];
        nodes.extend((0..10).map(|i| LayoutNode {
            key: format!("n{i}"),
            fixed: None,
        }));
        let edges: Vec<(usize, usize)> = (2..12).map(|i| (i, i % 2)).collect();
        let p = force_layout(&nodes, &edges, "wide", 1.0);
        let (xs, _): (Vec<f64>, Vec<f64>) = p[2..].iter().copied().unzip();
        let span = xs.iter().cloned().fold(f64::MIN, f64::max)
            - xs.iter().cloned().fold(f64::MAX, f64::min);
        assert!(span > 2000.0, "newcomers squeezed: {span}");
    }

    #[test]
    fn degenerate_inputs() {
        assert!(force_layout(&[], &[], "s", 1.0).is_empty());
        let one = force_layout(&free(&["x"]), &[(0, 0), (0, 9)], "s", 1.0);
        assert_eq!(one.len(), 1);
        assert!(one[0].0.is_finite());
    }
}
