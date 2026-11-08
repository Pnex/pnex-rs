//! Fine price history (markets-research.md appendix C: D807–D809, D820,
//! D822): which partitions of a source to fetch, and the decoding of a
//! partition into typed rows. Pure code: no I/O.

use std::collections::HashSet;

use chrono::{Datelike, Days, Months, NaiveDate};

/// Decimal places of the prices and volumes kept (Binance publishes 8).
pub const SCALE: u32 = 8;
/// An `empty` partition this recent is asked again: a source can publish late.
pub const EMPTY_RETRY_DAYS: u64 = 7;

/// A source of history, fixed in the code (D822, R9): an import names it,
/// never a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistorySource {
    pub key: &'static str,
    pub base: &'static str,
    /// Bar length, the `interval` column.
    pub interval: &'static str,
    /// The table written, one per (source, shape) (D807).
    pub table: &'static str,
    pub license_note: &'static str,
}

pub const BINANCE_SPOT_1M: HistorySource = HistorySource {
    key: "binance_public",
    base: "https://data.binance.vision",
    interval: "1m",
    table: "binance_spot_klines_1m",
    license_note: "Binance public data files; terms of use not checked for redistribution \
                   or SaaS (Q-MK11): private to the org",
};

/// A partition of a source: one file, one transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Period {
    Month(NaiveDate),
    Day(NaiveDate),
}

impl Period {
    /// `month` or `day`, the `period` column of the registry.
    pub fn kind(self) -> &'static str {
        match self {
            Self::Month(_) => "month",
            Self::Day(_) => "day",
        }
    }

    pub fn start(self) -> NaiveDate {
        match self {
            Self::Month(d) | Self::Day(d) => d,
        }
    }

    /// First day after the period.
    pub fn end(self) -> NaiveDate {
        match self {
            Self::Month(d) => d + Months::new(1),
            Self::Day(d) => d + Days::new(1),
        }
    }

    pub fn of(kind: &str, start: NaiveDate) -> Option<Self> {
        match kind {
            "month" => Some(Self::Month(start.with_day(1)?)),
            "day" => Some(Self::Day(start)),
            _ => None,
        }
    }

    /// Whether a partition in `state` is never asked again.
    pub fn settled(self, state: &str, today: NaiveDate) -> bool {
        match state {
            "done" => true,
            "empty" => self.end() + Days::new(EMPTY_RETRY_DAYS) <= today,
            _ => false,
        }
    }
}

/// A symbol safe to splice into a path: `BTCUSDT`, 2 to 20 capitals or digits.
pub fn symbol_ok(s: &str) -> bool {
    (2..=20).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// Path of a Binance spot klines file, under the source's base.
pub fn binance_klines_path(symbol: &str, interval: &str, p: Period) -> String {
    match p {
        Period::Month(d) => format!(
            "data/spot/monthly/klines/{symbol}/{interval}/{symbol}-{interval}-{}.zip",
            d.format("%Y-%m")
        ),
        Period::Day(d) => format!(
            "data/spot/daily/klines/{symbol}/{interval}/{symbol}-{interval}-{}.zip",
            d.format("%Y-%m-%d")
        ),
    }
}

/// The partitions still to fetch from `from` (a month) up to yesterday:
/// closed months whole, the current month day by day. A closed month of
/// which a day is already known goes on by days, so no minute is loaded
/// twice. `known` holds the settled partitions.
pub fn plan(from: NaiveDate, today: NaiveDate, known: &HashSet<Period>) -> Vec<Period> {
    let Some(mut month) = from.with_day(1) else {
        return Vec::new();
    };
    let Some(current) = today.with_day(1) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while month <= current {
        let next = month + Months::new(1);
        let by_days = month == current
            || known
                .iter()
                .any(|p| matches!(p, Period::Day(d) if *d >= month && *d < next));
        if by_days {
            let mut d = month;
            while d < next.min(today) {
                if !known.contains(&Period::Day(d)) {
                    out.push(Period::Day(d));
                }
                d = d + Days::new(1);
            }
        } else if !known.contains(&Period::Month(month)) {
            out.push(Period::Month(month));
        }
        month = next;
    }
    out
}

/// The SHA-256 of a `.CHECKSUM` file (`<hex>  <name>`), lower case.
pub fn checksum_of(text: &str) -> Option<String> {
    let hex = text.split_whitespace().next()?;
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| hex.to_ascii_lowercase())
}

/// A decimal text as an integer at `scale` places. Digits past the scale
/// must be zeros: a price is never rounded.
pub fn scaled(s: &str, scale: u32) -> Option<i128> {
    let (neg, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    let digits = |t: &str| t.bytes().all(|b| b.is_ascii_digit());
    if !digits(int) || !digits(frac) || int.len() > 24 {
        return None;
    }
    let (kept, dropped) = frac.split_at(frac.len().min(scale as usize));
    if dropped.bytes().any(|b| b != b'0') {
        return None;
    }
    let mut v: i128 = if int.is_empty() { 0 } else { int.parse().ok()? };
    for i in 0..scale as usize {
        let d = kept.as_bytes().get(i).map_or(0, |b| i128::from(b - b'0'));
        v = v.checked_mul(10)?.checked_add(d)?;
    }
    Some(if neg { -v } else { v })
}

/// One bar of a klines file, prices and volumes at [`SCALE`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kline {
    /// Open time, µs since the epoch, UTC.
    pub open_us: i64,
    pub open: i128,
    pub high: i128,
    pub low: i128,
    pub close: i128,
    pub volume: i128,
    pub quote_volume: i128,
    pub trades: i64,
}

/// The 1-based line that could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError {
    pub line: usize,
}

/// A Binance klines CSV (the file inside the zip). A header line is
/// skipped; the time unit is read per value: Binance moved the spot files
/// from milliseconds to microseconds on 2025-01-01.
pub fn binance_klines(csv: &[u8]) -> Result<Vec<Kline>, DecodeError> {
    let text = std::str::from_utf8(csv).map_err(|_| DecodeError { line: 1 })?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        let bad = DecodeError { line: i + 1 };
        let Ok(open_time) = f[0].parse::<i64>() else {
            if i == 0 {
                continue;
            }
            return Err(bad);
        };
        if f.len() < 9 {
            return Err(bad);
        }
        let open_us = if open_time >= 1_000_000_000_000_000 {
            open_time
        } else {
            open_time.checked_mul(1000).ok_or(bad)?
        };
        let n = |k: usize| scaled(f[k], SCALE).ok_or(bad);
        out.push(Kline {
            open_us,
            open: n(1)?,
            high: n(2)?,
            low: n(3)?,
            close: n(4)?,
            volume: n(5)?,
            quote_volume: n(7)?,
            trades: f[8].parse().map_err(|_| bad)?,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn decimals_are_never_rounded() {
        assert_eq!(scaled("4261.48000000", 8), Some(426_148_000_000));
        assert_eq!(scaled("0.1", 2), Some(10));
        assert_eq!(scaled("7", 2), Some(700));
        assert_eq!(scaled("-1.50", 1), Some(-15));
        assert_eq!(scaled("1.005", 2), None, "a digit past the scale");
        assert_eq!(scaled("1.0050", 3), Some(1005));
        assert_eq!(scaled("", 2), None);
        assert_eq!(scaled("1e5", 2), None);
        assert_eq!(scaled("1.2.3", 2), None);
    }

    #[test]
    fn klines_in_milliseconds_and_microseconds() {
        // First minute of BTCUSDT on Binance, then a 2025 line in µs.
        let csv = b"open_time,open,high,low,close,volume,close_time,quote_volume,count,tbbv,tbqv,ignore\n\
1502942400000,4261.48000000,4261.48000000,4261.48000000,4261.48000000,1.77518300,1502942459999,7564.90685084,3,0.07518300,320.39085357,0\n\
1735689600000000,93576.00000000,93610.93000000,93537.50000000,93610.93000000,8.21827000,1735689659999999,768978.26748550,1613,4.65023000,435126.24934380,0\n";
        let k = binance_klines(csv).unwrap();
        assert_eq!(k.len(), 2);
        assert_eq!(k[0].open_us, 1_502_942_400_000_000);
        assert_eq!(k[0].close, 426_148_000_000);
        assert_eq!(k[0].trades, 3);
        assert_eq!(k[1].open_us, 1_735_689_600_000_000);
        assert_eq!(k[1].quote_volume, 76_897_826_748_550);
        assert_eq!(
            binance_klines(b"1,2,3\n"),
            Err(DecodeError { line: 1 }),
            "a short line"
        );
        assert_eq!(
            binance_klines(b"1502942400000,1,1,1,1,1,0,1,3\nx,1\n"),
            Err(DecodeError { line: 2 }),
            "only the first line may be a header"
        );
    }

    #[test]
    fn closed_months_whole_the_current_month_by_days() {
        let today = d("2026-03-04");
        let p = plan(d("2026-01-15"), today, &HashSet::new());
        assert_eq!(
            p,
            vec![
                Period::Month(d("2026-01-01")),
                Period::Month(d("2026-02-01")),
                Period::Day(d("2026-03-01")),
                Period::Day(d("2026-03-02")),
                Period::Day(d("2026-03-03")),
            ]
        );
        // March has closed: its days already loaded, the rest by days too.
        let known: HashSet<Period> = p.iter().copied().collect();
        let later = plan(d("2026-01-01"), d("2026-04-02"), &known);
        assert_eq!(later.first(), Some(&Period::Day(d("2026-03-04"))));
        assert_eq!(later.last(), Some(&Period::Day(d("2026-04-01"))));
        assert!(!later.contains(&Period::Month(d("2026-03-01"))));
        assert_eq!(later.len(), 28 + 1);
    }

    #[test]
    fn an_empty_partition_is_asked_again_for_a_week() {
        let p = Period::Day(d("2026-03-01"));
        assert!(!p.settled("empty", d("2026-03-05")));
        assert!(p.settled("empty", d("2026-03-09")));
        assert!(p.settled("done", d("2026-03-02")));
        assert!(!p.settled("failed", d("2030-01-01")));
        assert_eq!(
            Period::of("month", d("2026-03-17")),
            Some(Period::Month(d("2026-03-01")))
        );
    }

    #[test]
    fn paths_and_checksums() {
        assert_eq!(
            binance_klines_path("BTCUSDT", "1m", Period::Month(d("2017-08-01"))),
            "data/spot/monthly/klines/BTCUSDT/1m/BTCUSDT-1m-2017-08.zip"
        );
        assert_eq!(
            binance_klines_path("BTCUSDT", "1m", Period::Day(d("2026-11-07"))),
            "data/spot/daily/klines/BTCUSDT/1m/BTCUSDT-1m-2026-11-07.zip"
        );
        assert!(symbol_ok("BTCUSDT") && !symbol_ok("btc/../x") && !symbol_ok("B"));
        let hex = "A".repeat(64);
        assert_eq!(
            checksum_of(&format!("{hex}  BTCUSDT-1m-2017-08.zip\n")),
            Some("a".repeat(64))
        );
        assert_eq!(checksum_of("nope"), None);
    }
}
