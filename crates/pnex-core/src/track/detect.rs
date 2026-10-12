//! Detectors over the tracks of a case window (cases-workspace.md K30):
//! co-presence of two members, stops, signal losses, visits of an area. Pure functions on
//! gap-free segments ([`super::split_gaps`]); a signal gap is never bridged,
//! so nothing is detected while a member is unobserved. The server turns the
//! results into proposals; a human retains or rejects them.

use serde::{Deserialize, Serialize};

use super::{haversine_m, sample, LocalFrame, TrackPoint};

/// Exit threshold of a co-presence, as a factor of the entry distance
/// (hysteresis: a member hovering at the threshold is one encounter, not ten).
pub const CO_PRESENCE_EXIT_FACTOR: f64 = 1.2;
/// Default thresholds, used when a case does not set its own (K30).
pub const DEFAULT_CO_DISTANCE_M: f64 = 40.0;
pub const DEFAULT_CO_MIN_S: i64 = 120;
pub const DEFAULT_STOP_DIAMETER_M: f64 = 25.0;
pub const DEFAULT_STOP_MIN_S: i64 = 900;
pub const DEFAULT_SIGNAL_GAP_S: i64 = 300;

/// Two members within `distance_m` of each other for a while.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CoPresence {
    pub from_ms: i64,
    pub to_ms: i64,
    pub min_distance_m: f64,
    /// Midpoint of the two members at their closest, `(lon, lat)`.
    pub at: (f64, f64),
}

/// A member staying within a small circle for a while.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    pub from_ms: i64,
    pub to_ms: i64,
    /// Median position, `(lon, lat)`.
    pub at: (f64, f64),
}

/// A member's signal stopping: until it comes back (`to_ms`), or still lost
/// at the end of the window (`None`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SignalLoss {
    pub from_ms: i64,
    pub to_ms: Option<i64>,
    /// Last known position, `(lon, lat)`.
    pub at: (f64, f64),
}

/// A member inside an area. `entered` / `left` say whether the crossing was
/// observed; false means the visit began or ended in a signal gap.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ZoneVisit {
    pub from_ms: i64,
    pub to_ms: i64,
    pub entered: bool,
    pub left: bool,
}

/// Instant where the straight move from `a` (inside = `was`) to `b` crosses
/// the boundary, by bisection on the interpolated position (to the ms).
fn crossing(a: &TrackPoint, b: &TrackPoint, was: bool, inside: &impl Fn(f64, f64) -> bool) -> i64 {
    let (mut lo, mut hi) = (a.t_ms, b.t_ms);
    let seg = [*a, *b];
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        let (lon, lat) = sample(&seg, mid).unwrap_or((a.lon, a.lat));
        if inside(lon, lat) == was {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

/// Visits of an area by one member, per gap-free segment (a gap ends a
/// visit, unobserved). Crossing instants are interpolated between fixes.
// shortcut: one crossing per pair of fixes; an area thinner than the step
// between two fixes can be crossed unseen (1 Hz tracks: under ~30 m at speed).
pub fn zone_visits(
    segments: &[Vec<TrackPoint>],
    inside: impl Fn(f64, f64) -> bool,
) -> Vec<ZoneVisit> {
    let mut out = Vec::new();
    for seg in segments {
        let mut open: Option<ZoneVisit> = None;
        let mut prev: Option<(&TrackPoint, bool)> = None;
        for p in seg {
            let now = inside(p.lon, p.lat);
            match (prev, now) {
                (None, true) => {
                    open = Some(ZoneVisit {
                        from_ms: p.t_ms,
                        to_ms: p.t_ms,
                        entered: false,
                        left: false,
                    })
                }
                (Some((a, false)), true) => {
                    let t = crossing(a, p, false, &inside);
                    open = Some(ZoneVisit {
                        from_ms: t,
                        to_ms: p.t_ms,
                        entered: true,
                        left: false,
                    });
                }
                (Some((a, true)), false) => {
                    if let Some(mut v) = open.take() {
                        v.to_ms = crossing(a, p, true, &inside);
                        v.left = true;
                        out.push(v);
                    }
                }
                (Some(_), true) => {
                    if let Some(v) = open.as_mut() {
                        v.to_ms = p.t_ms;
                    }
                }
                _ => {}
            }
            prev = Some((p, now));
        }
        out.extend(open);
    }
    out
}

/// Spans where members `a` and `b` are within `distance_m` for at least
/// `min_ms`, leaving at [`CO_PRESENCE_EXIT_FACTOR`] × the distance. Distances
/// are evaluated at every observed instant of either member while both are
/// observed (same segment pair).
// shortcut: vertex resolution (the span edges are observed instants, 1 s at
// 1 Hz); solve the quadratic relative distance between instants if coarser
// tracks need sub-sample edges.
pub fn co_presence(
    a: &[Vec<TrackPoint>],
    b: &[Vec<TrackPoint>],
    distance_m: f64,
    min_ms: i64,
) -> Vec<CoPresence> {
    let exit_m = distance_m * CO_PRESENCE_EXIT_FACTOR;
    let mut out = Vec::new();
    for sa in a {
        for sb in b {
            let (Some(a0), Some(a1), Some(b0), Some(b1)) =
                (sa.first(), sa.last(), sb.first(), sb.last())
            else {
                continue;
            };
            let (from, to) = (a0.t_ms.max(b0.t_ms), a1.t_ms.min(b1.t_ms));
            if from > to {
                continue;
            }
            let mut instants: Vec<i64> = sa
                .iter()
                .chain(sb.iter())
                .map(|p| p.t_ms)
                .filter(|t| (from..=to).contains(t))
                .chain([from, to])
                .collect();
            instants.sort_unstable();
            instants.dedup();
            let mut open: Option<CoPresence> = None;
            let close = |c: CoPresence, out: &mut Vec<CoPresence>| {
                if c.to_ms - c.from_ms >= min_ms {
                    out.push(c);
                }
            };
            for t in instants {
                let (Some(pa), Some(pb)) = (sample(sa, t), sample(sb, t)) else {
                    continue;
                };
                let d = haversine_m(pa, pb);
                let mid = ((pa.0 + pb.0) / 2.0, (pa.1 + pb.1) / 2.0);
                match open.as_mut() {
                    Some(c) if d <= exit_m => {
                        c.to_ms = t;
                        if d < c.min_distance_m {
                            c.min_distance_m = d;
                            c.at = mid;
                        }
                    }
                    Some(_) => close(open.take().unwrap(), &mut out),
                    None if d <= distance_m => {
                        open = Some(CoPresence {
                            from_ms: t,
                            to_ms: t,
                            min_distance_m: d,
                            at: mid,
                        })
                    }
                    None => {}
                }
            }
            if let Some(c) = open {
                close(c, &mut out);
            }
        }
    }
    out.sort_by_key(|c| c.from_ms);
    out
}

/// Diameter of `points`, as the diagonal of their bounding box in metres
/// (never smaller than the true diameter: a stop found here is tight).
fn diameter_m(points: &[TrackPoint]) -> f64 {
    let Some(frame) = LocalFrame::around(points) else {
        return 0.0;
    };
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in points {
        let (x, y) = frame.xy(p.lon, p.lat);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt()
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Stops of one segment: the longest runs that fit in `diameter_m` and last
/// at least `min_ms` (sliding window, MovingPandas `TrajectoryStopDetector`).
pub fn stops(segment: &[TrackPoint], diameter: f64, min_ms: i64) -> Vec<Stop> {
    let mut out = Vec::new();
    let emit = |run: &[TrackPoint], out: &mut Vec<Stop>| {
        out.push(Stop {
            from_ms: run[0].t_ms,
            to_ms: run[run.len() - 1].t_ms,
            at: (
                median(run.iter().map(|p| p.lon).collect()),
                median(run.iter().map(|p| p.lat).collect()),
            ),
        });
    };
    let (mut i, mut j) = (0, 0);
    while j < segment.len() {
        if diameter_m(&segment[i..=j]) <= diameter {
            j += 1;
            continue;
        }
        // segment[i..j] fits, adding j does not.
        if segment[j - 1].t_ms - segment[i].t_ms >= min_ms {
            emit(&segment[i..j], &mut out);
            i = j;
        } else {
            i += 1;
        }
    }
    if i < segment.len() && segment[segment.len() - 1].t_ms - segment[i].t_ms >= min_ms {
        emit(&segment[i..], &mut out);
    }
    out
}

/// Signal losses of a member: each gap between two segments, and a track
/// ending more than `gap_ms` before `until_ms` (the end of the window, or now).
pub fn signal_losses(segments: &[Vec<TrackPoint>], until_ms: i64, gap_ms: i64) -> Vec<SignalLoss> {
    let mut out = Vec::new();
    for (k, seg) in segments.iter().enumerate() {
        let Some(last) = seg.last() else { continue };
        let next = segments[k + 1..].iter().find_map(|s| s.first());
        match next {
            Some(n) => out.push(SignalLoss {
                from_ms: last.t_ms,
                to_ms: Some(n.t_ms),
                at: (last.lon, last.lat),
            }),
            None if until_ms - last.t_ms > gap_ms => out.push(SignalLoss {
                from_ms: last.t_ms,
                to_ms: None,
                at: (last.lon, last.lat),
            }),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ~11.1 m per 0.0001° of latitude.
    fn p(t_s: i64, lon: f64, lat: f64) -> TrackPoint {
        TrackPoint {
            t_ms: t_s * 1_000,
            lon,
            lat,
            alt: None,
        }
    }

    /// A member walking north at ~1.1 m/s from `lat0`, one point per second.
    fn walk(from_s: i64, to_s: i64, lat0: f64) -> Vec<TrackPoint> {
        (from_s..=to_s)
            .map(|s| p(s, 2.0, lat0 + (s - from_s) as f64 * 0.00001))
            .collect()
    }

    fn still(from_s: i64, to_s: i64, lat: f64) -> Vec<TrackPoint> {
        (from_s..=to_s).map(|s| p(s, 2.0, lat)).collect()
    }

    #[test]
    fn co_presence_needs_both_close_long_enough() {
        // b stands still where a walks past: close for ~70 s around t=300.
        let a = vec![walk(0, 600, 48.0)];
        let b = vec![still(0, 600, 48.003)];
        let found = co_presence(&a, &b, 40.0, 60_000);
        assert_eq!(found.len(), 1, "{found:?}");
        let c = found[0];
        assert!(c.from_ms < 300_000 && c.to_ms > 300_000, "{c:?}");
        assert!(c.min_distance_m < 1.0, "{c:?}");
        // Too short for a 2 min threshold.
        assert!(co_presence(&a, &b, 40.0, 120_000).is_empty());
    }

    #[test]
    fn co_presence_is_unknown_during_a_gap() {
        // Together the whole time, but b is unobserved from 100 s to 400 s.
        let a = vec![still(0, 500, 48.0)];
        let b = vec![still(0, 100, 48.0), still(400, 500, 48.0)];
        let found = co_presence(&a, &b, 40.0, 30_000);
        assert_eq!(
            found
                .iter()
                .map(|c| (c.from_ms, c.to_ms))
                .collect::<Vec<_>>(),
            vec![(0, 100_000), (400_000, 500_000)]
        );
    }

    #[test]
    fn co_presence_hysteresis_keeps_one_encounter() {
        // b oscillates between 35 m and 45 m: above the entry, under the exit.
        let a = vec![still(0, 300, 48.0)];
        let b = vec![(0..=300)
            .map(|s| p(s, 2.0, 48.0 + if s % 2 == 0 { 0.000315 } else { 0.000405 }))
            .collect::<Vec<_>>()];
        assert_eq!(co_presence(&a, &b, 40.0, 60_000).len(), 1);
    }

    #[test]
    fn stops_find_the_still_part_only() {
        let mut seg = walk(0, 300, 48.0);
        let lat = seg.last().unwrap().lat;
        seg.extend(still(301, 1_300, lat));
        seg.extend(walk(1_301, 1_600, lat));
        let found = stops(&seg, 25.0, 900_000);
        assert_eq!(found.len(), 1, "{found:?}");
        let s = found[0];
        assert!(s.from_ms <= 301_000 && s.to_ms >= 1_300_000, "{s:?}");
        assert!((s.at.1 - lat).abs() < 1e-9);
        assert!(stops(&seg, 25.0, 1_200_000).is_empty());
        assert!(stops(&walk(0, 600, 48.0), 25.0, 60_000).is_empty());
    }

    #[test]
    fn zone_visits_interpolate_crossings_and_stop_at_gaps() {
        let area = crate::geo_area::GeoArea::parse(&serde_json::json!({
            "center": [2.0, 48.002], "radius_m": 50.0
        }))
        .unwrap();
        let inside = |lon, lat| area.contains(lon, lat);
        // Walking north through the circle: ~11 m/s steps of 0.0001°.
        let seg: Vec<TrackPoint> = (0..=40)
            .map(|s| p(s, 2.0, 48.0 + s as f64 * 0.0001))
            .collect();
        let v = zone_visits(&[seg.clone()], inside);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].entered && v[0].left);
        // Enters ~50 m before the centre (t≈15.5 s), leaves ~50 m after (t≈24.5 s).
        assert!((v[0].from_ms - 15_500).abs() < 100, "{v:?}");
        assert!((v[0].to_ms - 24_500).abs() < 100, "{v:?}");
        // A gap inside the area: two visits, each with an unobserved end.
        let split = vec![seg[..19].to_vec(), seg[21..].to_vec()];
        let v = zone_visits(&split, inside);
        assert_eq!(
            v.iter().map(|z| (z.entered, z.left)).collect::<Vec<_>>(),
            vec![(true, false), (false, true)]
        );
    }

    #[test]
    fn signal_losses_mark_gaps_and_a_silent_end() {
        let segs = vec![still(0, 100, 48.0), still(500, 600, 48.0)];
        let found = signal_losses(&segs, 2_000_000, 300_000);
        assert_eq!(
            found
                .iter()
                .map(|l| (l.from_ms, l.to_ms))
                .collect::<Vec<_>>(),
            vec![(100_000, Some(500_000)), (600_000, None)]
        );
        // Ending just before the window end is not a loss.
        assert_eq!(signal_losses(&segs, 700_000, 300_000).len(), 1);
    }
}
