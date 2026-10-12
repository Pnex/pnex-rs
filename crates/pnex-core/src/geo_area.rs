//! Areas (cases-workspace.md K29): the `geo_area` property kind, a circle
//! `{"center": {"lon", "lat"}, "radius_m"}` or a GeoJSON polygon with one
//! outer ring `{"type": "Polygon", "coordinates": [[[lon, lat], ...]]}`
//! (closed, at most [`MAX_VERTICES`] vertices, not self-intersecting).
//! Validated here for the server and the wasm front alike; point-in-area runs
//! in a local metric frame, never on raw degrees.

use serde_json::{json, Value};

use crate::track::{haversine_m, LocalFrame};

/// Vertices of a polygon ring at most (the closing point not counted).
pub const MAX_VERTICES: usize = 1_000;
/// Largest circle radius.
pub const MAX_RADIUS_M: f64 = 100_000.0;

#[derive(Debug, Clone, PartialEq)]
pub enum GeoArea {
    Circle {
        center: (f64, f64),
        radius_m: f64,
    },
    /// Closed ring `(lon, lat)`, first point repeated last.
    Polygon {
        ring: Vec<(f64, f64)>,
    },
}

fn point(v: &Value) -> Option<(f64, f64)> {
    let (lon, lat) = match v {
        Value::Array(a) if a.len() == 2 => (a[0].as_f64()?, a[1].as_f64()?),
        Value::Object(o) if o.len() == 2 => (o.get("lon")?.as_f64()?, o.get("lat")?.as_f64()?),
        _ => return None,
    };
    ((-180.0..=180.0).contains(&lon) && (-90.0..=90.0).contains(&lat)).then_some((lon, lat))
}

fn orient(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

/// Proper or touching intersection of segments `ab` and `cd`.
fn crosses(a: (f64, f64), b: (f64, f64), c: (f64, f64), d: (f64, f64)) -> bool {
    let (d1, d2) = (orient(c, d, a), orient(c, d, b));
    let (d3, d4) = (orient(a, b, c), orient(a, b, d));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    let on = |p: (f64, f64), q: (f64, f64), r: (f64, f64), o: f64| {
        o == 0.0
            && r.0 >= p.0.min(q.0)
            && r.0 <= p.0.max(q.0)
            && r.1 >= p.1.min(q.1)
            && r.1 <= p.1.max(q.1)
    };
    on(c, d, a, d1) || on(c, d, b, d2) || on(a, b, c, d3) || on(a, b, d, d4)
}

/// A simple ring: no two non-adjacent edges meet.
// shortcut: O(n²) edge pairs, fine up to MAX_VERTICES; sweep-line if raised.
fn simple(ring: &[(f64, f64)]) -> bool {
    let n = ring.len() - 1;
    for i in 0..n {
        for j in i + 1..n {
            let adjacent = j == i + 1 || (i == 0 && j == n - 1);
            if !adjacent && crosses(ring[i], ring[i + 1], ring[j], ring[j + 1]) {
                return false;
            }
        }
    }
    true
}

impl GeoArea {
    /// A valid area from its JSON value, `None` otherwise.
    pub fn parse(v: &Value) -> Option<Self> {
        let o = v.as_object()?;
        if let Some(r) = o.get("radius_m") {
            let radius_m = r.as_f64()?;
            if o.len() != 2 || !(radius_m > 0.0 && radius_m <= MAX_RADIUS_M) {
                return None;
            }
            return Some(Self::Circle {
                center: point(o.get("center")?)?,
                radius_m,
            });
        }
        if o.len() != 2 || o.get("type")?.as_str()? != "Polygon" {
            return None;
        }
        let rings = o.get("coordinates")?.as_array()?;
        if rings.len() != 1 {
            return None;
        }
        let ring: Vec<(f64, f64)> = rings[0]
            .as_array()?
            .iter()
            .map(point)
            .collect::<Option<_>>()?;
        if ring.len() < 4
            || ring.len() > MAX_VERTICES + 1
            || ring.first() != ring.last()
            || !simple(&ring)
        {
            return None;
        }
        Some(Self::Polygon { ring })
    }

    /// Whether `(lon, lat)` lies inside (the boundary counts as inside).
    pub fn contains(&self, lon: f64, lat: f64) -> bool {
        match self {
            Self::Circle { center, radius_m } => haversine_m(*center, (lon, lat)) <= *radius_m,
            Self::Polygon { ring } => {
                let frame = LocalFrame::new(ring[0].0, ring[0].1);
                let (x, y) = frame.xy(lon, lat);
                let xy: Vec<(f64, f64)> = ring.iter().map(|p| frame.xy(p.0, p.1)).collect();
                let mut inside = false;
                for w in xy.windows(2) {
                    let ((x1, y1), (x2, y2)) = (w[0], w[1]);
                    if orient(w[0], w[1], (x, y)) == 0.0
                        && x >= x1.min(x2)
                        && x <= x1.max(x2)
                        && y >= y1.min(y2)
                        && y <= y1.max(y2)
                    {
                        return true;
                    }
                    if (y1 > y) != (y2 > y) && x < x1 + (y - y1) * (x2 - x1) / (y2 - y1) {
                        inside = !inside;
                    }
                }
                inside
            }
        }
    }

    /// A representative point: the centre, or the mean of the ring's vertices.
    pub fn center(&self) -> (f64, f64) {
        match self {
            Self::Circle { center, .. } => *center,
            Self::Polygon { ring } => {
                let n = (ring.len() - 1) as f64;
                let (sx, sy) = ring[..ring.len() - 1]
                    .iter()
                    .fold((0.0, 0.0), |(a, b), p| (a + p.0, b + p.1));
                (sx / n, sy / n)
            }
        }
    }

    /// GeoJSON polygon for drawing (a circle as a `segments`-gon).
    pub fn to_geojson(&self, segments: usize) -> Value {
        let ring: Vec<[f64; 2]> = match self {
            Self::Polygon { ring } => ring.iter().map(|p| [p.0, p.1]).collect(),
            Self::Circle { center, radius_m } => {
                let (lon0, lat0) = *center;
                let dlat = (radius_m / 6_371_008.8).to_degrees();
                let dlon = dlat / lat0.to_radians().cos().max(1e-6);
                let mut r: Vec<[f64; 2]> = (0..segments)
                    .map(|i| {
                        let a = i as f64 / segments as f64 * std::f64::consts::TAU;
                        [lon0 + dlon * a.cos(), lat0 + dlat * a.sin()]
                    })
                    .collect();
                r.push(r[0]);
                r
            }
        };
        json!({ "type": "Polygon", "coordinates": [ring] })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Value {
        json!({"type": "Polygon", "coordinates": [[[2.0, 48.0], [2.01, 48.0], [2.01, 48.01], [2.0, 48.01], [2.0, 48.0]]]})
    }

    #[test]
    fn parses_circles_and_simple_polygons_only() {
        assert!(
            GeoArea::parse(&json!({"center": {"lon": 2.0, "lat": 48.0}, "radius_m": 50.0}))
                .is_some()
        );
        assert!(GeoArea::parse(&square()).is_some());
        for bad in [
            json!({"center": {"lon": 2.0, "lat": 48.0}, "radius_m": 0}),
            json!({"center": {"lon": 2.0, "lat": 98.0}, "radius_m": 10}),
            json!({"center": {"lon": 2.0, "lat": 48.0}, "radius_m": 10, "x": 1}),
            // Not closed.
            json!({"type": "Polygon", "coordinates": [[[2.0, 48.0], [2.01, 48.0], [2.01, 48.01], [2.0, 48.01]]]}),
            // Bow tie: self-intersecting.
            json!({"type": "Polygon", "coordinates": [[[2.0, 48.0], [2.01, 48.01], [2.01, 48.0], [2.0, 48.01], [2.0, 48.0]]]}),
            // A hole: one outer ring only.
            json!({"type": "Polygon", "coordinates": [[[2.0, 48.0], [2.01, 48.0], [2.01, 48.01], [2.0, 48.0]], [[2.0, 48.0], [2.01, 48.0], [2.01, 48.01], [2.0, 48.0]]]}),
            json!({"lat": 48.0, "lon": 2.0}),
        ] {
            assert!(GeoArea::parse(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn contains_in_metres() {
        let c = GeoArea::parse(&json!({"center": [2.0, 48.0], "radius_m": 40.0})).unwrap();
        // ~33 m north, then ~55 m north.
        assert!(c.contains(2.0, 48.0003));
        assert!(!c.contains(2.0, 48.0005));
        let p = GeoArea::parse(&square()).unwrap();
        assert!(p.contains(2.005, 48.005));
        assert!(p.contains(2.0, 48.005), "boundary");
        assert!(!p.contains(2.02, 48.005));
        let (lon, lat) = p.center();
        assert!((lon - 2.005).abs() < 1e-9 && (lat - 48.005).abs() < 1e-9);
    }

    #[test]
    fn a_circle_draws_as_a_closed_ring() {
        let c = GeoArea::parse(&json!({"center": [2.0, 48.0], "radius_m": 40.0})).unwrap();
        let g = c.to_geojson(32);
        let ring = g["coordinates"][0].as_array().unwrap();
        assert_eq!(ring.len(), 33);
        assert_eq!(ring[0], ring[32]);
        let lat = ring[8][1].as_f64().unwrap();
        assert!((haversine_m((2.0, 48.0), (2.0, lat)) - 40.0).abs() < 0.5);
    }
}
