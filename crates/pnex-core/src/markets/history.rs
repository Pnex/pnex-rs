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

/// A partition of a source: one file, one transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Period {
    /// A closed year in one file (HistData), from January 1.
    Year(NaiveDate),
    Month(NaiveDate),
    Day(NaiveDate),
}

impl Period {
    /// `month` or `day`, the `period` column of the registry.
    pub fn kind(self) -> &'static str {
        match self {
            Self::Year(_) => "year",
            Self::Month(_) => "month",
            Self::Day(_) => "day",
        }
    }

    pub fn start(self) -> NaiveDate {
        match self {
            Self::Year(d) | Self::Month(d) | Self::Day(d) => d,
        }
    }

    /// First day after the period.
    pub fn end(self) -> NaiveDate {
        match self {
            Self::Year(d) => d + Months::new(12),
            Self::Month(d) => d + Months::new(1),
            Self::Day(d) => d + Days::new(1),
        }
    }

    pub fn of(kind: &str, start: NaiveDate) -> Option<Self> {
        match kind {
            "year" => Some(Self::Year(start.with_day(1)?.with_month(1)?)),
            "month" => Some(Self::Month(start.with_day(1)?)),
            "day" => Some(Self::Day(start)),
            _ => None,
        }
    }

    /// Whether a partition in `state` is never asked again.
    pub fn settled(self, state: &str, today: NaiveDate) -> bool {
        match state {
            "done" => true,
            // A year file appears weeks after the year closes: asked for a year.
            "empty" if matches!(self, Self::Year(_)) => self.end() + Months::new(12) <= today,
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
    /// Absent when the source does not publish it (MT5).
    pub quote_volume: Option<i128>,
    pub trades: Option<i64>,
}

/// The 1-based line that could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError {
    pub line: usize,
}

/// The partitions still to fetch from `from` up to today, the HistData way:
/// closed years whole, the current year month by month (closed months).
/// A closed year of which a month is already known goes on by months.
pub fn plan_years(from: NaiveDate, today: NaiveDate, known: &HashSet<Period>) -> Vec<Period> {
    let (Some(mut year), Some(current)) = (
        from.with_day(1).and_then(|d| d.with_month(1)),
        today.with_day(1).and_then(|d| d.with_month(1)),
    ) else {
        return Vec::new();
    };
    let this_month = today.with_day(1).unwrap_or(today);
    let mut out = Vec::new();
    while year <= current {
        let next = year + Months::new(12);
        let by_months = year == current
            || known
                .iter()
                .any(|p| matches!(p, Period::Month(d) if *d >= year && *d < next));
        if by_months {
            let mut m = year;
            while m < next.min(this_month) {
                if !known.contains(&Period::Month(m)) {
                    out.push(Period::Month(m));
                }
                m = m + Months::new(1);
            }
        } else if !known.contains(&Period::Year(year)) {
            out.push(Period::Year(year));
        }
        year = next;
    }
    out
}

/// The days still to fetch from `from` up to yesterday, `known` left out.
pub fn plan_days(from: NaiveDate, today: NaiveDate, known: &HashSet<Period>) -> Vec<Period> {
    let mut out = Vec::new();
    let mut d = from;
    while d < today {
        if !known.contains(&Period::Day(d)) {
            out.push(Period::Day(d));
        }
        d = d + Days::new(1);
    }
    out
}

/// One quote of a tick file, prices at [`SCALE`].
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    /// µs since the epoch, UTC.
    pub ts_us: i64,
    pub bid: i128,
    pub ask: i128,
    /// Absent when the source has no volume per side (MT5).
    pub bid_volume: Option<f64>,
    pub ask_volume: Option<f64>,
}

/// Bars pushed by the PneX bridge for MetaTrader 5 (D816); the `source`
/// column says `mt5:<broker>`.
pub const MT5_OHLCV_1M: HistorySource = HistorySource {
    key: "mt5",
    base: "",
    interval: "1m",
    table: "mt5_ohlcv_1m",
    license_note: "Quotes of the org's own broker, pushed by its terminal: private to the org",
};

/// Ticks pushed by the PneX bridge for MetaTrader 5 (D816).
pub const MT5_TICKS: HistorySource = HistorySource {
    key: "mt5",
    base: "",
    interval: "tick",
    table: "mt5_ticks",
    license_note: "Quotes of the org's own broker, pushed by its terminal: private to the org",
};

/// One call of the PneX bridge Expert Advisor for MetaTrader 5 (D816):
/// closed 1-minute bars and ticks of a broker, times already in UTC.
#[derive(Debug, Clone, PartialEq)]
pub struct Mt5Batch {
    /// `mt5:<broker>`, the `source` column (D824).
    pub source: String,
    pub bars: Vec<(String, Kline)>,
    pub ticks: Vec<(String, Tick)>,
}

/// Bars plus ticks of one call; the 1 MiB body holds far fewer.
pub const MT5_ROWS_MAX: usize = 50_000;

/// A broker symbol, kept as a column only: `EURUSD`, `US500.cash`, `XAUUSD#`.
pub fn mt5_symbol_ok(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._#-".contains(&b))
}

/// Reads one call: `{"v": 1, "broker": "…", "bars": [{"symbol", "time" (s),
/// "open", "high", "low", "close", "volume"}], "ticks": [{"symbol",
/// "time_msc" (ms), "bid", "ask"}]}`. Prices are decimal texts (numbers
/// pass too); the error is a machine token.
pub fn mt5_payload(body: &serde_json::Value) -> Result<Mt5Batch, &'static str> {
    use serde_json::Value;
    if body.get("v").and_then(Value::as_i64) != Some(1) {
        return Err("version");
    }
    let broker: String = body
        .get("broker")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    let broker = if broker.is_empty() {
        "unknown".into()
    } else {
        broker
    };
    let list = |key: &str| match body.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => Ok(a.clone()),
        Some(_) => Err("shape"),
    };
    let (bars_in, ticks_in) = (list("bars")?, list("ticks")?);
    if bars_in.len() + ticks_in.len() > MT5_ROWS_MAX {
        return Err("too_many");
    }
    let symbol = |r: &Value| {
        r.get("symbol")
            .and_then(Value::as_str)
            .filter(|s| mt5_symbol_ok(s))
            .map(str::to_string)
            .ok_or("symbol")
    };
    let price = |r: &Value, k: &str| {
        let text = match r.get(k) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => return Err("price"),
        };
        scaled(text.trim(), SCALE)
            .filter(|v| *v >= 0)
            .ok_or("price")
    };
    let int = |r: &Value, k: &str| r.get(k).and_then(Value::as_i64).ok_or("time");
    let mut bars = Vec::with_capacity(bars_in.len());
    for r in &bars_in {
        let open_us = int(r, "time")?.checked_mul(1_000_000).ok_or("time")?;
        let volume = match r.get("volume") {
            None | Some(Value::Null) => 0,
            Some(_) => price(r, "volume")?,
        };
        bars.push((
            symbol(r)?,
            Kline {
                open_us,
                open: price(r, "open")?,
                high: price(r, "high")?,
                low: price(r, "low")?,
                close: price(r, "close")?,
                volume,
                quote_volume: None,
                trades: None,
            },
        ));
    }
    let mut ticks = Vec::with_capacity(ticks_in.len());
    for r in &ticks_in {
        ticks.push((
            symbol(r)?,
            Tick {
                ts_us: int(r, "time_msc")?.checked_mul(1000).ok_or("time")?,
                bid: price(r, "bid")?,
                ask: price(r, "ask")?,
                bid_volume: None,
                ask_volume: None,
            },
        ));
    }
    Ok(Mt5Batch {
        source: format!("mt5:{broker}"),
        bars,
        ticks,
    })
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
    fn mt5_calls_are_read_strictly() {
        let body = serde_json::json!({
            "v": 1,
            "broker": "Demo Broker/Live",
            "bars": [{"symbol": "EURUSD", "time": 1_735_689_600, "open": "1.03456",
                      "high": "1.03470", "low": "1.03400", "close": 1.0345, "volume": 120}],
            "ticks": [{"symbol": "US500.cash", "time_msc": 1_735_689_600_123_i64,
                       "bid": "5881.25", "ask": "5881.75"}]
        });
        let b = mt5_payload(&body).unwrap();
        assert_eq!(b.source, "mt5:Demo_Broker_Live");
        assert_eq!(b.bars[0].0, "EURUSD");
        assert_eq!(b.bars[0].1.open_us, 1_735_689_600_000_000);
        assert_eq!(b.bars[0].1.close, 103_450_000);
        assert_eq!(b.bars[0].1.volume, 12_000_000_000);
        assert_eq!(b.bars[0].1.trades, None);
        assert_eq!(b.ticks[0].1.ts_us, 1_735_689_600_123_000);
        assert_eq!(b.ticks[0].1.ask, 588_175_000_000);
        for (bad, token) in [
            (serde_json::json!({"v": 2}), "version"),
            (serde_json::json!({"v": 1, "bars": {}}), "shape"),
            (
                serde_json::json!({"v": 1, "bars": [{"symbol": "../x", "time": 1}]}),
                "symbol",
            ),
            (
                serde_json::json!({"v": 1, "ticks": [{"symbol": "X", "time_msc": 1, "bid": "-1", "ask": "1"}]}),
                "price",
            ),
        ] {
            assert_eq!(mt5_payload(&bad), Err(token));
        }
    }

    #[test]
    fn years_go_on_by_months_once_begun() {
        let known: HashSet<Period> = [Period::Year(d("2024-01-01"))].into();
        assert_eq!(
            plan_years(d("2023-06-01"), d("2026-03-10"), &known),
            vec![
                Period::Year(d("2023-01-01")),
                Period::Year(d("2025-01-01")),
                Period::Month(d("2026-01-01")),
                Period::Month(d("2026-02-01")),
            ]
        );
        let by_month: HashSet<Period> = [Period::Month(d("2025-11-01"))].into();
        assert_eq!(
            plan_years(d("2025-01-01"), d("2026-01-05"), &by_month).len(),
            11,
            "a year begun by months goes on by months, never whole again"
        );
        assert!(!Period::Year(d("2025-01-01")).settled("empty", d("2026-02-01")));
        assert!(Period::Year(d("2024-01-01")).settled("empty", d("2026-01-01")));
    }
}
