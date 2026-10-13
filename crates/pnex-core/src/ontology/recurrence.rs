//! `recurrence` property values (ontology.md annex B, maintenance plans):
//! a day-granular series "every N days/weeks/months/years from a start
//! date", fixed (from the planned dates) or floating (from the last
//! completion). Shared by the backend materializer and the UI preview.
//!
//! shortcut: no RRULE (BYDAY, BYSETPOS, time of day, time zones); upgrade
//! to the `rrule` crate when a plan needs more than a period (study notes:
//! `research/gmao-calendar/moteur-recurrence.md`).

use chrono::{Days, Months, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const EVERY_MAX: u32 = 999;
pub const DAYS_MAX: u32 = 366;
/// Hard cap of one enumeration, whatever the caller asks.
pub const DATES_MAX: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Day,
    Week,
    Month,
    Year,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recurrence {
    /// First planned date, `YYYY-MM-DD`.
    pub start: NaiveDate,
    pub every: u32,
    pub unit: Unit,
    /// Last date an occurrence may start on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<NaiveDate>,
    /// Next date counted from the last completion instead of the plan.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub floating: bool,
    /// Length of one occurrence in days (1 = the day itself).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub days: u32,
    /// Days before the planned start when an occurrence becomes due
    /// (to prepare: parts, permits, staff).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_days: Option<u32>,
}

fn one() -> u32 {
    1
}

fn is_one(d: &u32) -> bool {
    *d == 1
}

impl Recurrence {
    /// Parses and bounds a property value; `None` = invalid.
    pub fn parse(v: &Value) -> Option<Self> {
        let r: Self = serde_json::from_value(v.clone()).ok()?;
        let ok = (1..=EVERY_MAX).contains(&r.every)
            && (1..=DAYS_MAX).contains(&r.days)
            && r.lead_days.is_none_or(|l| l <= DAYS_MAX)
            && r.until.is_none_or(|u| u >= r.start);
        ok.then_some(r)
    }

    /// Normalized property value.
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// The `n`-th date of the series, always computed from `start` so that
    /// a clamped month end (Jan 31 → Feb 28) does not drift.
    fn nth(&self, n: u32) -> Option<NaiveDate> {
        let k = n.checked_mul(self.every)?;
        match self.unit {
            Unit::Day => self.start.checked_add_days(Days::new(k.into())),
            Unit::Week => self.start.checked_add_days(Days::new(u64::from(k) * 7)),
            Unit::Month => self.start.checked_add_months(Months::new(k)),
            Unit::Year => self
                .start
                .checked_add_months(Months::new(k.checked_mul(12)?)),
        }
    }

    /// First index whose date may be `>= from` (a lower bound).
    fn first_index(&self, from: NaiveDate) -> u32 {
        if from <= self.start {
            return 0;
        }
        let days = (from - self.start).num_days();
        let approx_unit_days = match self.unit {
            Unit::Day => 1,
            Unit::Week => 7,
            Unit::Month => 31,
            Unit::Year => 366,
        };
        u32::try_from(days / (approx_unit_days * i64::from(self.every))).unwrap_or(u32::MAX)
    }

    /// Series dates in `[from, to]`, at most `max` (capped by [`DATES_MAX`]).
    pub fn dates(&self, from: NaiveDate, to: NaiveDate, max: usize) -> Vec<NaiveDate> {
        let mut out = Vec::new();
        let mut n = self.first_index(from);
        while out.len() < max.min(DATES_MAX) {
            let Some(d) = self.nth(n) else { break };
            if d > to || self.until.is_some_and(|u| d > u) {
                break;
            }
            if d >= from {
                out.push(d);
            }
            n = n.saturating_add(1);
            if n == u32::MAX {
                break;
            }
        }
        out
    }

    /// The series restarted one period after `done` (floating mode).
    pub fn after_completion(&self, done: NaiveDate) -> Option<Self> {
        let next = Self {
            start: done,
            ..self.clone()
        }
        .nth(1)?;
        Some(Self {
            start: next,
            ..self.clone()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn parse_bounds_and_normalizes() {
        let r = Recurrence::parse(&json!({"start": "2026-01-31", "every": 1, "unit": "month"}))
            .unwrap();
        assert_eq!(
            r.to_value(),
            json!({"start": "2026-01-31", "every": 1, "unit": "month"})
        );
        for bad in [
            json!({"start": "2026-01-31", "every": 0, "unit": "month"}),
            json!({"start": "2026-01-31", "every": 1000, "unit": "day"}),
            json!({"start": "2026-02-30", "every": 1, "unit": "day"}),
            json!({"start": "2026-01-31", "every": 1, "unit": "hour"}),
            json!({"start": "2026-01-31", "every": 1, "unit": "day", "until": "2026-01-01"}),
            json!({"start": "2026-01-31", "every": 1, "unit": "day", "days": 0}),
            json!({"start": "2026-01-31", "every": 1, "unit": "day", "x": 1}),
            json!({"start": "2026-01-31", "every": 1, "unit": "day", "lead_days": 400}),
            json!("monthly"),
        ] {
            assert!(Recurrence::parse(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn month_ends_do_not_drift() {
        let r = Recurrence::parse(&json!({"start": "2026-01-31", "every": 1, "unit": "month"}))
            .unwrap();
        assert_eq!(
            r.dates(d("2026-01-01"), d("2026-04-30"), 10),
            [
                d("2026-01-31"),
                d("2026-02-28"),
                d("2026-03-31"),
                d("2026-04-30")
            ]
        );
    }

    #[test]
    fn window_until_and_caps() {
        let r = Recurrence::parse(&json!({
            "start": "2020-01-01", "every": 3, "unit": "month", "until": "2026-12-31"
        }))
        .unwrap();
        assert_eq!(
            r.dates(d("2026-05-15"), d("2030-01-01"), 10),
            [d("2026-07-01"), d("2026-10-01")]
        );
        let daily =
            Recurrence::parse(&json!({"start": "2000-01-01", "every": 1, "unit": "day"})).unwrap();
        assert_eq!(
            daily.dates(d("2026-01-01"), d("2099-01-01"), 5000).len(),
            DATES_MAX
        );
        assert_eq!(daily.dates(d("2026-01-01"), d("2026-01-07"), 3).len(), 3);
        let weekly =
            Recurrence::parse(&json!({"start": "2026-01-05", "every": 2, "unit": "week"})).unwrap();
        assert_eq!(
            weekly.dates(d("2026-01-06"), d("2026-02-01"), 10),
            [d("2026-01-19"), d("2026-02-02")][..1]
        );
    }

    #[test]
    fn floating_restarts_from_the_completion() {
        let r = Recurrence::parse(&json!({
            "start": "2026-01-01", "every": 3, "unit": "month", "floating": true
        }))
        .unwrap();
        let next = r.after_completion(d("2026-04-11")).unwrap();
        assert_eq!(next.start, d("2026-07-11"));
        assert_eq!(
            next.dates(d("2026-01-01"), d("2026-12-31"), 10),
            [d("2026-07-11"), d("2026-10-11")]
        );
    }
}
