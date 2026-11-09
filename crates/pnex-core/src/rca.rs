//! Causal chain detector (cases-workspace.md appendix I, D904–D908;
//! markets-research.md H.7). Pure and deterministic: typed signals, the
//! links that propagate a failure, declared cause → effect pairs. A chain
//! is a lead, never a fact, and is measured against a placebo.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

/// Closed list of signal kinds (D904, H.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum SignalKind {
    #[serde(rename = "change.config")]
    ChangeConfig,
    #[serde(rename = "change.firmware")]
    ChangeFirmware,
    #[serde(rename = "change.link")]
    ChangeLink,
    #[serde(rename = "range.maintenance")]
    RangeMaintenance,
    #[serde(rename = "alert")]
    Alert,
    #[serde(rename = "device.offline")]
    DeviceOffline,
    #[serde(rename = "device.online")]
    DeviceOnline,
    #[serde(rename = "series.threshold")]
    SeriesThreshold,
    #[serde(rename = "geo_event")]
    GeoEvent,
    #[serde(rename = "macro.release")]
    MacroRelease,
    #[serde(rename = "supply.disruption")]
    SupplyDisruption,
    #[serde(rename = "price.move")]
    PriceMove,
}

impl SignalKind {
    pub const ALL: [SignalKind; 12] = [
        SignalKind::ChangeConfig,
        SignalKind::ChangeFirmware,
        SignalKind::ChangeLink,
        SignalKind::RangeMaintenance,
        SignalKind::Alert,
        SignalKind::DeviceOffline,
        SignalKind::DeviceOnline,
        SignalKind::SeriesThreshold,
        SignalKind::GeoEvent,
        SignalKind::MacroRelease,
        SignalKind::SupplyDisruption,
        SignalKind::PriceMove,
    ];

    /// The wire name (`change.config`).
    pub fn as_str(self) -> &'static str {
        match self {
            SignalKind::ChangeConfig => "change.config",
            SignalKind::ChangeFirmware => "change.firmware",
            SignalKind::ChangeLink => "change.link",
            SignalKind::RangeMaintenance => "range.maintenance",
            SignalKind::Alert => "alert",
            SignalKind::DeviceOffline => "device.offline",
            SignalKind::DeviceOnline => "device.online",
            SignalKind::SeriesThreshold => "series.threshold",
            SignalKind::GeoEvent => "geo_event",
            SignalKind::MacroRelease => "macro.release",
            SignalKind::SupplyDisruption => "supply.disruption",
            SignalKind::PriceMove => "price.move",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// A typed signal on an object (D904). `severity` in [0, 1] is computed by
/// the server, never by a model. `at` in Unix seconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Signal {
    /// Stable id of the signal (its source row), for deduplication.
    pub id: String,
    pub at: i64,
    /// Object id the signal is on.
    pub on: String,
    pub kind: SignalKind,
    pub severity: f64,
    /// Where the signal comes from (a change, a notification, a bar).
    pub source_ref: String,
}

/// A link that propagates a failure, source → target, valid on
/// `[from, to)` (Unix seconds; `to = None` while open). D905.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub source: String,
    pub target: String,
    pub from: i64,
    pub to: Option<i64>,
}

/// A declared cause → effect pair (D906). `max_gap` in seconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CausalPair {
    pub cause: SignalKind,
    pub effect: SignalKind,
    pub max_gap: i64,
    pub prior: f64,
}

/// A pair as a pack declares it (D906): its gap in minutes. Each pair is
/// also two pack settings (`causal_<cause>_<effect>_prior`, `…_max_gap_min`)
/// the org may set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PackPair {
    pub cause: SignalKind,
    pub effect: SignalKind,
    pub max_gap_min: u32,
    pub prior: f64,
}

impl PackPair {
    /// Key stem of the pair's settings (`causal_change_config_alert`).
    pub fn setting_stem(&self) -> String {
        format!(
            "causal_{}_{}",
            self.cause.as_str().replace('.', "_"),
            self.effect.as_str().replace('.', "_")
        )
    }

    /// The pair with the values in force (`settings` keyed by setting key).
    pub fn in_force(&self, settings: &dyn Fn(&str) -> Option<f64>) -> CausalPair {
        let stem = self.setting_stem();
        let prior = settings(&format!("{stem}_prior")).unwrap_or(self.prior);
        let gap = settings(&format!("{stem}_max_gap_min")).unwrap_or(f64::from(self.max_gap_min));
        CausalPair {
            cause: self.cause,
            effect: self.effect,
            max_gap: (gap.max(1.0) * 60.0) as i64,
            prior: prior.clamp(0.0, 1.0),
        }
    }
}

/// How a pair fared in the org's cases (D911): chains proposed with a hop
/// of this pair, how many a member retained, the mean rank of the retained
/// ones. `flagged` past [`FLAG_MIN_PROPOSED`] proposals under
/// [`FLAG_MAX_PRECISION`]: shown, never switched off.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PairStats {
    pub cause: SignalKind,
    pub effect: SignalKind,
    pub proposed: u32,
    pub retained: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_rank: Option<f64>,
    pub flagged: bool,
}

pub const FLAG_MIN_PROPOSED: u32 = 20;
pub const FLAG_MAX_PRECISION: f64 = 0.2;

/// The stats of each pair from the decided and open chain proposals:
/// (rules of its hops, retained, rank).
pub fn pair_stats(
    pairs: &[PackPair],
    proposals: &[(Vec<(SignalKind, SignalKind)>, bool, u32)],
) -> Vec<PairStats> {
    pairs
        .iter()
        .map(|p| {
            let mine: Vec<_> = proposals
                .iter()
                .filter(|(rules, _, _)| rules.contains(&(p.cause, p.effect)))
                .collect();
            let retained: Vec<_> = mine.iter().filter(|(_, r, _)| *r).collect();
            let proposed = mine.len() as u32;
            let kept = retained.len() as u32;
            let mean_rank = (!retained.is_empty()).then(|| {
                retained.iter().map(|(_, _, k)| f64::from(*k)).sum::<f64>() / retained.len() as f64
            });
            PairStats {
                cause: p.cause,
                effect: p.effect,
                proposed,
                retained: kept,
                mean_rank,
                flagged: proposed >= FLAG_MIN_PROPOSED
                    && f64::from(kept) / f64::from(proposed) < FLAG_MAX_PRECISION,
            }
        })
        .collect()
}

/// One step of a chain: `cause` precedes `effect` by `gap` seconds,
/// `distance` links apart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Hop {
    pub cause: Signal,
    pub effect: Signal,
    pub gap: i64,
    pub distance: u32,
    /// Index of the pair in the list given.
    pub pair: usize,
    pub score: f64,
}

/// A chain, root cause first (D907). `lift` = confidence ÷ placebo (D908).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Chain {
    pub hops: Vec<Hop>,
    pub confidence: f64,
    pub placebo: f64,
    pub lift: f64,
}

impl Chain {
    pub fn root(&self) -> &Signal {
        &self.hops[0].cause
    }
    pub fn effect(&self) -> &Signal {
        &self.hops[self.hops.len() - 1].effect
    }
}

/// Most links between two objects (D905).
pub const MAX_DISTANCE: u32 = 3;
/// Most hops of a chain.
pub const MAX_HOPS: usize = 3;
/// Chains kept per effect (D907).
pub const TOP_PER_EFFECT: usize = 3;
/// Placebo shifts (D908): 24 h, 48 h, 7 days.
pub const PLACEBO_SHIFTS: [i64; 3] = [86_400, 172_800, 604_800];
/// Keeps the lift finite when the placebo finds nothing.
pub const LIFT_EPSILON: f64 = 0.01;
/// Default lift threshold of a case (Q-RC3).
pub const DEFAULT_MIN_LIFT: f64 = 2.0;

/// The propagating topology: who is downstream of whom, at an instant.
pub struct Topology {
    out: BTreeMap<String, Vec<Edge>>,
}

impl Topology {
    pub fn new(edges: Vec<Edge>) -> Self {
        let mut out: BTreeMap<String, Vec<Edge>> = BTreeMap::new();
        for e in edges {
            out.entry(e.source.clone()).or_default().push(e);
        }
        for v in out.values_mut() {
            v.sort_by(|a, b| a.target.cmp(&b.target).then(a.from.cmp(&b.from)));
        }
        Self { out }
    }

    /// Links from `from` down to `to` at `at`, following open links in
    /// their direction; `Some(0)` on the same object.
    pub fn distance(&self, from: &str, to: &str, at: i64) -> Option<u32> {
        if from == to {
            return Some(0);
        }
        let mut seen = BTreeSet::from([from.to_string()]);
        let mut queue = VecDeque::from([(from.to_string(), 0u32)]);
        while let Some((node, d)) = queue.pop_front() {
            if d == MAX_DISTANCE {
                continue;
            }
            for e in self.out.get(&node).into_iter().flatten() {
                let open = e.from <= at && e.to.is_none_or(|t| t > at);
                if !open || !seen.insert(e.target.clone()) {
                    continue;
                }
                if e.target == to {
                    return Some(d + 1);
                }
                queue.push_back((e.target.clone(), d + 1));
            }
        }
        None
    }
}

/// A gap as people read it: `12 min`, `3 h 05`, `2 d 4 h`.
pub fn gap_text(secs: i64) -> String {
    let m = secs.max(0) / 60;
    match m {
        0..=59 => format!("{m} min"),
        60..=1_439 => format!("{} h {:02}", m / 60, m % 60),
        _ => format!("{} d {} h", m / 1_440, (m % 1_440) / 60),
    }
}

/// Score of one hop (D907): prior × exp(−gap / τ) × 1 / (1 + distance),
/// τ = max_gap / 3.
pub fn hop_score(pair: &CausalPair, gap: i64, distance: u32) -> f64 {
    let tau = (pair.max_gap as f64 / 3.0).max(1.0);
    pair.prior.clamp(0.0, 1.0) * (-(gap as f64) / tau).exp() / (1.0 + f64::from(distance))
}

/// Direct causes of `effect` among `signals`: every signal before it, within
/// the gap of a declared pair, upstream through links open at the cause.
fn direct(
    effect: &Signal,
    signals: &[Signal],
    topology: &Topology,
    pairs: &[CausalPair],
) -> Vec<Hop> {
    let mut hops = Vec::new();
    for (i, pair) in pairs.iter().enumerate() {
        if pair.effect != effect.kind {
            continue;
        }
        for c in signals {
            if c.kind != pair.cause || c.id == effect.id {
                continue;
            }
            let gap = effect.at - c.at;
            if gap < 0 || gap > pair.max_gap {
                continue;
            }
            let Some(distance) = topology.distance(&c.on, &effect.on, c.at) else {
                continue;
            };
            hops.push(Hop {
                cause: c.clone(),
                effect: effect.clone(),
                gap,
                distance,
                pair: i,
                score: hop_score(pair, gap, distance),
            });
        }
    }
    hops
}

/// Every chain ending at `effect`, up to [`MAX_HOPS`], best first.
fn chains_to(
    effect: &Signal,
    signals: &[Signal],
    topology: &Topology,
    pairs: &[CausalPair],
    depth: usize,
) -> Vec<(Vec<Hop>, f64)> {
    let mut out = Vec::new();
    for hop in direct(effect, signals, topology, pairs) {
        let score = hop.score;
        out.push((vec![hop.clone()], score));
        if depth > 1 {
            for (mut up, s) in chains_to(&hop.cause, signals, topology, pairs, depth - 1) {
                // A signal appears once in a chain.
                if up.iter().any(|h| h.cause.id == effect.id) {
                    continue;
                }
                up.push(hop.clone());
                out.push((up, s * score));
            }
        }
    }
    out
}

fn order(a: &(Vec<Hop>, f64), b: &(Vec<Hop>, f64)) -> std::cmp::Ordering {
    b.1.total_cmp(&a.1)
        .then(a.0[0].cause.at.cmp(&b.0[0].cause.at))
        .then(a.0[0].cause.id.cmp(&b.0[0].cause.id))
        .then(a.0.len().cmp(&b.0.len()))
}

/// Best placebo confidence (D908): the same effect, moved back by each
/// shift, explained by a chain from the same root kind on the same root
/// object; the mean over the shifts.
fn placebo(chain: &[Hop], signals: &[Signal], topology: &Topology, pairs: &[CausalPair]) -> f64 {
    let root = &chain[0].cause;
    let effect = &chain[chain.len() - 1].effect;
    let total: f64 = PLACEBO_SHIFTS
        .iter()
        .map(|shift| {
            let ghost = Signal {
                id: format!("{}@-{shift}", effect.id),
                at: effect.at - shift,
                ..effect.clone()
            };
            chains_to(&ghost, signals, topology, pairs, chain.len())
                .into_iter()
                .filter(|(hops, _)| hops[0].cause.kind == root.kind && hops[0].cause.on == root.on)
                .map(|(_, s)| s)
                .fold(0.0, f64::max)
        })
        .sum();
    total / PLACEBO_SHIFTS.len() as f64
}

/// Chains explaining the `effects` (D907, D908): the
/// [`TOP_PER_EFFECT`] best per effect, with their lift. Same inputs, same
/// output, same order.
pub fn chains(
    effects: &[Signal],
    signals: &[Signal],
    topology: &Topology,
    pairs: &[CausalPair],
) -> Vec<Chain> {
    let mut effects: Vec<&Signal> = effects.iter().collect();
    effects.sort_by(|a, b| a.at.cmp(&b.at).then(a.id.cmp(&b.id)));
    let mut out = Vec::new();
    for effect in effects {
        let mut found = chains_to(effect, signals, topology, pairs, MAX_HOPS);
        found.sort_by(order);
        for (hops, confidence) in found.into_iter().take(TOP_PER_EFFECT) {
            let placebo = placebo(&hops, signals, topology, pairs);
            out.push(Chain {
                lift: confidence / (placebo + LIFT_EPSILON),
                hops,
                confidence,
                placebo,
            });
        }
    }
    out
}

/// The effects among `signals`: every signal of a kind some pair explains.
pub fn effects_of(signals: &[Signal], pairs: &[CausalPair]) -> Vec<Signal> {
    signals
        .iter()
        .filter(|s| pairs.iter().any(|p| p.effect == s.kind))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3_600;
    const M: i64 = 60;

    fn sig(id: &str, at: i64, on: &str, kind: SignalKind) -> Signal {
        Signal {
            id: id.into(),
            at,
            on: on.into(),
            kind,
            severity: 1.0,
            source_ref: id.into(),
        }
    }

    fn drives(from: i64, to: Option<i64>) -> Topology {
        Topology::new(vec![Edge {
            source: "controller".into(),
            target: "compressor".into(),
            from,
            to,
        }])
    }

    fn fridge_pairs() -> Vec<CausalPair> {
        vec![
            CausalPair {
                cause: SignalKind::ChangeConfig,
                effect: SignalKind::Alert,
                max_gap: 30 * M,
                prior: 0.6,
            },
            CausalPair {
                cause: SignalKind::DeviceOffline,
                effect: SignalKind::Alert,
                max_gap: 15 * M,
                prior: 0.5,
            },
        ]
    }

    // Day 10, 10:00 UTC.
    const T: i64 = 10 * 86_400 + 10 * H;

    #[test]
    fn setpoint_then_high_pressure_is_rank_one() {
        let signals = vec![
            sig("cfg", T, "controller", SignalKind::ChangeConfig),
            sig("hp", T + 12 * M, "compressor", SignalKind::Alert),
        ];
        let out = chains(
            &effects_of(&signals, &fridge_pairs()),
            &signals,
            &drives(0, None),
            &fridge_pairs(),
        );
        assert_eq!(out.len(), 1);
        let c = &out[0];
        assert_eq!(c.root().id, "cfg");
        assert_eq!(c.effect().id, "hp");
        assert_eq!(c.hops[0].gap, 12 * M);
        assert_eq!(c.hops[0].distance, 1);
        // 0.6 × exp(−12/10) × 1/2
        assert!((c.confidence - 0.6 * (-1.2f64).exp() / 2.0).abs() < 1e-12);
        assert_eq!(c.placebo, 0.0);
        assert!(c.lift >= DEFAULT_MIN_LIFT);
    }

    #[test]
    fn no_upstream_change_no_chain() {
        let signals = vec![sig("hp", T + 12 * M, "compressor", SignalKind::Alert)];
        assert!(chains(
            &effects_of(&signals, &fridge_pairs()),
            &signals,
            &drives(0, None),
            &fridge_pairs()
        )
        .is_empty());
    }

    #[test]
    fn a_link_closed_before_the_cause_breaks_the_chain() {
        let signals = vec![
            sig("cfg", T, "controller", SignalKind::ChangeConfig),
            sig("hp", T + 12 * M, "compressor", SignalKind::Alert),
        ];
        let closed = drives(0, Some(T - H));
        assert!(chains(
            &effects_of(&signals, &fridge_pairs()),
            &signals,
            &closed,
            &fridge_pairs()
        )
        .is_empty());
        // Downstream only: the alert never explains the setpoint.
        let reversed = Topology::new(vec![Edge {
            source: "compressor".into(),
            target: "controller".into(),
            from: 0,
            to: None,
        }]);
        assert!(chains(
            &effects_of(&signals, &fridge_pairs()),
            &signals,
            &reversed,
            &fridge_pairs()
        )
        .is_empty());
    }

    #[test]
    fn a_daily_alarm_after_a_daily_push_falls_under_the_lift() {
        let mut signals = Vec::new();
        for day in 0..10 {
            let t = day * 86_400 + 10 * H;
            signals.push(sig(
                &format!("cfg{day}"),
                t,
                "controller",
                SignalKind::ChangeConfig,
            ));
            signals.push(sig(
                &format!("hp{day}"),
                t + 12 * M,
                "compressor",
                SignalKind::Alert,
            ));
        }
        let pairs = fridge_pairs();
        let last = vec![signals.last().unwrap().clone()];
        let out = chains(&last, &signals, &drives(0, None), &pairs);
        assert_eq!(out.len(), 1);
        assert!(out[0].lift < DEFAULT_MIN_LIFT, "lift {}", out[0].lift);
    }

    #[test]
    fn offline_device_then_downstream_alarm() {
        let signals = vec![
            sig("off", T, "controller", SignalKind::DeviceOffline),
            sig("hp", T + 10 * M, "compressor", SignalKind::Alert),
        ];
        let out = chains(
            &effects_of(&signals, &fridge_pairs()),
            &signals,
            &drives(0, None),
            &fridge_pairs(),
        );
        assert_eq!(out[0].root().kind, SignalKind::DeviceOffline);
        // Past the 15 min gap: nothing.
        let late = vec![
            signals[0].clone(),
            sig("hp", T + 16 * M, "compressor", SignalKind::Alert),
        ];
        assert!(chains(
            &effects_of(&late, &fridge_pairs()),
            &late,
            &drives(0, None),
            &fridge_pairs()
        )
        .is_empty());
    }

    #[test]
    fn two_hop_chain_and_top_three() {
        let pairs = vec![
            CausalPair {
                cause: SignalKind::ChangeFirmware,
                effect: SignalKind::DeviceOffline,
                max_gap: 2 * H,
                prior: 0.5,
            },
            CausalPair {
                cause: SignalKind::DeviceOffline,
                effect: SignalKind::Alert,
                max_gap: 15 * M,
                prior: 0.5,
            },
            CausalPair {
                cause: SignalKind::ChangeConfig,
                effect: SignalKind::Alert,
                max_gap: 30 * M,
                prior: 0.6,
            },
        ];
        let mut signals = vec![
            sig("ota", T - H, "controller", SignalKind::ChangeFirmware),
            sig("off", T, "controller", SignalKind::DeviceOffline),
            sig("hp", T + 10 * M, "compressor", SignalKind::Alert),
        ];
        for i in 0..5 {
            signals.push(sig(
                &format!("cfg{i}"),
                T + i * M,
                "controller",
                SignalKind::ChangeConfig,
            ));
        }
        let effect = vec![signals[2].clone()];
        let out = chains(&effect, &signals, &drives(0, None), &pairs);
        assert_eq!(out.len(), TOP_PER_EFFECT);
        assert!(out.windows(2).all(|w| w[0].confidence >= w[1].confidence));
        let all = chains_to(&effect[0], &signals, &drives(0, None), &pairs, MAX_HOPS);
        assert!(all
            .iter()
            .any(|(h, _)| h.len() == 2 && h[0].cause.id == "ota"));
    }

    #[test]
    fn same_output_on_every_run() {
        let mut signals = Vec::new();
        for i in 0..40 {
            let kind = if i % 3 == 0 {
                SignalKind::Alert
            } else {
                SignalKind::ChangeConfig
            };
            signals.push(sig(
                &format!("s{i}"),
                T + i * 2 * M,
                if i % 2 == 0 {
                    "controller"
                } else {
                    "compressor"
                },
                kind,
            ));
        }
        let first = chains(
            &effects_of(&signals, &fridge_pairs()),
            &signals,
            &drives(0, None),
            &fridge_pairs(),
        );
        for _ in 0..100 {
            assert_eq!(
                chains(
                    &effects_of(&signals, &fridge_pairs()),
                    &signals,
                    &drives(0, None),
                    &fridge_pairs()
                ),
                first
            );
        }
    }

    #[test]
    fn gaps_read_like_people_write_them() {
        assert_eq!(gap_text(12 * M), "12 min");
        assert_eq!(gap_text(3 * H + 5 * M), "3 h 05");
        assert_eq!(gap_text(2 * 86_400 + 4 * H), "2 d 4 h");
    }

    #[test]
    fn pair_stats_count_and_flag() {
        let pair = PackPair {
            cause: SignalKind::ChangeConfig,
            effect: SignalKind::Alert,
            max_gap_min: 30,
            prior: 0.6,
        };
        let rule = vec![(SignalKind::ChangeConfig, SignalKind::Alert)];
        let mut props: Vec<_> = (0..19).map(|_| (rule.clone(), false, 1)).collect();
        props.push((rule.clone(), true, 2));
        props.push((
            vec![(SignalKind::DeviceOffline, SignalKind::Alert)],
            true,
            1,
        ));
        let s = &pair_stats(&[pair], &props)[0];
        assert_eq!((s.proposed, s.retained, s.mean_rank), (20, 1, Some(2.0)));
        assert!(s.flagged, "5 % of 20");
    }

    #[test]
    fn kinds_round_trip() {
        for k in SignalKind::ALL {
            assert_eq!(SignalKind::parse(k.as_str()), Some(k));
            assert_eq!(serde_json::to_value(k).unwrap(), k.as_str());
        }
    }
}
