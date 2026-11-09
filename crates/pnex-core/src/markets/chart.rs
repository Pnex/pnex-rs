//! The issuer chart (markets-research.md D813): the raw closes of an
//! instrument and, on the same time line, its abnormal sessions and its
//! issuer's filings. Types shared by the server and the page, and the pure
//! geometry of the drawing.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChartPoint {
    #[cfg_attr(feature = "openapi", schema(format = "date"))]
    pub date: String,
    /// Raw close as last known, never adjusted (D525).
    pub value: f64,
}

/// What a marker on the time line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum ChartEventKind {
    /// A `market_event` of the instrument (abnormal session, D734).
    Move,
    /// A filing of the issuer (8-K, 10-Q, 10-K…).
    Filing,
    /// The last decision of a day of an external model (D897).
    Decision,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChartEvent {
    /// Object id of the event or the filing.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    #[cfg_attr(feature = "openapi", schema(format = "date"))]
    pub date: String,
    pub kind: ChartEventKind,
    pub title: String,
    /// `up` / `down` for a move, the form (`8-K`…) for a filing, the most
    /// likely option for a decision.
    pub detail: String,
    /// Model and version of a decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Probability of the most likely option of a decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probability: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct IssuerChart {
    /// The instrument drawn; `None` when the issuer has none.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub instrument_id: Option<String>,
    pub ticker: Option<String>,
    /// Oldest first.
    pub closes: Vec<ChartPoint>,
    /// Oldest first.
    pub events: Vec<ChartEvent>,
}

/// Size of the drawing (SVG view box), padding included.
pub const W: f64 = 800.0;
pub const H: f64 = 240.0;
const PAD_L: f64 = 56.0;
const PAD_R: f64 = 12.0;
const PAD_T: f64 = 12.0;
const PAD_B: f64 = 28.0;

fn day_number(d: &str) -> Option<i64> {
    chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
        .ok()
        .map(|d| i64::from(chrono::Datelike::num_days_from_ce(&d)))
}

/// The drawing of a chart: the closes as one polyline, each event as an x.
#[derive(Debug, Clone, PartialEq)]
pub struct Drawing {
    pub line: String,
    pub min: f64,
    pub max: f64,
    pub first: String,
    pub last: String,
    /// (x, event index) of the events inside the closes' dates.
    pub marks: Vec<(f64, usize)>,
}

/// `None` with fewer than two dated closes: nothing worth a line.
pub fn draw(chart: &IssuerChart) -> Option<Drawing> {
    let pts: Vec<(i64, f64)> = chart
        .closes
        .iter()
        .filter(|p| p.value.is_finite())
        .filter_map(|p| Some((day_number(&p.date)?, p.value)))
        .collect();
    if pts.len() < 2 {
        return None;
    }
    let (d0, d1) = (pts[0].0, pts[pts.len() - 1].0);
    let min = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min);
    let max = pts.iter().map(|p| p.1).fold(f64::MIN, f64::max);
    let span_d = ((d1 - d0) as f64).max(1.0);
    let span_v = (max - min).max(1e-9);
    let x = |d: i64| PAD_L + (d - d0) as f64 / span_d * (W - PAD_L - PAD_R);
    // A flat series sits in the middle, not on the axis.
    let flat = max - min < 1e-12;
    let y = |v: f64| {
        if flat {
            (PAD_T + H - PAD_B) / 2.0
        } else {
            PAD_T + (1.0 - (v - min) / span_v) * (H - PAD_T - PAD_B)
        }
    };
    let line = pts
        .iter()
        .map(|&(d, v)| format!("{:.1},{:.1}", x(d), y(v)))
        .collect::<Vec<_>>()
        .join(" ");
    let marks = chart
        .events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let d = day_number(&e.date)?;
            (d0..=d1).contains(&d).then(|| (x(d), i))
        })
        .collect();
    Some(Drawing {
        line,
        min,
        max,
        first: chart.closes[0].date.clone(),
        last: chart.closes[chart.closes.len() - 1].date.clone(),
        marks,
    })
}

/// Plot area bounds, for the axis and the markers.
pub const PLOT: (f64, f64, f64, f64) = (PAD_L, PAD_T, W - PAD_R, H - PAD_B);

#[cfg(test)]
mod tests {
    use super::*;

    fn p(date: &str, value: f64) -> ChartPoint {
        ChartPoint {
            date: date.into(),
            value,
        }
    }

    fn ev(date: &str) -> ChartEvent {
        ChartEvent {
            id: "x".into(),
            date: date.into(),
            kind: ChartEventKind::Filing,
            title: "8-K".into(),
            detail: "8-K".into(),
            model: None,
            probability: None,
        }
    }

    #[test]
    fn closes_span_the_plot_and_events_outside_are_left_out() {
        let chart = IssuerChart {
            instrument_id: Some("i".into()),
            ticker: Some("ACME".into()),
            closes: vec![
                p("2026-01-01", 10.0),
                p("2026-01-11", 20.0),
                p("2026-01-21", 15.0),
            ],
            events: vec![ev("2026-01-11"), ev("2025-12-01")],
        };
        let d = draw(&chart).unwrap();
        assert_eq!((d.min, d.max), (10.0, 20.0));
        assert!(
            d.line.starts_with(&format!("{:.1},{:.1}", PLOT.0, PLOT.3)),
            "{}",
            d.line
        );
        assert_eq!(d.marks.len(), 1, "the event before the first close is out");
        assert!(
            (d.marks[0].0 - (PLOT.0 + PLOT.2) / 2.0).abs() < 1e-9,
            "middle day, middle x"
        );
        assert_eq!(
            (d.first.as_str(), d.last.as_str()),
            ("2026-01-01", "2026-01-21")
        );
    }

    #[test]
    fn a_flat_series_is_drawn_in_the_middle() {
        let chart = IssuerChart {
            instrument_id: None,
            ticker: None,
            closes: vec![p("2026-01-01", 42.0), p("2026-01-02", 42.0)],
            events: vec![],
        };
        let mid = format!("{:.1}", (PLOT.1 + PLOT.3) / 2.0);
        assert!(draw(&chart).unwrap().line.ends_with(&mid));
    }

    #[test]
    fn one_close_draws_nothing() {
        let chart = IssuerChart {
            instrument_id: None,
            ticker: None,
            closes: vec![p("2026-01-01", 10.0), p("bad", 3.0)],
            events: vec![],
        };
        assert!(draw(&chart).is_none());
    }
}
