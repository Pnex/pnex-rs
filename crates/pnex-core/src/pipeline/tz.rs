//! Wall clocks in their IANA zone (pipeline-engine.md D939): a time written
//! without an offset is read in its zone, daylight saving included, never
//! with a fixed offset. Shared by the `read` step and the SQL functions of
//! the pipeline worker.

use chrono::{Duration, LocalResult, NaiveDateTime, Offset, TimeZone};
pub use chrono_tz::Tz;
use std::str::FromStr;

/// An IANA zone (`America/New_York`, `UTC`).
pub fn zone(name: &str) -> Result<Tz, String> {
    Tz::from_str(name.trim()).map_err(|_| format!("unknown time zone: {name}"))
}

/// The UTC instant of a wall clock in `tz`. An hour repeated when clocks go
/// back is its first occurrence; a time skipped when they go forward is read
/// with the offset in force just before.
pub fn wall_to_utc(wall: NaiveDateTime, tz: &Tz) -> NaiveDateTime {
    match tz.from_local_datetime(&wall) {
        LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => t.naive_utc(),
        LocalResult::None => {
            let before = tz
                .from_local_datetime(&(wall - Duration::hours(3)))
                .earliest()
                .map(|t| t.offset().fix().local_minus_utc())
                .unwrap_or(0);
            wall - Duration::seconds(i64::from(before))
        }
    }
}

/// The wall clock in `tz` of a UTC instant.
pub fn utc_to_wall(utc: NaiveDateTime, tz: &Tz) -> NaiveDateTime {
    tz.from_utc_datetime(&utc).naive_local()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    /// The case of the minute files that were read 5 h behind New York all
    /// year (2026-11-12): summer is 4 h behind, under both rules of the US.
    #[test]
    fn new_york_wall_clock_follows_daylight_saving() {
        let ny = zone("America/New_York").unwrap();
        let utc = |s: &str| wall_to_utc(at(s), &ny);
        // US jobs report, 08:30 in New York.
        assert_eq!(utc("2015-07-02 08:30"), at("2015-07-02 12:30"));
        assert_eq!(utc("2015-01-09 08:30"), at("2015-01-09 13:30"));
        // Last minute of a summer Friday.
        assert_eq!(utc("2015-07-10 16:59"), at("2015-07-10 20:59"));
        // Before 2007 summer time began in April, since then in March.
        assert_eq!(utc("2005-04-01 08:30"), at("2005-04-01 13:30"));
        assert_eq!(utc("2005-04-04 08:30"), at("2005-04-04 12:30"));
        assert_eq!(utc("2007-03-12 08:30"), at("2007-03-12 12:30"));
    }

    #[test]
    fn repeated_and_skipped_hours_have_one_reading() {
        let ny = zone("America/New_York").unwrap();
        // 01:30 happens twice on 2015-11-01: the first one, still summer time.
        assert_eq!(
            wall_to_utc(at("2015-11-01 01:30"), &ny),
            at("2015-11-01 05:30")
        );
        // 02:30 never happens on 2015-03-08: read with winter time.
        assert_eq!(
            wall_to_utc(at("2015-03-08 02:30"), &ny),
            at("2015-03-08 07:30")
        );
        assert_eq!(
            utc_to_wall(at("2015-07-02 12:30"), &ny),
            at("2015-07-02 08:30")
        );
        assert!(zone("EST+5").is_err());
    }
}
