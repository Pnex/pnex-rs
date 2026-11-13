//! Primary market sources (markets-research.md D540, D541): what SEC EDGAR
//! and FRED answer, turned into import records, and the observations those
//! records hold. Pure code on hostile input: a shape it does not recognise
//! gives `None`, never a partial guess.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod accounts;
pub mod chart;
pub mod decisions;
pub mod exhibit21;
pub mod history;
pub mod moves;
pub mod prediction;

/// Field of a per-object record holding the value it was queried with.
pub const QUERIED: &str = "@queried";

fn is_date(s: &str) -> bool {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

/// Where the documents of the filings may be read from.
pub const ESEF_BASE: &str = "https://filings.xbrl.org";
pub const SEC_ARCHIVE: &str = "https://www.sec.gov/Archives/edgar/data/";

/// Where a mapping's observations belong: the live object of this type whose
/// unique property holds the value the record was queried with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ObservationTarget {
    #[serde(rename = "type")]
    pub type_key: String,
    pub property: String,
}

/// One value of a dated metric, as a source gave it (D541).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PlannedObservation {
    pub key: String,
    pub metric: String,
    /// Empty when the source gives none.
    #[serde(default)]
    pub unit: String,
    /// Equal to `period_end` for a value at a date.
    pub period_start: String,
    pub period_end: String,
    /// The day the source says it published this value; `None` = unknown,
    /// the value counts as known from the moment it is stored.
    #[serde(default)]
    pub known_on: Option<String>,
    pub value: f64,
    #[serde(default)]
    pub source: Option<String>,
}

/// The observations of a run's records. A later filing repeating a value
/// unchanged says nothing new: only the first report and real restatements
/// remain, which is what a reading "as known at a date" needs.
pub fn observations(rows: &[Value]) -> Vec<PlannedObservation> {
    first_reports(rows.iter().filter_map(observation).collect())
}

/// One record read as an observation: columns `@queried`, `metric`, `unit`,
/// `start`, `end`, `known_on`, `value`, `source`. A table gives every value as
/// text: `value` may be a number or its text.
pub fn observation(r: &Value) -> Option<PlannedObservation> {
    let text = |k: &str| r.get(k).and_then(Value::as_str).map(str::to_string);
    let end = text("end").filter(|d| is_date(d))?;
    let value = match r.get("value")? {
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        v => v.as_f64()?,
    };
    Some(PlannedObservation {
        // `queried` is the column a pipeline writes, `@queried` an import's own.
        key: text(QUERIED).or_else(|| text("queried"))?,
        metric: text("metric").filter(|m| !m.is_empty())?,
        unit: text("unit").unwrap_or_default(),
        period_start: text("start").filter(|d| is_date(d)).unwrap_or(end.clone()),
        period_end: end,
        known_on: text("known_on").filter(|d| is_date(d)),
        value: Some(value).filter(|v| v.is_finite())?,
        source: text("source"),
    })
}

/// Keeps the first report of each value and its real restatements.
pub fn first_reports(mut out: Vec<PlannedObservation>) -> Vec<PlannedObservation> {
    let same_period = |a: &PlannedObservation, b: &PlannedObservation| {
        (&a.key, &a.metric, &a.unit, &a.period_start, &a.period_end)
            == (&b.key, &b.metric, &b.unit, &b.period_start, &b.period_end)
    };
    out.sort_by(|a, b| {
        (
            &a.key,
            &a.metric,
            &a.unit,
            &a.period_start,
            &a.period_end,
            &a.known_on,
        )
            .cmp(&(
                &b.key,
                &b.metric,
                &b.unit,
                &b.period_start,
                &b.period_end,
                &b.known_on,
            ))
    });
    let mut kept: Vec<PlannedObservation> = Vec::with_capacity(out.len());
    for o in out {
        match kept.last() {
            Some(p) if same_period(p, &o) && p.value == o.value => {}
            _ => kept.push(o),
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(end: &str, filed: &str, value: f64) -> Value {
        json!({"@queried": "k", "metric": "Assets", "unit": "USD", "end": end,
               "value": value, "known_on": filed})
    }

    #[test]
    fn a_repeated_value_is_dropped_and_a_restatement_kept() {
        let got = observations(&[
            row("2025-12-31", "2027-02-01", 10.0),
            row("2025-12-31", "2026-02-01", 10.0),
            row("2025-12-31", "2026-05-01", 12.0),
        ]);
        let seen: Vec<(Option<&str>, f64)> = got
            .iter()
            .map(|o| (o.known_on.as_deref(), o.value))
            .collect();
        assert_eq!(
            seen,
            vec![
                (Some("2026-02-01"), 10.0),
                (Some("2026-05-01"), 12.0),
                (Some("2027-02-01"), 10.0)
            ],
            "the value that came back after a restatement is a change too"
        );
        assert_eq!(got[0].period_start, "2025-12-31", "a value at a date");
    }

    #[test]
    fn a_record_without_a_usable_date_or_value_is_no_observation() {
        let bad = [
            json!({"@queried": "k", "metric": "m", "end": "soon", "value": 1.0}),
            json!({"@queried": "k", "metric": "m", "end": "2026-01-01"}),
            json!({"metric": "m", "end": "2026-01-01", "value": 1.0}),
        ];
        assert!(observations(&bad).is_empty());
    }
}
