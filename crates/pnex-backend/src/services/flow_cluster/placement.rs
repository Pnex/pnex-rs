//! Placement planner (D106) — **pure**: from the workers, the current
//! placements and the weight of each org (its deployed flows), compute the
//! changes the controller must apply. No I/O, fully unit-tested.
//!
//! Rules, in order:
//! 1. **Release**: an org without deployed flows frees its placement.
//! 2. **Failover**: an org placed on a dead or draining worker (or not
//!    placed at all) goes to the least loaded eligible worker (capacity
//!    permitting; when every worker is full, the least loaded one relative
//!    to its capacity takes it — flows never stay unplaced).
//! 3. **Rebalance** (bounded by `max_moves` per round): while the spread
//!    between the most and the least loaded worker exceeds the tolerance,
//!    move the largest org that strictly reduces the spread.

use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone)]
pub struct WorkerView {
    pub id: String,
    /// Max deployed flows (0 = unbounded).
    pub capacity: u32,
    /// Alive (fresh heartbeat) and not draining.
    pub eligible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Assign {
        org_id: i64,
        worker_id: String,
    },
    Move {
        org_id: i64,
        from: String,
        to: String,
    },
    Release {
        org_id: i64,
        worker_id: String,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct PlanLimits {
    /// Max rebalance moves per round (failover is never bounded).
    pub max_moves: usize,
    /// Spread (in flows) tolerated before rebalancing, as a floor…
    pub min_spread: u64,
    /// …or as a fraction of the mean load, whichever is larger.
    pub spread_ratio: f64,
}

impl Default for PlanLimits {
    fn default() -> Self {
        Self {
            max_moves: 20,
            min_spread: 4,
            spread_ratio: 0.2,
        }
    }
}

/// Plans one controller round. `placements`: org → worker; `weights`: org
/// → deployed flows (orgs absent from `weights` have none). When `weights`
/// is `None` (not refreshed this round) releases and rebalancing are
/// skipped: failover alone never needs the weights to be fresh.
pub fn plan(
    workers: &[WorkerView],
    placements: &HashMap<i64, String>,
    weights: Option<&HashMap<i64, u64>>,
    limits: PlanLimits,
) -> Vec<Change> {
    let mut changes = Vec::new();
    let mut eligible: Vec<&WorkerView> = workers.iter().filter(|w| w.eligible).collect();
    // Sorted by id: index order doubles as the deterministic tie-break.
    eligible.sort_by(|a, b| a.id.cmp(&b.id));
    let weight_of = |org: i64| -> u64 {
        weights
            .and_then(|w| w.get(&org).copied())
            // Unknown weight (not refreshed): count the org as one flow.
            .unwrap_or(1)
            .max(1)
    };

    // Current load of each eligible worker (BTreeMap: deterministic order).
    let mut load: BTreeMap<&str, u64> = eligible.iter().map(|w| (w.id.as_str(), 0)).collect();
    // Orgs per eligible worker, for rebalancing.
    let mut owned: BTreeMap<&str, Vec<i64>> = BTreeMap::new();
    let mut orphans: Vec<i64> = Vec::new();

    // Deterministic iteration over the placements.
    let mut placed: Vec<(&i64, &String)> = placements.iter().collect();
    placed.sort();
    for (&org, worker) in placed {
        if let Some(w) = weights {
            if w.get(&org).copied().unwrap_or(0) == 0 {
                changes.push(Change::Release {
                    org_id: org,
                    worker_id: worker.clone(),
                });
                continue;
            }
        }
        match load.get_mut(worker.as_str()) {
            Some(l) => {
                *l += weight_of(org);
                owned.entry(worker.as_str()).or_default().push(org);
            }
            None => orphans.push(org),
        }
    }
    // Orgs with flows but no placement at all.
    if let Some(w) = weights {
        let mut missing: Vec<i64> = w
            .iter()
            .filter(|(org, n)| **n > 0 && !placements.contains_key(org))
            .map(|(org, _)| *org)
            .collect();
        missing.sort();
        orphans.extend(missing);
    }
    if eligible.is_empty() {
        return changes;
    }

    // Failover: heaviest orphans first (better packing).
    orphans.sort_by_key(|org| std::cmp::Reverse(weight_of(*org)));
    // Index-aligned with `eligible`: avoids string-keyed lookups in the
    // orphans × workers inner loop (10k orgs on 1k workers).
    let mut load_vec: Vec<u64> = eligible.iter().map(|w| load[w.id.as_str()]).collect();
    for org in orphans {
        let w = weight_of(org);
        let idx = pick_target(&eligible, &load_vec, w);
        load_vec[idx] += w;
        let target = eligible[idx].id.as_str();
        *load.get_mut(target).expect("eligible worker") += w;
        owned.entry(target).or_default().push(org);
        match placements.get(&org) {
            Some(from) => changes.push(Change::Move {
                org_id: org,
                from: from.clone(),
                to: target.to_string(),
            }),
            None => changes.push(Change::Assign {
                org_id: org,
                worker_id: target.to_string(),
            }),
        }
    }

    // Rebalance (only with fresh weights: moving on guesses would churn).
    if weights.is_none() || eligible.len() < 2 {
        return changes;
    }
    let total: u64 = load.values().sum();
    let mean = total as f64 / eligible.len() as f64;
    let tolerance = (limits.min_spread as f64).max(mean * limits.spread_ratio);
    let caps: HashMap<&str, u32> = eligible
        .iter()
        .map(|w| (w.id.as_str(), w.capacity))
        .collect();
    for _ in 0..limits.max_moves {
        let (max_w, max_l) = load
            .iter()
            .max_by_key(|(id, l)| (**l, std::cmp::Reverse(*id)))
            .map(|(id, l)| (*id, *l))
            .expect("eligible workers");
        let (min_w, min_l) = load
            .iter()
            .min_by_key(|(id, l)| (**l, *id))
            .map(|(id, l)| (*id, *l))
            .expect("eligible workers");
        let spread = max_l - min_l;
        if (spread as f64) <= tolerance || max_w == min_w {
            break;
        }
        // Largest org whose move strictly reduces the spread (w < spread)
        // and fits in the target capacity.
        let cap = caps.get(min_w).copied().unwrap_or(0) as u64;
        let candidates = owned.get(max_w).cloned().unwrap_or_default();
        let best = candidates
            .iter()
            .copied()
            .filter(|org| {
                let w = weight_of(*org);
                w < spread && (cap == 0 || min_l + w <= cap)
            })
            .max_by_key(|org| (weight_of(*org), std::cmp::Reverse(*org)));
        let Some(org) = best else {
            break;
        };
        let w = weight_of(org);
        *load.get_mut(max_w).expect("max") -= w;
        *load.get_mut(min_w).expect("min") += w;
        owned.get_mut(max_w).expect("max").retain(|o| *o != org);
        owned.entry(min_w).or_default().push(org);
        // Collapse with an earlier change of the same org (failover then
        // rebalance in the same round): keep one change from the origin.
        let origin = changes.iter().position(|c| {
            matches!(c, Change::Assign { org_id, .. } | Change::Move { org_id, .. } if *org_id == org)
        });
        match origin.map(|i| changes.remove(i)) {
            Some(Change::Assign { .. }) => changes.push(Change::Assign {
                org_id: org,
                worker_id: min_w.to_string(),
            }),
            Some(Change::Move { from, .. }) => changes.push(Change::Move {
                org_id: org,
                from,
                to: min_w.to_string(),
            }),
            _ => changes.push(Change::Move {
                org_id: org,
                from: max_w.to_string(),
                to: min_w.to_string(),
            }),
        }
    }
    changes
}

/// Least loaded eligible worker that can take `w` more flows; when all are
/// full, the least loaded relative to its capacity. `eligible` is sorted by
/// id and `loads` is index-aligned with it; returns the chosen index.
fn pick_target(eligible: &[&WorkerView], loads: &[u64], w: u64) -> usize {
    let mut fitting: Option<usize> = None;
    for (i, wv) in eligible.iter().enumerate() {
        let fits = wv.capacity == 0 || loads[i] + w <= wv.capacity as u64;
        // Strict `<` keeps the first (lowest id) on ties.
        if fits && fitting.is_none_or(|b| loads[i] < loads[b]) {
            fitting = Some(i);
        }
    }
    if let Some(best) = fitting {
        return best;
    }
    let ratio = |i: usize| loads[i] as f64 / eligible[i].capacity.max(1) as f64;
    (0..eligible.len())
        .min_by(|&a, &b| {
            ratio(a)
                .partial_cmp(&ratio(b))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.cmp(&b))
        })
        .expect("eligible workers")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(id: &str) -> WorkerView {
        WorkerView {
            id: id.into(),
            capacity: 0,
            eligible: true,
        }
    }

    fn apply(placements: &mut HashMap<i64, String>, changes: &[Change]) {
        for c in changes {
            match c {
                Change::Assign { org_id, worker_id } => {
                    placements.insert(*org_id, worker_id.clone());
                }
                Change::Move { org_id, to, .. } => {
                    placements.insert(*org_id, to.clone());
                }
                Change::Release { org_id, .. } => {
                    placements.remove(org_id);
                }
            }
        }
    }

    fn loads(
        placements: &HashMap<i64, String>,
        weights: &HashMap<i64, u64>,
    ) -> BTreeMap<String, u64> {
        let mut out = BTreeMap::new();
        for (org, wid) in placements {
            *out.entry(wid.clone()).or_default() += weights[org];
        }
        out
    }

    #[test]
    fn assigns_unplaced_orgs_to_least_loaded() {
        let workers = vec![w("a"), w("b")];
        let weights: HashMap<i64, u64> = [(1, 5), (2, 3), (3, 2)].into();
        let changes = plan(
            &workers,
            &HashMap::new(),
            Some(&weights),
            PlanLimits::default(),
        );
        let mut p = HashMap::new();
        apply(&mut p, &changes);
        assert_eq!(p.len(), 3);
        let l = loads(&p, &weights);
        assert_eq!(l["a"] + l["b"], 10);
        assert!(l["a"].abs_diff(l["b"]) <= 2, "{l:?}");
    }

    #[test]
    fn fails_over_orgs_of_a_dead_worker_without_fresh_weights() {
        let mut workers = vec![w("a"), w("b"), w("c")];
        workers[2].eligible = false; // c died
        let placements: HashMap<i64, String> =
            [(1, "a".into()), (2, "c".into()), (3, "c".into())].into();
        let changes = plan(&workers, &placements, None, PlanLimits::default());
        assert_eq!(changes.len(), 2, "{changes:?}");
        for c in &changes {
            match c {
                Change::Move { from, to, .. } => {
                    assert_eq!(from, "c");
                    assert_ne!(to, "c");
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        // Spread over the survivors: b (empty) takes the first one.
        let mut p = placements.clone();
        apply(&mut p, &changes);
        assert!(p.values().all(|v| v != "c"));
    }

    #[test]
    fn releases_orgs_without_deployed_flows() {
        let workers = vec![w("a")];
        let placements: HashMap<i64, String> = [(1, "a".into()), (2, "a".into())].into();
        let weights: HashMap<i64, u64> = [(1, 3)].into();
        let changes = plan(&workers, &placements, Some(&weights), PlanLimits::default());
        assert_eq!(
            changes,
            vec![Change::Release {
                org_id: 2,
                worker_id: "a".into()
            }]
        );
    }

    #[test]
    fn rebalances_toward_a_new_worker_with_bounded_moves() {
        let workers = vec![w("a"), w("b")];
        // 40 orgs of 1 flow on a, b just joined.
        let placements: HashMap<i64, String> = (1..=40).map(|o| (o, "a".to_string())).collect();
        let weights: HashMap<i64, u64> = (1..=40).map(|o| (o, 1)).collect();
        let limits = PlanLimits {
            max_moves: 5,
            ..Default::default()
        };
        let changes = plan(&workers, &placements, Some(&weights), limits);
        assert_eq!(changes.len(), 5, "bounded per round");
        // Converges over rounds.
        let mut p = placements;
        for _ in 0..10 {
            let c = plan(&workers, &p, Some(&weights), limits);
            apply(&mut p, &c);
        }
        let l = loads(&p, &weights);
        assert!(l["a"].abs_diff(l["b"]) <= 4, "{l:?}");
        // Stable once balanced: no churn.
        assert!(plan(&workers, &p, Some(&weights), limits).is_empty());
    }

    #[test]
    fn never_moves_an_org_that_would_not_reduce_the_spread() {
        let workers = vec![w("a"), w("b")];
        // One huge org on a: moving it would just flip the imbalance.
        let placements: HashMap<i64, String> = [(1, "a".into())].into();
        let weights: HashMap<i64, u64> = [(1, 100)].into();
        assert!(plan(&workers, &placements, Some(&weights), PlanLimits::default()).is_empty());
    }

    #[test]
    fn respects_capacity_then_overflows_to_the_least_relatively_loaded() {
        let mut a = w("a");
        a.capacity = 5;
        let mut b = w("b");
        b.capacity = 10;
        let workers = vec![a, b];
        let weights: HashMap<i64, u64> = [(1, 4), (2, 4), (3, 4), (4, 4)].into();
        let changes = plan(
            &workers,
            &HashMap::new(),
            Some(&weights),
            PlanLimits::default(),
        );
        let mut p = HashMap::new();
        apply(&mut p, &changes);
        assert_eq!(p.len(), 4, "nothing left unplaced");
        let l = loads(&p, &weights);
        assert!(l["a"] <= 8 && l["b"] >= 8, "{l:?}");
    }

    #[test]
    fn no_eligible_worker_keeps_placements_untouched() {
        let mut workers = vec![w("a")];
        workers[0].eligible = false;
        let placements: HashMap<i64, String> = [(1, "a".into())].into();
        let weights: HashMap<i64, u64> = [(1, 1)].into();
        assert!(plan(&workers, &placements, Some(&weights), PlanLimits::default()).is_empty());
    }

    #[test]
    fn draining_worker_is_emptied() {
        let mut workers = vec![w("a"), w("b")];
        workers[0].eligible = false; // draining
        let placements: HashMap<i64, String> = (1..=10).map(|o| (o, "a".to_string())).collect();
        let weights: HashMap<i64, u64> = (1..=10).map(|o| (o, 1)).collect();
        let changes = plan(&workers, &placements, Some(&weights), PlanLimits::default());
        assert_eq!(changes.len(), 10, "failover is never bounded");
    }

    #[test]
    fn scales_to_ten_thousand_orgs_on_a_thousand_workers() {
        let workers: Vec<WorkerView> = (0..1000).map(|i| w(&format!("w{i:04}"))).collect();
        let weights: HashMap<i64, u64> = (0..10_000).map(|o| (o, 1 + (o as u64 % 400))).collect();
        let t = std::time::Instant::now();
        let changes = plan(
            &workers,
            &HashMap::new(),
            Some(&weights),
            PlanLimits::default(),
        );
        let mut p = HashMap::new();
        apply(&mut p, &changes);
        assert_eq!(p.len(), 10_000);
        let l = loads(&p, &weights);
        let (mn, mx) = (l.values().min().unwrap(), l.values().max().unwrap());
        // Greedy heaviest-first packing: within one max org weight.
        assert!(mx - mn <= 400, "min {mn} max {mx}");
        assert!(t.elapsed().as_secs() < 5, "planning took {:?}", t.elapsed());
    }
}
