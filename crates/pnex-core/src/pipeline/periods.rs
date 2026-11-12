//! Periods of an `enumerate` axis (pipeline-engine.md D936): the years,
//! months, days or hours from a start to today, each with the parts an
//! address template needs.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};

use super::doc::Period;

/// Periods one axis may produce.
pub const PERIODS_MAX: usize = 200_000;

/// Start of the first period, at `now`: `YYYY`, `YYYY-MM`, `YYYY-MM-DD` or
/// `YYYY-MM-DDTHH`, or relative: `this_year`, `this_month`, `today`, or a
/// count back (`-12m`, `-7d`, `-2y`, `-6h`). A coarser start than the
/// period is its first instant.
pub fn parse_start_at(period: Period, s: &str, now: NaiveDateTime) -> Option<NaiveDateTime> {
    let s = s.trim();
    let midnight = |d: NaiveDate| d.and_hms_opt(0, 0, 0);
    match s {
        "this_year" => return midnight(NaiveDate::from_ymd_opt(now.year(), 1, 1)?),
        "this_month" => return midnight(NaiveDate::from_ymd_opt(now.year(), now.month(), 1)?),
        "today" => return midnight(now.date()),
        _ => {}
    }
    if let Some(back) = s.strip_prefix('-') {
        let (n, unit) = back.split_at(back.find(|c: char| !c.is_ascii_digit())?);
        let n: u32 = n.parse().ok()?;
        return match unit {
            "y" => now.checked_sub_months(chrono::Months::new(n.checked_mul(12)?)),
            "m" => now.checked_sub_months(chrono::Months::new(n)),
            "d" => now.checked_sub_signed(Duration::days(i64::from(n))),
            "h" => now.checked_sub_signed(Duration::hours(i64::from(n))),
            _ => None,
        };
    }
    parse_start(period, s)
}

/// Start of the first period from `YYYY`, `YYYY-MM`, `YYYY-MM-DD` or
/// `YYYY-MM-DDTHH`; a coarser start than the period is its first instant.
pub fn parse_start(_period: Period, s: &str) -> Option<NaiveDateTime> {
    let s = s.trim();
    let date = |y: &str, m: &str, d: &str| {
        NaiveDate::from_ymd_opt(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?)
    };
    let (day, hour) = match s.len() {
        4 => (date(s, "1", "1")?, 0),
        7 => (date(&s[..4], &s[5..7], "1")?, 0),
        10 => (date(&s[..4], &s[5..7], &s[8..10])?, 0),
        13 if &s[10..11] == "T" => (date(&s[..4], &s[5..7], &s[8..10])?, s[11..13].parse().ok()?),
        _ => return None,
    };
    if !s.is_char_boundary(4) || (s.len() > 4 && &s[4..5] != "-") {
        return None;
    }
    day.and_hms_opt(hour, 0, 0)
}

fn floor(p: Period, t: NaiveDateTime) -> NaiveDateTime {
    let d = t.date();
    let day = match p {
        Period::Year => NaiveDate::from_ymd_opt(d.year(), 1, 1),
        Period::Month => NaiveDate::from_ymd_opt(d.year(), d.month(), 1),
        Period::Day | Period::Hour => Some(d),
    }
    .unwrap_or(d);
    let hour = if p == Period::Hour { t.hour() } else { 0 };
    day.and_time(NaiveTime::from_hms_opt(hour, 0, 0).unwrap_or_default())
}

fn next(p: Period, t: NaiveDateTime) -> Option<NaiveDateTime> {
    let d = t.date();
    Some(match p {
        Period::Year => NaiveDate::from_ymd_opt(d.year() + 1, 1, 1)?.and_hms_opt(0, 0, 0)?,
        Period::Month => {
            let (y, m) = if d.month() == 12 {
                (d.year() + 1, 1)
            } else {
                (d.year(), d.month() + 1)
            };
            NaiveDate::from_ymd_opt(y, m, 1)?.and_hms_opt(0, 0, 0)?
        }
        Period::Day => t + Duration::days(1),
        Period::Hour => t + Duration::hours(1),
    })
}

/// One period: its label (the row value) and its parts, named after the
/// axis: `<as>_y`, `<as>_m` (01–12), `<as>_m0` (00–11), `<as>_d`, `<as>_h`,
/// `<as>_start` and `<as>_end` (RFC 3339, end excluded).
pub fn parts(p: Period, start: NaiveDateTime, axis: &str) -> Vec<(String, String)> {
    let end = next(p, start).unwrap_or(start);
    let label = match p {
        Period::Year => start.format("%Y"),
        Period::Month => start.format("%Y-%m"),
        Period::Day => start.format("%Y-%m-%d"),
        Period::Hour => start.format("%Y-%m-%dT%H"),
    }
    .to_string();
    let mut out = vec![
        (axis.to_string(), label),
        (format!("{axis}_y"), start.format("%Y").to_string()),
        (
            format!("{axis}_start"),
            format!("{}Z", start.format("%Y-%m-%dT%H:%M:%S")),
        ),
        (
            format!("{axis}_end"),
            format!("{}Z", end.format("%Y-%m-%dT%H:%M:%S")),
        ),
    ];
    if p != Period::Year {
        out.push((format!("{axis}_m"), start.format("%m").to_string()));
        out.push((format!("{axis}_m0"), format!("{:02}", start.month0())));
    }
    if matches!(p, Period::Day | Period::Hour) {
        out.push((format!("{axis}_d"), start.format("%d").to_string()));
    }
    if p == Period::Hour {
        out.push((format!("{axis}_h"), start.format("%H").to_string()));
    }
    out
}

/// The starts of the periods from `from` to `end` (excluded); only those
/// over by `end` when `closed_only`. Capped at [`PERIODS_MAX`].
pub fn starts(
    p: Period,
    from: NaiveDateTime,
    end: NaiveDateTime,
    closed_only: bool,
) -> Vec<NaiveDateTime> {
    let mut t = floor(p, from);
    let mut out = Vec::new();
    while out.len() < PERIODS_MAX {
        let Some(n) = next(p, t) else { break };
        let keep = if closed_only { n <= end } else { t < end };
        if !keep {
            break;
        }
        out.push(t);
        t = n;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn closed_years_and_months() {
        let now = at("2026-11-12 10:00");
        let years = starts(
            Period::Year,
            parse_start(Period::Year, "2023").unwrap(),
            now,
            true,
        );
        assert_eq!(years.len(), 3);
        let months = starts(
            Period::Month,
            parse_start(Period::Month, "2026-09").unwrap(),
            now,
            false,
        );
        assert_eq!(months.len(), 3);
        assert_eq!(
            starts(Period::Month, at("2026-11-01 00:00"), now, true),
            Vec::new()
        );
    }

    #[test]
    fn parts_of_an_hour_and_a_month() {
        let p = parts(Period::Hour, at("2024-03-05 07:00"), "h");
        let get = |k: &str| p.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("h").as_deref(), Some("2024-03-05T07"));
        assert_eq!(get("h_m0").as_deref(), Some("02"));
        assert_eq!(get("h_end").as_deref(), Some("2024-03-05T08:00:00Z"));
        let m = parts(Period::Month, at("2024-12-01 00:00"), "m");
        assert!(m.contains(&("m_end".into(), "2025-01-01T00:00:00Z".into())));
    }

    #[test]
    fn relative_starts_are_read_at_now() {
        let now = at("2026-11-12 10:00");
        let p = Period::Day;
        assert_eq!(
            parse_start_at(p, "this_month", now),
            Some(at("2026-11-01 00:00"))
        );
        assert_eq!(
            parse_start_at(p, "this_year", now),
            Some(at("2026-01-01 00:00"))
        );
        assert_eq!(parse_start_at(p, "-12m", now), Some(at("2025-11-12 10:00")));
        assert_eq!(parse_start_at(p, "-7d", now), Some(at("2026-11-05 10:00")));
        assert_eq!(parse_start_at(p, "2024", now), Some(at("2024-01-01 00:00")));
        assert_eq!(parse_start_at(p, "-3w", now), None);
    }

    #[test]
    fn starts_are_parsed_strictly() {
        assert!(parse_start(Period::Day, "2024-02-30").is_none());
        assert!(parse_start(Period::Day, "2024/02/01").is_none());
        assert_eq!(
            parse_start(Period::Hour, "2024-02-01T05"),
            Some(at("2024-02-01 05:00"))
        );
    }
}
