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

/// Path of a Binance spot klines file, under the source's base.
pub fn binance_klines_path(symbol: &str, interval: &str, p: Period) -> String {
    match p {
        // Binance has no yearly file: never planned for it.
        Period::Year(d) | Period::Month(d) => format!(
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
    /// Absent when the source does not publish it (MT5).
    pub quote_volume: Option<i128>,
    pub trades: Option<i64>,
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
            quote_volume: Some(n(7)?),
            trades: Some(f[8].parse().map_err(|_| bad)?),
        });
    }
    Ok(out)
}

pub const BINANCE_SPOT_TRADES: HistorySource = HistorySource {
    key: "binance_public_trades",
    base: "https://data.binance.vision",
    interval: "trade",
    table: "binance_spot_trades",
    license_note: "Binance public data files; terms of use not checked for redistribution \
                   or SaaS (Q-MK11): private to the org",
};

/// Path of a Binance spot trades file of one day (D821: by day, a month
/// would pass the unzip cap).
pub fn binance_trades_path(symbol: &str, day: NaiveDate) -> String {
    format!(
        "data/spot/daily/trades/{symbol}/{symbol}-trades-{}.zip",
        day.format("%Y-%m-%d")
    )
}

/// One trade of a trades file, price and quantity at [`SCALE`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trade {
    pub trade_id: i64,
    /// µs since the epoch, UTC.
    pub ts_us: i64,
    pub price: i128,
    pub qty: i128,
    /// `buy` when the taker bought, `sell` when the buyer was the maker.
    pub side: &'static str,
}

/// A Binance trades CSV: id, price, qty, quote qty, time, buyer is maker,
/// best match. Same header and time unit rules as [`binance_klines`].
pub fn binance_trades(csv: &[u8]) -> Result<Vec<Trade>, DecodeError> {
    let text = std::str::from_utf8(csv).map_err(|_| DecodeError { line: 1 })?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        let bad = DecodeError { line: i + 1 };
        let Ok(trade_id) = f[0].parse::<i64>() else {
            if i == 0 {
                continue;
            }
            return Err(bad);
        };
        if f.len() < 6 {
            return Err(bad);
        }
        let time: i64 = f[4].parse().map_err(|_| bad)?;
        let ts_us = if time >= 1_000_000_000_000_000 {
            time
        } else {
            time.checked_mul(1000).ok_or(bad)?
        };
        let side = match f[5].to_ascii_lowercase().as_str() {
            "true" => "sell",
            "false" => "buy",
            _ => return Err(bad),
        };
        out.push(Trade {
            trade_id,
            ts_us,
            price: scaled(f[1], SCALE).ok_or(bad)?,
            qty: scaled(f[2], SCALE).ok_or(bad)?,
            side,
        });
    }
    Ok(out)
}

pub const HISTDATA_M1: HistorySource = HistorySource {
    key: "histdata",
    base: "https://www.histdata.com",
    interval: "1m",
    table: "histdata_ohlcv_1m",
    license_note: "HistData.com free files: bid prices, no license or terms published, \
                   source unstated, no warranty: private to the org, never redistributed",
};

/// The HistData page of a pair's MetaTrader 1-minute file for a year or a
/// month; the file is the answer to the form of that page.
pub fn histdata_page_path(pair: &str, p: Period) -> String {
    let base = format!(
        "download-free-forex-historical-data/?/metatrader/1-minute-bar-quotes/{}",
        pair.to_ascii_lowercase()
    );
    match p {
        Period::Year(d) => format!("{base}/{}", d.year()),
        Period::Month(d) | Period::Day(d) => format!("{base}/{}/{}", d.year(), d.month()),
    }
}

/// The hidden fields of the page's download form (`id="file_down"`), in
/// order, to send back as the page does. `None` when the page has no file.
pub fn histdata_form(html: &str) -> Option<Vec<(String, String)>> {
    let start = html.find("id=\"file_down\"")?;
    let end = start + html[start..].find("</form>")?;
    let attr = |tag: &str, name: &str| {
        let at = tag.find(&format!("{name}=\""))? + name.len() + 2;
        Some(tag[at..at + tag[at..].find('"')?].to_string())
    };
    let mut out = Vec::new();
    let mut rest = &html[start..end];
    while let Some(i) = rest.find("<input") {
        let tag = &rest[i..i + rest[i..].find('>')?];
        if let (Some(n), Some(v)) = (attr(tag, "name"), attr(tag, "value")) {
            let safe = |t: &str| {
                t.len() <= 64 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            };
            if !safe(&n) || !safe(&v) {
                return None;
            }
            out.push((n, v));
        }
        rest = &rest[i + tag.len()..];
    }
    (out.iter().any(|(n, _)| n == "tk")).then_some(out)
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

/// A HistData MetaTrader 1-minute CSV: `YYYY.MM.DD,HH:MM,open,high,low,
/// close,volume`, bid prices, EST without daylight saving (UTC-5), turned
/// to UTC. HistData publishes a volume of 0: kept as published.
pub fn histdata_mt_m1(csv: &[u8]) -> Result<Vec<Kline>, DecodeError> {
    let text = std::str::from_utf8(csv).map_err(|_| DecodeError { line: 1 })?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let bad = DecodeError { line: i + 1 };
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        if f.len() < 7 {
            return Err(bad);
        }
        let at =
            chrono::NaiveDateTime::parse_from_str(&format!("{} {}", f[0], f[1]), "%Y.%m.%d %H:%M")
                .map_err(|_| bad)?;
        let open_us = (at.and_utc().timestamp() + 5 * 3600) * 1_000_000;
        let n = |k: usize| scaled(f[k], SCALE).ok_or(bad);
        out.push(Kline {
            open_us,
            open: n(2)?,
            high: n(3)?,
            low: n(4)?,
            close: n(5)?,
            volume: n(6)?,
            quote_volume: None,
            trades: None,
        });
    }
    Ok(out)
}

/// HistData's tick files: a month of bid/ask quotes per pair. Its own key:
/// the registry keys partitions by source, and the 1-minute job already
/// holds months of `histdata`.
pub const HISTDATA_TICKS: HistorySource = HistorySource {
    key: "histdata_tick",
    base: "https://www.histdata.com",
    interval: "tick",
    table: "histdata_ticks",
    license_note: "HistData.com free tick files: bid and ask quotes, no license or terms \
                   published, source unstated, no warranty: private to the org, never \
                   redistributed",
};

/// The HistData page of a pair's month of ticks (generic ASCII).
pub fn histdata_tick_page_path(pair: &str, month: NaiveDate) -> String {
    format!(
        "download-free-forex-historical-data/?/ascii/tick-data-quotes/{}/{}/{}",
        pair.to_ascii_lowercase(),
        month.year(),
        month.month()
    )
}

/// One line of a HistData tick file: `YYYYMMDD HHMMSSmmm,bid,ask,volume`,
/// EST without daylight saving (UTC-5), turned to UTC. The volume is
/// always 0 and not per side: left out.
pub fn histdata_tick_line(line: &str) -> Option<Tick> {
    let mut f = line.trim().split(',');
    let (stamp, bid, ask) = (f.next()?, f.next()?, f.next()?);
    let (day, time) = stamp.split_once(' ')?;
    if time.len() != 9 || !time.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let at =
        chrono::NaiveDateTime::parse_from_str(&format!("{day} {}", &time[..6]), "%Y%m%d %H%M%S")
            .ok()?;
    let ms: i64 = time[6..].parse().ok()?;
    Some(Tick {
        ts_us: (at.and_utc().timestamp() + 5 * 3600) * 1_000_000 + ms * 1_000,
        bid: scaled(bid.trim(), SCALE)?,
        ask: scaled(ask.trim(), SCALE)?,
        bid_volume: None,
        ask_volume: None,
    })
}

/// Closed months from `from` up to the one before `today`'s, minus the
/// settled ones: a month file is published once the month is over.
pub fn plan_months(from: NaiveDate, today: NaiveDate, known: &HashSet<Period>) -> Vec<Period> {
    let (Some(mut month), Some(current)) = (from.with_day(1), today.with_day(1)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while month < current {
        if !known.contains(&Period::Month(month)) {
            out.push(Period::Month(month));
        }
        month = month + Months::new(1);
    }
    out
}

pub const DUKASCOPY_TICKS: HistorySource = HistorySource {
    key: "dukascopy_public",
    base: "https://datafeed.dukascopy.com",
    interval: "tick",
    table: "dukascopy_ticks",
    license_note: "Dukascopy public tick files: broker quotes, not exchange data; terms of \
                   use not checked for redistribution or SaaS (Q-MK11): private to the org",
};

/// Currencies of the forex pairs whose scale is known. Metals, indices and
/// CFDs have other scales: refused until listed, never guessed (D820).
const FX_CURRENCIES: [&str; 10] = [
    "EUR", "USD", "GBP", "JPY", "CHF", "AUD", "CAD", "NZD", "SEK", "NOK",
];

/// Decimal places of a Dukascopy pair's integer prices: 3 for a JPY
/// quote, 5 for the other forex pairs, `None` when unknown.
pub fn fx_scale(pair: &str) -> Option<u32> {
    if pair.len() != 6 || !pair.is_ascii() {
        return None;
    }
    let (base, quote) = pair.split_at(3);
    if base == quote || !FX_CURRENCIES.contains(&base) || !FX_CURRENCIES.contains(&quote) {
        return None;
    }
    Some(if quote == "JPY" { 3 } else { 5 })
}

/// Path of a Dukascopy hour of ticks; the month is counted from 0.
pub fn dukascopy_path(pair: &str, day: NaiveDate, hour: u32) -> String {
    format!(
        "datafeed/{pair}/{:04}/{:02}/{:02}/{hour:02}h_ticks.bi5",
        day.year(),
        day.month0(),
        day.day()
    )
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

/// A decompressed `.bi5`: 20-byte big-endian records of (ms since the
/// hour, ask, bid, ask volume, bid volume). `scale` is the pair's
/// ([`fx_scale`]). `line` of the error is the 1-based record.
pub fn dukascopy_bi5(raw: &[u8], hour_start_us: i64, scale: u32) -> Result<Vec<Tick>, DecodeError> {
    if !raw.len().is_multiple_of(20) || scale > SCALE {
        return Err(DecodeError {
            line: raw.len() / 20 + 1,
        });
    }
    let up = 10_i128.pow(SCALE - scale);
    let u32_at = |r: &[u8], i: usize| u32::from_be_bytes([r[i], r[i + 1], r[i + 2], r[i + 3]]);
    // The shortest text of the f32, so 1.23 stays 1.23 and not 1.2300000190734863.
    let vol = |r: &[u8], i: usize| {
        let v = f32::from_bits(u32_at(r, i));
        v.to_string().parse::<f64>().unwrap_or(f64::from(v))
    };
    let mut out = Vec::with_capacity(raw.len() / 20);
    for (n, r) in raw.as_chunks::<20>().0.iter().enumerate() {
        let ms = u32_at(r, 0);
        if ms >= 3_600_000 {
            return Err(DecodeError { line: n + 1 });
        }
        out.push(Tick {
            ts_us: hour_start_us + i64::from(ms) * 1000,
            ask: i128::from(u32_at(r, 4)) * up,
            bid: i128::from(u32_at(r, 8)) * up,
            ask_volume: Some(vol(r, 12)),
            bid_volume: Some(vol(r, 16)),
        });
    }
    Ok(out)
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
    fn klines_in_milliseconds_and_microseconds() {
        // First minute of BTCUSDT on Binance, then a 2025 line in µs.
        let csv = b"open_time,open,high,low,close,volume,close_time,quote_volume,count,tbbv,tbqv,ignore\n\
1502942400000,4261.48000000,4261.48000000,4261.48000000,4261.48000000,1.77518300,1502942459999,7564.90685084,3,0.07518300,320.39085357,0\n\
1735689600000000,93576.00000000,93610.93000000,93537.50000000,93610.93000000,8.21827000,1735689659999999,768978.26748550,1613,4.65023000,435126.24934380,0\n";
        let k = binance_klines(csv).unwrap();
        assert_eq!(k.len(), 2);
        assert_eq!(k[0].open_us, 1_502_942_400_000_000);
        assert_eq!(k[0].close, 426_148_000_000);
        assert_eq!(k[0].trades, Some(3));
        assert_eq!(k[1].open_us, 1_735_689_600_000_000);
        assert_eq!(k[1].quote_volume, Some(76_897_826_748_550));
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

    #[test]
    fn dukascopy_scales_paths_and_records() {
        assert_eq!(fx_scale("EURUSD"), Some(5));
        assert_eq!(fx_scale("USDJPY"), Some(3));
        assert_eq!(fx_scale("XAUUSD"), None, "a metal is never guessed");
        assert_eq!(fx_scale("EUREUR"), None);
        assert_eq!(
            dukascopy_path("EURUSD", d("2025-01-06"), 7),
            "datafeed/EURUSD/2025/00/06/07h_ticks.bi5",
            "months from 0"
        );
        let mut raw = Vec::new();
        let (ms, ask, bid, av, bv) = (1500_u32, 103_456_u32, 103_452_u32, 1.23_f32, 0.75_f32);
        raw.extend(ms.to_be_bytes());
        raw.extend(ask.to_be_bytes());
        raw.extend(bid.to_be_bytes());
        raw.extend(av.to_bits().to_be_bytes());
        raw.extend(bv.to_bits().to_be_bytes());
        let t = dukascopy_bi5(&raw, 1_000_000, 5).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].ts_us, 1_000_000 + 1_500_000);
        assert_eq!(t[0].ask, 103_456_000, "1.03456 at 8 places");
        assert_eq!(t[0].bid, 103_452_000);
        assert_eq!(t[0].ask_volume, Some(1.23));
        assert_eq!(t[0].bid_volume, Some(0.75));
        // JPY: 157.123 published as 157123.
        let mut jpy = raw.clone();
        jpy[4..8].copy_from_slice(&157_123_u32.to_be_bytes());
        assert_eq!(dukascopy_bi5(&jpy, 0, 3).unwrap()[0].ask, 15_712_300_000);
        assert!(dukascopy_bi5(&raw[..19], 0, 5).is_err(), "a cut record");
        assert!(dukascopy_bi5(&[], 0, 5).unwrap().is_empty());
        let known: HashSet<Period> = [Period::Day(d("2025-01-02"))].into();
        assert_eq!(
            plan_days(d("2025-01-01"), d("2025-01-04"), &known),
            vec![Period::Day(d("2025-01-01")), Period::Day(d("2025-01-03"))]
        );
    }

    #[test]
    fn trades_by_day_in_both_units() {
        assert_eq!(
            binance_trades_path("BTCUSDT", d("2025-03-04")),
            "data/spot/daily/trades/BTCUSDT/BTCUSDT-trades-2025-03-04.zip"
        );
        let csv = b"id,price,qty,quote_qty,time,is_buyer_maker,is_best_match\n\
12,4261.48000000,0.00500000,21.30740000,1502942428322,True,True\n\
13,98000.10000000,0.10000000,9800.01000000,1735689600000123,false,true\n";
        let t = binance_trades(csv).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].ts_us, 1_502_942_428_322_000);
        assert_eq!((t[0].trade_id, t[0].side, t[0].qty), (12, "sell", 500_000));
        assert_eq!((t[1].ts_us, t[1].side), (1_735_689_600_000_123, "buy"));
        assert!(binance_trades(b"1,2,3,4,5,maybe,true\n").is_err());
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
    fn histdata_ticks_pages_lines_and_months() {
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        assert_eq!(
            histdata_tick_page_path("XAUUSD", d("2024-06-01")),
            "download-free-forex-historical-data/?/ascii/tick-data-quotes/xauusd/2024/6"
        );
        // First and last lines of the real June 2024 file: EST + 5 h, the
        // last tick of June is in July in UTC.
        let t = histdata_tick_line("20240602 180001677,2327.425000,2328.175000,0").unwrap();
        assert_eq!(t.ts_us, 1_717_369_201_677_000, "2024-06-02T23:00:01.677Z");
        assert_eq!((t.bid, t.ask), (232_742_500_000, 232_817_500_000));
        assert_eq!((t.bid_volume, t.ask_volume), (None, None));
        let last = histdata_tick_line("20240630 235959159,2324.608000,2324.995000,0").unwrap();
        assert_eq!(
            last.ts_us, 1_719_809_999_159_000,
            "2024-07-01T04:59:59.159Z"
        );
        for bad in [
            "",
            "20240602 1800016,1,2",
            "20240602180001677,1,2",
            "20240602 180001677,x,2",
            "20240602 180001677,1",
        ] {
            assert!(histdata_tick_line(bad).is_none(), "{bad}");
        }
        let known: HashSet<Period> = [Period::Month(d("2024-07-01"))].into();
        assert_eq!(
            plan_months(d("2024-06-15"), d("2024-09-10"), &known),
            vec![
                Period::Month(d("2024-06-01")),
                Period::Month(d("2024-08-01"))
            ],
            "closed months only, the settled one skipped"
        );
    }

    #[test]
    fn histdata_years_pages_forms_and_rows() {
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
        assert_eq!(
            histdata_page_path("EURUSD", Period::Month(d("2026-10-01"))),
            "download-free-forex-historical-data/?/metatrader/1-minute-bar-quotes/eurusd/2026/10"
        );
        let html = r#"<form id="file_down" name="file_down" method="POST" action="/get.php">
<input type="hidden" name="tk" id="tk" value="13f41008164167f78d67ca5eeba86ad0" />
<input type="hidden" name="date" id="date" value="2024" />
<input type="hidden" name="fxpair" id="fxpair" value="EURUSD" /></form>"#;
        assert_eq!(
            histdata_form(html).unwrap(),
            vec![
                (
                    "tk".to_string(),
                    "13f41008164167f78d67ca5eeba86ad0".to_string()
                ),
                ("date".to_string(), "2024".to_string()),
                ("fxpair".to_string(), "EURUSD".to_string()),
            ]
        );
        assert_eq!(histdata_form("<html>no file</html>"), None);
        assert_eq!(
            histdata_form(&html.replace("2024", "a&b=c")),
            None,
            "a field is sent back only as plain letters"
        );
        let rows =
            histdata_mt_m1(b"2024.01.01,17:00,1.104270,1.104290,1.104250,1.104290,0\n").unwrap();
        // 17:00 EST is 22:00 UTC.
        assert_eq!(rows[0].open_us, 1_704_146_400_000_000);
        assert_eq!(rows[0].close, 110_429_000);
        assert!(histdata_mt_m1(b"2024-01-01,17:00,1,1,1,1,0\n").is_err());
    }
}
