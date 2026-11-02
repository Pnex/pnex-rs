//! Event study (markets-research.md D532, D535): how a price moved around
//! dated events, against a benchmark. Pure functions on daily bars, tested on
//! synthetic series whose result is known. No look-ahead: the market model is
//! estimated strictly before the event.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// Trading days of the market model before the event: [-250, -30].
const BETA_FROM: usize = 250;
const BETA_TO: usize = 30;
const BETA_MIN_POINTS: usize = 60;
/// Trading days of the normal volume before the event.
const VOLUME_DAYS: usize = 30;
/// Windows in trading days around the event.
pub const WINDOWS: [(i32, i32); 3] = [(-1, 1), (0, 5), (0, 20)];
const BOOTSTRAP_DRAWS: usize = 2000;

/// One trading day as the source gave it: the raw close, never adjusted.
#[derive(Debug, Clone, PartialEq)]
pub struct Bar {
    pub date: NaiveDate,
    pub close: f64,
    pub volume: f64,
    /// Cash paid per share on this ex-date.
    pub dividend: f64,
    /// New shares per old share on this date; 1 when none.
    pub split: f64,
}

/// Total return of each day but the first: dividends and splits count, so a
/// 10-for-1 split is not a 90% fall. A day without a usable price is skipped.
pub fn returns(bars: &[Bar]) -> Vec<(NaiveDate, f64)> {
    bars.windows(2)
        .filter(|w| w[0].close > 0.0 && w[1].close > 0.0 && w[1].split > 0.0)
        .map(|w| {
            let today = w[1].close * w[1].split + w[1].dividend;
            (w[1].date, today / w[0].close - 1.0)
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WindowResult {
    /// First trading day of the window, relative to the event.
    pub from: i32,
    pub to: i32,
    /// Compounded total return over the window.
    pub raw: f64,
    /// Sum of the daily returns less what the benchmark explains.
    pub abnormal: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EventStudy {
    /// The first trading day at or after the event date.
    #[cfg_attr(feature = "openapi", schema(format = "date"))]
    pub day_zero: String,
    pub beta: f64,
    /// Only the windows the prices fully cover.
    pub windows: Vec<WindowResult>,
    /// Volume of day zero against the 30 days before; `None` without volume.
    pub relative_volume: Option<f64>,
}

/// Slope of `y` on `x`; `None` when `x` does not move.
fn slope(points: &[(f64, f64)]) -> Option<f64> {
    let n = points.len() as f64;
    let (mx, my) = points
        .iter()
        .fold((0.0, 0.0), |(a, b), (x, y)| (a + x / n, b + y / n));
    let (mut cov, mut var) = (0.0, 0.0);
    for (x, y) in points {
        cov += (x - mx) * (y - my);
        var += (x - mx) * (x - mx);
    }
    (var > 0.0).then(|| cov / var)
}

/// The study of one event, or `None` when the history before it is too short
/// to estimate the market model, or no price follows it.
pub fn event_study(stock: &[Bar], benchmark: &[Bar], event: NaiveDate) -> Option<EventStudy> {
    let bench: std::collections::HashMap<NaiveDate, f64> = returns(benchmark).into_iter().collect();
    // Common days only: a holiday of one market is no return of the other.
    let days: Vec<(NaiveDate, f64, f64)> = returns(stock)
        .into_iter()
        .filter_map(|(d, r)| Some((d, r, *bench.get(&d)?)))
        .collect();
    let zero = days.iter().position(|(d, _, _)| *d >= event)?;
    if zero < BETA_TO {
        return None;
    }
    let estimation = &days[zero.saturating_sub(BETA_FROM)..zero - BETA_TO + 1];
    if estimation.len() < BETA_MIN_POINTS {
        return None;
    }
    let points: Vec<(f64, f64)> = estimation.iter().map(|(_, r, b)| (*b, *r)).collect();
    let beta = slope(&points)?;
    let windows = WINDOWS
        .iter()
        .filter_map(|&(from, to)| {
            let (a, b) = (zero as i32 + from, zero as i32 + to);
            if a < 0 || b as usize >= days.len() {
                return None;
            }
            let span = &days[a as usize..=b as usize];
            Some(WindowResult {
                from,
                to,
                raw: span.iter().fold(1.0, |acc, (_, r, _)| acc * (1.0 + r)) - 1.0,
                abnormal: span.iter().map(|(_, r, m)| r - beta * m).sum(),
            })
        })
        .collect();
    let day_zero = days[zero].0;
    let volumes: Vec<f64> = stock
        .iter()
        .filter(|b| b.date < day_zero && b.volume > 0.0)
        .map(|b| b.volume)
        .collect();
    let recent = &volumes[volumes.len().saturating_sub(VOLUME_DAYS)..];
    let normal = recent.iter().sum::<f64>() / recent.len().max(1) as f64;
    let relative_volume = stock
        .iter()
        .find(|b| b.date == day_zero)
        .filter(|b| b.volume > 0.0 && normal > 0.0)
        .map(|b| b.volume / normal);
    Some(EventStudy {
        day_zero: day_zero.to_string(),
        beta,
        windows,
        relative_volume,
    })
}

/// Distribution of a window's abnormal return over a set of events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Distribution {
    /// Events measured: always shown, a mean of five says little.
    pub count: usize,
    pub mean: f64,
    pub median: f64,
    /// Share of the events with a positive abnormal return, 0 to 1.
    pub positive: f64,
    /// 95% bootstrap interval of the mean.
    pub mean_low: f64,
    pub mean_high: f64,
}

/// One window of a rule: the events against the drawn dates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RuleWindow {
    pub from: i32,
    pub to: i32,
    pub events: Option<Distribution>,
    pub baseline: Option<Distribution>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RuleResult {
    /// Filings matching the rule; `events` of a window can be fewer, when the
    /// prices do not cover it.
    pub filings: usize,
    /// Instruments with both filings and prices.
    pub instruments: usize,
    /// Followed instruments only: those the org stopped following or that
    /// disappeared are missing, which flatters a result.
    pub windows: Vec<RuleWindow>,
    /// No benchmark prices: nothing can be measured.
    pub benchmark_missing: bool,
}

/// A fictitious position measured on closing prices (D534).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PositionResult {
    #[cfg_attr(feature = "openapi", schema(format = "date"))]
    pub entry_on: String,
    pub entry_close: f64,
    #[cfg_attr(feature = "openapi", schema(format = "date"))]
    pub last_on: String,
    pub last_close: f64,
    /// Total return in the position's direction: dividends and splits count.
    pub total_return: f64,
    /// `quantity` shares at the entry close, times that return.
    pub result: f64,
    /// False when an exit day is set: the result no longer moves.
    pub open: bool,
}

/// Entry at the first close at or after `entry`, exit at the last close at or
/// before `exit` (the latest one when open). `None` without two such closes.
pub fn position_result(
    bars: &[Bar],
    short: bool,
    quantity: f64,
    entry: NaiveDate,
    exit: Option<NaiveDate>,
) -> Option<PositionResult> {
    let held: Vec<Bar> = bars
        .iter()
        .filter(|b| b.date >= entry && exit.is_none_or(|e| b.date <= e))
        .cloned()
        .collect();
    let (first, last) = (held.first()?, held.last()?);
    let growth = returns(&held)
        .iter()
        .fold(1.0, |acc, (_, r)| acc * (1.0 + r))
        - 1.0;
    let total_return = if short { -growth } else { growth };
    Some(PositionResult {
        entry_on: first.date.to_string(),
        entry_close: first.close,
        last_on: last.date.to_string(),
        last_close: last.close,
        total_return,
        result: quantity * first.close * total_return,
        open: exit.is_none(),
    })
}

/// Small deterministic generator: the same events always give the same
/// interval, and the WASM build pulls no random source.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n
    }
}

pub fn distribution(values: &[f64]) -> Option<Distribution> {
    if values.is_empty() {
        return None;
    }
    let n = values.len();
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let median = if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    };
    let mut rng = Lcg(0x9E37_79B9_7F4A_7C15 ^ n as u64);
    let mut means: Vec<f64> = (0..BOOTSTRAP_DRAWS)
        .map(|_| (0..n).map(|_| values[rng.below(n)]).sum::<f64>() / n as f64)
        .collect();
    means.sort_by(f64::total_cmp);
    Some(Distribution {
        count: n,
        mean: values.iter().sum::<f64>() / n as f64,
        median,
        positive: values.iter().filter(|v| **v > 0.0).count() as f64 / n as f64,
        mean_low: means[BOOTSTRAP_DRAWS * 25 / 1000],
        mean_high: means[BOOTSTRAP_DRAWS * 975 / 1000 - 1],
    })
}

/// `count` dates drawn among those of the bars, far enough from both ends to
/// be studied: the baseline a rule is compared to, on the same instrument.
pub fn baseline_dates(bars: &[Bar], count: usize, seed: u64) -> Vec<NaiveDate> {
    let margin = BETA_MIN_POINTS + BETA_TO;
    if bars.len() <= margin + 21 {
        return Vec::new();
    }
    let usable = &bars[margin..bars.len() - 21];
    let mut rng = Lcg(seed ^ 0xD6E8_FEB8_6659_FD93);
    (0..count)
        .map(|_| usable[rng.below(usable.len())].date)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(i: i64) -> NaiveDate {
        NaiveDate::from_ymd_opt(2020, 1, 1).unwrap() + chrono::Duration::days(i)
    }

    fn series(rets: &[f64]) -> Vec<Bar> {
        let mut close = 100.0;
        let mut out = vec![Bar {
            date: day(0),
            close,
            volume: 100.0,
            dividend: 0.0,
            split: 1.0,
        }];
        for (i, r) in rets.iter().enumerate() {
            close *= 1.0 + r;
            out.push(Bar {
                date: day(i as i64 + 1),
                close,
                volume: 100.0,
                dividend: 0.0,
                split: 1.0,
            });
        }
        out
    }

    fn market(n: usize) -> Vec<f64> {
        (0..n).map(|i| 0.01 * ((i as f64 * 1.7).sin())).collect()
    }

    #[test]
    fn a_split_and_a_dividend_are_not_price_moves() {
        let mk = |close, dividend, split, i| Bar {
            date: day(i),
            close,
            volume: 1.0,
            dividend,
            split,
        };
        let r = returns(&[
            mk(100.0, 0.0, 1.0, 0),
            mk(10.0, 0.0, 10.0, 1),
            mk(9.0, 1.0, 1.0, 2),
        ]);
        assert!((r[0].1 - 0.0).abs() < 1e-12, "10-for-1 split: {r:?}");
        assert!((r[1].1 - 0.0).abs() < 1e-12, "dividend paid: {r:?}");
    }

    #[test]
    fn the_known_jump_and_beta_come_back() {
        let m = market(320);
        let mut s: Vec<f64> = m.iter().map(|r| 2.0 * r).collect();
        s[300] += 0.05;
        let (stock, bench) = (series(&s), series(&m));
        // Returns index i is the bar of day i + 1.
        let st = event_study(&stock, &bench, day(301)).expect("enough history");
        assert_eq!(st.day_zero, day(301).to_string());
        assert!((st.beta - 2.0).abs() < 1e-9, "{st:?}");
        let w = st
            .windows
            .iter()
            .find(|w| (w.from, w.to) == (-1, 1))
            .unwrap();
        assert!((w.abnormal - 0.05).abs() < 1e-9, "{w:?}");
        assert_eq!(st.windows.len(), 2, "[0,+20] goes past the prices: {st:?}");
    }

    #[test]
    fn the_market_model_never_sees_the_event() {
        let m = market(320);
        let mut a: Vec<f64> = m.iter().map(|r| 1.5 * r).collect();
        let b = a.clone();
        // A move after the estimation window cannot change the beta.
        a[290] += 0.5;
        let beta = |s: &[f64]| event_study(&series(s), &series(&m), day(301)).unwrap().beta;
        assert_eq!(beta(&a), beta(&b));
    }

    #[test]
    fn too_little_history_gives_no_study() {
        let m = market(50);
        assert!(event_study(&series(&m), &series(&m), day(45)).is_none());
        // After the last price: nothing to measure.
        let m = market(320);
        assert!(event_study(&series(&m), &series(&m), day(900)).is_none());
    }

    #[test]
    fn relative_volume_compares_with_the_days_before() {
        let m = market(320);
        let mut stock = series(&m);
        stock[301].volume = 500.0;
        let st = event_study(&stock, &series(&m), day(301)).unwrap();
        assert_eq!(st.relative_volume, Some(5.0));
    }

    #[test]
    fn distribution_counts_and_its_interval_holds_the_mean() {
        let d = distribution(&[0.01, 0.02, 0.03, -0.01]).unwrap();
        assert_eq!(d.count, 4);
        assert!((d.mean - 0.0125).abs() < 1e-12);
        assert!((d.median - 0.015).abs() < 1e-12);
        assert_eq!(d.positive, 0.75);
        assert!(d.mean_low <= d.mean && d.mean <= d.mean_high, "{d:?}");
        assert_eq!(
            distribution(&[0.01, 0.02]),
            distribution(&[0.01, 0.02]),
            "repeatable"
        );
        assert!(distribution(&[]).is_none());
    }

    #[test]
    fn a_position_counts_its_direction_and_survives_a_split() {
        let mk = |i, close, split| Bar {
            date: day(i),
            close,
            volume: 1.0,
            dividend: 0.0,
            split,
        };
        let bars = [
            mk(0, 100.0, 1.0),
            mk(1, 110.0, 1.0),
            mk(2, 11.0, 10.0),
            mk(3, 12.1, 1.0),
        ];
        let long = position_result(&bars, false, 2.0, day(0), None).unwrap();
        assert!((long.total_return - 0.21).abs() < 1e-9, "{long:?}");
        assert!((long.result - 42.0).abs() < 1e-9, "{long:?}");
        assert!(long.open);
        let short = position_result(&bars, true, 2.0, day(0), Some(day(1))).unwrap();
        assert!((short.total_return + 0.10).abs() < 1e-9, "{short:?}");
        assert_eq!(short.last_on, day(1).to_string());
        assert!(!short.open);
        assert!(position_result(&bars, false, 1.0, day(50), None).is_none());
    }

    #[test]
    fn baseline_dates_stay_studiable_and_repeat() {
        let bars = series(&market(320));
        let a = baseline_dates(&bars, 50, 7);
        assert_eq!(a, baseline_dates(&bars, 50, 7));
        assert!(a.iter().all(|d| *d >= bars[90].date && *d < bars[300].date));
        assert!(baseline_dates(&series(&market(40)), 5, 1).is_empty());
    }
}
