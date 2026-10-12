//! Position tracks of a case (cases-workspace.md K25–K28): normalisation at
//! the boundary, split at signal gaps, sampling at the cursor, time-aware
//! generalisation. Pure functions, wasm-safe: the detectors (server) and the
//! inspector (front) share them.
//!
//! Algorithms follow MovingPandas (BSD-3-Clause, © MovingPandas developers):
//! `ObservationGapSplitter` (strict `>` gap) and `TopDownTimeRatioGeneralizer`
//! (Meratnia & de By). See docs/bench/dossier-enquete/movingpandas.md.

pub mod detect;

use serde::{Deserialize, Serialize};

/// Mean Earth radius (m), IUGG.
const EARTH_RADIUS_M: f64 = 6_371_008.8;
/// Widest window of tracks served or scanned by the detectors in one go
/// (K27): wider, the timeline shows no track and invites to zoom in.
pub const MAX_WINDOW_DAYS: i64 = 31;

/// One observed position. `t_ms` is Unix time in milliseconds (UTC).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrackPoint {
    pub t_ms: i64,
    pub lon: f64,
    pub lat: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<f64>,
}

/// Why an input point was dropped by [`normalize`] (machine token, i18n by the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// Latitude or longitude is not a finite number.
    NotFinite,
    /// Latitude outside [-90, 90] or longitude outside [-180, 180].
    OutOfRange,
    /// Same timestamp as a later point: the later one wins.
    DuplicateTime,
    /// An imported row whose time or coordinates cannot be read.
    Unreadable,
}

/// A dropped point: its index in the input and the reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejected {
    pub index: usize,
    pub reason: RejectReason,
}

/// Output of [`normalize`]: clean points sorted by time, and what was dropped.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Normalized {
    pub points: Vec<TrackPoint>,
    pub rejected: Vec<Rejected>,
}

/// Cleans raw points: drops invalid coordinates, sorts by time (stable), and
/// keeps the last point of each timestamp. Every drop is reported so an import
/// can list its rejected rows instead of discarding them silently.
pub fn normalize(raw: Vec<TrackPoint>) -> Normalized {
    let mut rejected = Vec::new();
    let mut kept: Vec<(usize, TrackPoint)> = Vec::with_capacity(raw.len());
    for (index, p) in raw.into_iter().enumerate() {
        if !p.lat.is_finite() || !p.lon.is_finite() {
            rejected.push(Rejected {
                index,
                reason: RejectReason::NotFinite,
            });
        } else if !(-90.0..=90.0).contains(&p.lat) || !(-180.0..=180.0).contains(&p.lon) {
            rejected.push(Rejected {
                index,
                reason: RejectReason::OutOfRange,
            });
        } else {
            // A non-finite altitude is dropped, not the point.
            let alt = p.alt.filter(|a| a.is_finite());
            kept.push((index, TrackPoint { alt, ..p }));
        }
    }
    kept.sort_by_key(|(_, p)| p.t_ms);
    let mut points: Vec<TrackPoint> = Vec::with_capacity(kept.len());
    let mut last_index = 0usize;
    for (index, p) in kept {
        match points.last_mut() {
            Some(prev) if prev.t_ms == p.t_ms => {
                rejected.push(Rejected {
                    index: last_index,
                    reason: RejectReason::DuplicateTime,
                });
                *prev = p;
            }
            _ => points.push(p),
        }
        last_index = index;
    }
    rejected.sort_by_key(|r| r.index);
    Normalized { points, rejected }
}

/// Splits sorted points into segments wherever two consecutive observations
/// are more than `max_gap_ms` apart (strict `>`, like MovingPandas). A segment
/// boundary is a signal gap: nothing is interpolated across it.
pub fn split_gaps(points: &[TrackPoint], max_gap_ms: i64) -> Vec<Vec<TrackPoint>> {
    let mut out: Vec<Vec<TrackPoint>> = Vec::new();
    for p in points {
        match out.last_mut() {
            Some(seg) if seg.last().is_some_and(|q| p.t_ms - q.t_ms <= max_gap_ms) => seg.push(*p),
            _ => out.push(vec![*p]),
        }
    }
    out
}

/// Position `(lon, lat)` at `t_ms` inside one segment (sorted, gap-free), by
/// binary search and linear interpolation. `None` outside the segment's span,
/// so a cursor in a signal gap shows no position.
pub fn sample(segment: &[TrackPoint], t_ms: i64) -> Option<(f64, f64)> {
    let first = segment.first()?;
    let last = segment.last()?;
    if t_ms < first.t_ms || t_ms > last.t_ms {
        return None;
    }
    // First index whose time is >= t.
    let hi = segment.partition_point(|p| p.t_ms < t_ms);
    let b = segment[hi];
    if b.t_ms == t_ms || hi == 0 {
        return Some((b.lon, b.lat));
    }
    let a = segment[hi - 1];
    let k = (t_ms - a.t_ms) as f64 / (b.t_ms - a.t_ms) as f64;
    Some((a.lon + (b.lon - a.lon) * k, a.lat + (b.lat - a.lat) * k))
}

/// Great-circle distance in metres between two `(lon, lat)` points.
pub fn haversine_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (lon1, lat1) = (a.0.to_radians(), a.1.to_radians());
    let (lon2, lat2) = (b.0.to_radians(), b.1.to_radians());
    let h = ((lat2 - lat1) / 2.0).sin().powi(2)
        + lat1.cos() * lat2.cos() * ((lon2 - lon1) / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * h.sqrt().asin()
}

/// Local planar frame in metres around a reference point, so distances are
/// never computed on raw degrees.
// shortcut: equirectangular projection, error < 0.1 % within ~50 km of the
// reference; use a true ENU/geodesic projection if tracks span countries.
#[derive(Debug, Clone, Copy)]
pub struct LocalFrame {
    lon0: f64,
    lat0: f64,
    cos_lat0: f64,
}

impl LocalFrame {
    pub fn new(lon0: f64, lat0: f64) -> Self {
        Self {
            lon0,
            lat0,
            cos_lat0: lat0.to_radians().cos(),
        }
    }

    /// Frame centred on the bounding box of the points (`None` if empty).
    pub fn around(points: &[TrackPoint]) -> Option<Self> {
        let first = points.first()?;
        let (mut min_lon, mut max_lon, mut min_lat, mut max_lat) =
            (first.lon, first.lon, first.lat, first.lat);
        for p in points {
            min_lon = min_lon.min(p.lon);
            max_lon = max_lon.max(p.lon);
            min_lat = min_lat.min(p.lat);
            max_lat = max_lat.max(p.lat);
        }
        Some(Self::new(
            (min_lon + max_lon) / 2.0,
            (min_lat + max_lat) / 2.0,
        ))
    }

    /// `(x east, y north)` in metres.
    pub fn xy(&self, lon: f64, lat: f64) -> (f64, f64) {
        let x = (lon - self.lon0).to_radians() * EARTH_RADIUS_M * self.cos_lat0;
        let y = (lat - self.lat0).to_radians() * EARTH_RADIUS_M;
        (x, y)
    }
}

/// Top-Down Time-Ratio generalisation: keeps a point when its distance to the
/// position predicted by constant-speed motion between the kept neighbours
/// exceeds `tolerance_m`. Unlike Douglas-Peucker it keeps stops on a straight
/// line (the time matters). Endpoints are always kept.
pub fn tdtr(segment: &[TrackPoint], tolerance_m: f64) -> Vec<TrackPoint> {
    if segment.len() <= 2 {
        return segment.to_vec();
    }
    let frame = match LocalFrame::around(segment) {
        Some(f) => f,
        None => return Vec::new(),
    };
    let xy: Vec<(f64, f64)> = segment.iter().map(|p| frame.xy(p.lon, p.lat)).collect();
    let mut keep = vec![false; segment.len()];
    keep[0] = true;
    keep[segment.len() - 1] = true;
    // Explicit stack instead of recursion: long tracks must not overflow wasm.
    let mut stack = vec![(0usize, segment.len() - 1)];
    while let Some((s, e)) = stack.pop() {
        if e <= s + 1 {
            continue;
        }
        let dt = (segment[e].t_ms - segment[s].t_ms) as f64;
        let mut worst = (0usize, -1.0f64);
        for i in s + 1..e {
            let k = if dt > 0.0 {
                (segment[i].t_ms - segment[s].t_ms) as f64 / dt
            } else {
                0.0
            };
            let ex = xy[s].0 + (xy[e].0 - xy[s].0) * k;
            let ey = xy[s].1 + (xy[e].1 - xy[s].1) * k;
            let d = (xy[i].0 - ex).hypot(xy[i].1 - ey);
            if d > worst.1 {
                worst = (i, d);
            }
        }
        if worst.1 > tolerance_m {
            keep[worst.0] = true;
            stack.push((s, worst.0));
            stack.push((worst.0, e));
        }
    }
    segment
        .iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(t_ms: i64, lon: f64, lat: f64) -> TrackPoint {
        TrackPoint {
            t_ms,
            lon,
            lat,
            alt: None,
        }
    }

    #[test]
    fn normalize_sorts_dedupes_and_reports_rejects() {
        let raw = vec![
            p(3_000, 2.0, 48.0),
            p(1_000, 2.0, 48.0),
            p(2_000, f64::NAN, 48.0),
            p(1_000, 2.1, 48.1),
            p(4_000, 2.0, 95.0),
        ];
        let n = normalize(raw);
        assert_eq!(
            n.points.iter().map(|q| q.t_ms).collect::<Vec<_>>(),
            vec![1_000, 3_000]
        );
        // The later duplicate (input index 3) wins over index 1.
        assert_eq!(n.points[0].lon, 2.1);
        assert_eq!(
            n.rejected,
            vec![
                Rejected {
                    index: 1,
                    reason: RejectReason::DuplicateTime
                },
                Rejected {
                    index: 2,
                    reason: RejectReason::NotFinite
                },
                Rejected {
                    index: 4,
                    reason: RejectReason::OutOfRange
                },
            ]
        );
    }

    #[test]
    fn split_gaps_is_strict() {
        let pts = [p(0, 0.0, 0.0), p(300_000, 0.0, 0.0), p(600_001, 0.0, 0.0)];
        let segs = split_gaps(&pts, 300_000);
        // 300 000 ms is not a gap, 300 001 ms is.
        assert_eq!(segs.iter().map(Vec::len).collect::<Vec<_>>(), vec![2, 1]);
        assert!(split_gaps(&[], 1).is_empty());
    }

    #[test]
    fn sample_interpolates_inside_and_refuses_outside() {
        let seg = &[p(0, 2.0, 48.0), p(10_000, 2.2, 48.2)];
        let (lon, lat) = sample(seg, 5_000).unwrap();
        assert!((lon - 2.1).abs() < 1e-12 && (lat - 48.1).abs() < 1e-12);
        assert_eq!(sample(seg, 10_000), Some((2.2, 48.2)));
        assert_eq!(sample(seg, 0), Some((2.0, 48.0)));
        assert_eq!(sample(seg, -1), None);
        assert_eq!(sample(seg, 10_001), None);
        assert_eq!(sample(&[], 0), None);
    }

    #[test]
    fn haversine_paris_london() {
        let d = haversine_m((2.3522, 48.8566), (-0.1278, 51.5074));
        assert!((d - 343_500.0).abs() < 1_500.0, "{d}");
    }

    #[test]
    fn tdtr_drops_constant_speed_points_but_keeps_a_stop() {
        // Straight line at constant speed: only the endpoints survive.
        let line: Vec<_> = (0..=10)
            .map(|i| p(i * 1_000, 2.0 + i as f64 * 1e-4, 48.0))
            .collect();
        assert_eq!(tdtr(&line, 1.0).len(), 2);
        // Same straight path, but the mover waits 5 s halfway: Douglas-Peucker
        // would keep 2 points, the time ratio keeps the stop.
        let mut stop: Vec<_> = (0..=5)
            .map(|i| p(i * 1_000, 2.0 + i as f64 * 1e-4, 48.0))
            .collect();
        stop.extend((1..=5).map(|i| p(5_000 + i * 1_000, 2.0005, 48.0)));
        stop.extend((1..=5).map(|i| p(10_000 + i * 1_000, 2.0005 + i as f64 * 1e-4, 48.0)));
        let g = tdtr(&stop, 1.0);
        assert!(g.len() > 2, "{g:?}");
        assert!(g.iter().any(|q| q.t_ms == 5_000) && g.iter().any(|q| q.t_ms == 10_000));
    }
}
