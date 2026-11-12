//! Decisions of an external model (markets-research.md appendix G, D892–D896):
//! bars read at a date, the checks of a posted decision, its realized outcome
//! and the scores of a model. Pure code: no I/O. An aid to analysis: nothing
//! here places an order.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::history::SCALE;

/// Options of a decision in V1 (Q-MD2), in this order.
pub const OPTIONS: [&str; 3] = ["buy", "neutral", "sell"];
/// Most bars one read returns.
pub const BARS_MAX: usize = 5_000;
/// Decisions an org may post per UTC day.
pub const DAILY_MAX: i64 = 2_000;
/// Bytes of the free `inputs` JSON.
pub const INPUTS_MAX: usize = 4 * 1024;

/// Intervals a bar can be read at, with their length in µs.
pub const INTERVALS: [(&str, i64); 6] = [
    ("1m", 60_000_000),
    ("5m", 300_000_000),
    ("15m", 900_000_000),
    ("1h", 3_600_000_000),
    ("4h", 14_400_000_000),
    ("1d", 86_400_000_000),
];

pub fn interval_us(interval: &str) -> Option<i64> {
    INTERVALS
        .iter()
        .find(|(k, _)| *k == interval)
        .map(|(_, us)| *us)
}

/// The table of a `source` column value (D807): one per (source, shape).
// shortcut: names the tables the market pipelines write; goes with the market
// routes in PE7 (pipeline-engine.md D948).
pub fn bar_table(source: &str) -> Option<&'static str> {
    if source.starts_with("mt5:") && source.len() > 4 {
        return Some(super::history::MT5_OHLCV_1M.table);
    }
    match source {
        "binance_public" => Some("binance_klines_1m"),
        "histdata" => Some("histdata_m1"),
        _ => None,
    }
}

/// A bar, prices and volume at [`SCALE`] places, `ts_us` its open (UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bar {
    pub ts_us: i64,
    pub open: i128,
    pub high: i128,
    pub low: i128,
    pub close: i128,
    pub volume: i128,
}

/// Minute bars in commit order → bars of `interval_us` aligned on the epoch,
/// opening in [`from_us`, `to_us`) and closed at `as_of_us` (a bar is known at
/// its close, D892). A minute written twice keeps its last write (MT5
/// backfill, E.5).
pub fn resample(
    minutes: impl IntoIterator<Item = Bar>,
    interval_us: i64,
    from_us: i64,
    to_us: i64,
    as_of_us: i64,
) -> Vec<Bar> {
    let by_ts: BTreeMap<i64, Bar> = minutes.into_iter().map(|b| (b.ts_us, b)).collect();
    let mut out: Vec<Bar> = Vec::new();
    for b in by_ts.into_values() {
        let bin = b.ts_us - b.ts_us.rem_euclid(interval_us);
        if bin < from_us || bin >= to_us || bin + interval_us > as_of_us {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.ts_us == bin => {
                last.high = last.high.max(b.high);
                last.low = last.low.min(b.low);
                last.close = b.close;
                last.volume += b.volume;
            }
            _ => out.push(Bar { ts_us: bin, ..b }),
        }
    }
    out
}

/// A value at [`SCALE`] places as decimal text, trailing zeros cut.
pub fn decimal_text(v: i128) -> String {
    let unit = 10i128.pow(SCALE);
    let sign = if v < 0 { "-" } else { "" };
    let (int, frac) = (v.abs() / unit, v.abs() % unit);
    let frac = format!("{frac:0width$}", width = SCALE as usize);
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        format!("{sign}{int}")
    } else {
        format!("{sign}{int}.{frac}")
    }
}

fn price(v: i128) -> f64 {
    v as f64 / 10f64.powi(SCALE as i32)
}

/// Token of the first defect of posted probabilities, `None` when valid.
pub fn probabilities_defect(p: &[f64]) -> Option<&'static str> {
    if p.len() != OPTIONS.len() {
        return Some("length:3");
    }
    if p.iter().any(|x| !x.is_finite() || *x < 0.0 || *x > 1.0) {
        return Some("range:0..1");
    }
    ((p.iter().sum::<f64>() - 1.0).abs() > 1e-3).then_some("sum:1")
}

/// Index of the most likely option and 1 − normalized entropy (the formula
/// of `pnex-decide`), computed by the server, never taken from the caller.
pub fn best_and_confidence(p: &[f64]) -> (usize, f64) {
    let best = p
        .iter()
        .enumerate()
        .fold(0, |b, (i, x)| if *x > p[b] { i } else { b });
    let entropy: f64 = p.iter().map(|x| -x * x.max(1e-12).ln()).sum();
    (best, 1.0 - entropy / (p.len() as f64).ln())
}

/// A decision posted by the runner of an external model (D894).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NewDecision {
    pub model: String,
    pub model_version: String,
    pub instrument: String,
    /// `source` column of the bars read (`histdata`, `mt5:<broker>`…).
    pub source: String,
    /// `1m`, `5m`, `15m`, `1h`, `4h` or `1d`.
    pub interval: String,
    pub horizon_bars: u32,
    /// Instant of the data read (RFC 3339).
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub as_of: String,
    /// `buy`, `neutral`, `sell`, in this order (V1).
    pub options: Vec<String>,
    /// One per option, in [0, 1], summing to 1.
    pub probabilities: Vec<f64>,
    #[serde(default)]
    pub barriers: Option<Barriers>,
    /// Free JSON for traceability, 4 KiB at most.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Object>))]
    pub inputs: Option<serde_json::Value>,
    /// Probabilities on prediction markets, scored against the market once
    /// resolved (markets-research.md D925); 50 at most.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forecasts: Vec<super::prediction::Forecast>,
}

/// A stored decision; `best` and `confidence` are the server's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Decision {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub model: String,
    pub model_version: String,
    pub instrument: String,
    pub source: String,
    pub interval: String,
    pub horizon_bars: u32,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub as_of: String,
    pub options: Vec<String>,
    pub probabilities: Vec<f64>,
    pub best: String,
    pub confidence: f64,
    pub barriers: Option<Barriers>,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Object>))]
    pub inputs: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forecasts: Vec<super::prediction::Forecast>,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
}

/// Scores of one (model, version, instrument, source, interval).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ModelScores {
    pub model: String,
    pub model_version: String,
    pub instrument: String,
    pub source: String,
    pub interval: String,
    pub scores: Scores,
}

/// A bar as the API returns it, prices as exact decimal text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BarOut {
    /// Open of the bar (UTC); the bar is known at `ts` + interval.
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub ts: String,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: String,
}

/// Barriers of a decision, in ATR (D896).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Barriers {
    pub take_profit_atr: f64,
    pub stop_atr: f64,
    pub atr_period: u32,
}

/// What happened after a decision.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Outcome {
    /// Option realized by the triple barrier; `None` without barriers.
    pub label: Option<&'static str>,
    /// Close `horizon_bars` bars later over the close at `as_of`, minus 1.
    pub ret: f64,
}

/// The labelling contract (D896): `before` = bars closed at `as_of`,
/// `after` = bars opening at `as_of` or later, both oldest first. `None`
/// until `horizon` bars are closed, or without enough bars for the ATR.
pub fn outcome(
    before: &[Bar],
    after: &[Bar],
    horizon: usize,
    barriers: Option<&Barriers>,
) -> Option<Outcome> {
    let last = before.last()?;
    if horizon == 0 || after.len() < horizon {
        return None;
    }
    let base = price(last.close);
    let ret = price(after[horizon - 1].close) / base - 1.0;
    let Some(b) = barriers else {
        return Some(Outcome { label: None, ret });
    };
    let n = b.atr_period as usize;
    if n == 0 || before.len() < n + 1 {
        return None;
    }
    let tail = &before[before.len() - n - 1..];
    let atr = tail
        .windows(2)
        .map(|w| {
            let (prev, h, l) = (price(w[0].close), price(w[1].high), price(w[1].low));
            (h - l).max((h - prev).abs()).max((l - prev).abs())
        })
        .sum::<f64>()
        / n as f64;
    let (up, down) = (base + b.take_profit_atr * atr, base - b.stop_atr * atr);
    let mut label = "neutral";
    for bar in &after[..horizon] {
        let (hit_up, hit_down) = (price(bar.high) >= up, price(bar.low) <= down);
        if hit_up || hit_down {
            // Both in one bar: the order inside it is unknown.
            label = match (hit_up, hit_down) {
                (true, false) => "buy",
                (false, true) => "sell",
                _ => "neutral",
            };
            break;
        }
    }
    Some(Outcome {
        label: Some(label),
        ret,
    })
}

/// A tenth of the probability axis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReliabilityBin {
    pub from: f64,
    pub to: f64,
    /// (decision, option) pairs whose probability fell here.
    pub count: u64,
    pub mean_probability: f64,
    /// Share of those pairs whose option was realized.
    pub observed: f64,
}

/// Scores of a model on its measured decisions (D896).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Scores {
    pub decisions: u64,
    /// Decisions with barriers whose horizon has passed.
    pub measured: u64,
    /// Share of measured decisions whose most likely option was realized.
    pub hit_rate: Option<f64>,
    /// Multi-class Brier score, 0 (perfect) to 2.
    pub brier: Option<f64>,
    pub reliability: Vec<ReliabilityBin>,
    pub threshold: f64,
    /// Measured decisions whose top probability reaches `threshold`.
    pub above_threshold: u64,
    pub above_threshold_hit_rate: Option<f64>,
    /// Mean return at the horizon, decisions without barriers included.
    pub mean_return: Option<f64>,
}

/// `items` = (probabilities, outcome) of each decision, the outcome `None`
/// before its horizon.
pub fn scores(items: &[(Vec<f64>, Option<Outcome>)], threshold: f64) -> Scores {
    let mut bins = [(0u64, 0f64, 0f64); 10];
    let (mut measured, mut hits, mut brier, mut above, mut above_hits) =
        (0u64, 0u64, 0f64, 0u64, 0u64);
    let mut returns = Vec::new();
    for (p, out) in items {
        let Some(out) = out else { continue };
        returns.push(out.ret);
        let Some(label) = out.label else { continue };
        let Some(real) = OPTIONS.iter().position(|o| *o == label) else {
            continue;
        };
        measured += 1;
        let (best, _) = best_and_confidence(p);
        let hit = u64::from(best == real);
        hits += hit;
        if p[best] >= threshold {
            above += 1;
            above_hits += hit;
        }
        for (i, x) in p.iter().enumerate() {
            let y = if i == real { 1.0 } else { 0.0 };
            brier += (x - y).powi(2);
            let bin = &mut bins[((x * 10.0) as usize).min(9)];
            bin.0 += 1;
            bin.1 += x;
            bin.2 += y;
        }
    }
    let share = |a: u64, b: u64| (b > 0).then(|| a as f64 / b as f64);
    Scores {
        decisions: items.len() as u64,
        measured,
        hit_rate: share(hits, measured),
        brier: (measured > 0).then(|| brier / measured as f64),
        reliability: bins
            .iter()
            .enumerate()
            .filter(|(_, b)| b.0 > 0)
            .map(|(i, b)| ReliabilityBin {
                from: i as f64 / 10.0,
                to: (i + 1) as f64 / 10.0,
                count: b.0,
                mean_probability: b.1 / b.0 as f64,
                observed: b.2 / b.0 as f64,
            })
            .collect(),
        threshold,
        above_threshold: above,
        above_threshold_hit_rate: share(above_hits, above),
        mean_return: (!returns.is_empty())
            .then(|| returns.iter().sum::<f64>() / returns.len() as f64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const U: i128 = 100_000_000;
    const MIN: i64 = 60_000_000;

    fn bar(ts_us: i64, o: i128, h: i128, l: i128, c: i128) -> Bar {
        Bar {
            ts_us,
            open: o * U,
            high: h * U,
            low: l * U,
            close: c * U,
            volume: U,
        }
    }

    #[test]
    fn resample_keeps_closed_bins_and_the_last_write() {
        let h = 60 * MIN;
        // Two hours of minutes, the 10th minute written twice.
        let mut m: Vec<Bar> = (0..120).map(|i| bar(i * MIN, 10, 11, 9, 10)).collect();
        m.push(bar(10 * MIN, 10, 50, 1, 10));
        let bars = resample(m.clone(), h, 0, i64::MAX, 2 * h);
        assert_eq!(bars.len(), 2);
        assert_eq!((bars[0].high, bars[0].low), (50 * U, U));
        assert_eq!(bars[0].volume, 60 * U, "a duplicate counts once");
        // One minute before the second bar closes: it is never returned.
        let bars = resample(m.clone(), h, 0, i64::MAX, 2 * h - MIN);
        assert_eq!(bars.len(), 1);
        // `from` / `to` on the bar's open.
        assert_eq!(resample(m, h, h, i64::MAX, 2 * h)[0].ts_us, h);
    }

    #[test]
    fn decimal_text_cuts_trailing_zeros() {
        assert_eq!(decimal_text(123_450_000_000), "1234.5");
        assert_eq!(decimal_text(-5 * U), "-5");
        assert_eq!(decimal_text(1), "0.00000001");
    }

    #[test]
    fn probabilities_are_checked() {
        assert_eq!(probabilities_defect(&[0.2, 0.3, 0.5]), None);
        assert_eq!(probabilities_defect(&[0.5, 0.5]), Some("length:3"));
        assert_eq!(probabilities_defect(&[1.2, -0.1, -0.1]), Some("range:0..1"));
        assert_eq!(probabilities_defect(&[0.5, 0.5, 0.5]), Some("sum:1"));
        let (best, c) = best_and_confidence(&[1.0 / 3.0; 3]);
        assert!((best, c.abs() < 1e-9) == (0, true));
        let (best, c) = best_and_confidence(&[0.0, 0.0, 1.0]);
        assert_eq!(best, 2);
        assert!((c - 1.0).abs() < 1e-6);
    }

    fn history() -> Vec<Bar> {
        // Flat at 100 with a range of 2: ATR = 2.
        (0..20).map(|i| bar(i, 100, 101, 99, 100)).collect()
    }

    #[test]
    fn triple_barrier_outcomes() {
        let b = Barriers {
            take_profit_atr: 1.0,
            stop_atr: 1.0,
            atr_period: 14,
        };
        let o = |after: Vec<Bar>| outcome(&history(), &after, 3, Some(&b)).map(|o| o.label);
        let flat = bar(0, 100, 101, 99, 100);
        assert_eq!(
            o(vec![flat, bar(0, 100, 102, 99, 101), flat]),
            Some(Some("buy"))
        );
        assert_eq!(
            o(vec![flat, bar(0, 100, 101, 98, 99), flat]),
            Some(Some("sell"))
        );
        assert_eq!(o(vec![flat, flat, flat]), Some(Some("neutral")));
        assert_eq!(
            o(vec![bar(0, 100, 103, 97, 100), flat, flat]),
            Some(Some("neutral")),
            "both in one bar"
        );
        assert_eq!(o(vec![flat, flat]), None, "horizon not passed");
        assert_eq!(
            outcome(&history()[..10], &[flat; 3], 3, Some(&b)),
            None,
            "too few bars for the ATR"
        );
        let r = outcome(
            &history(),
            &[flat, flat, bar(0, 100, 110, 100, 110)],
            3,
            None,
        )
        .unwrap();
        assert_eq!(r.label, None);
        assert!((r.ret - 0.1).abs() < 1e-9);
    }

    #[test]
    fn brier_and_reliability() {
        let out = |l| {
            Some(Outcome {
                label: Some(l),
                ret: 0.0,
            })
        };
        let mut items = Vec::new();
        // 1 000 decisions: buy at 0.7, realized 7 times in 10.
        for i in 0..1000 {
            items.push((
                vec![0.7, 0.2, 0.1],
                out(if i % 10 < 7 { "buy" } else { "sell" }),
            ));
        }
        items.push((vec![0.7, 0.2, 0.1], None));
        let s = scores(&items, 0.6);
        assert_eq!(
            (s.decisions, s.measured, s.above_threshold),
            (1001, 1000, 1000)
        );
        assert!((s.hit_rate.unwrap() - 0.7).abs() < 1e-9);
        // buy: 0.7·0.09 + 0.3·0.49 ; neutral 0.04 ; sell 0.7·0.01 + 0.3·0.81
        let want = 0.7 * 0.09 + 0.3 * 0.49 + 0.04 + 0.7 * 0.01 + 0.3 * 0.81;
        assert!((s.brier.unwrap() - want).abs() < 1e-9, "{:?}", s.brier);
        let seven = s.reliability.iter().find(|b| b.from == 0.7).unwrap();
        assert_eq!(seven.count, 1000);
        assert!((seven.observed - 0.7).abs() < 1e-9);
        let one = s.reliability.iter().find(|b| b.from == 0.1).unwrap();
        assert!(
            (one.observed - 0.3).abs() < 1e-9,
            "sell at 0.1, realized 3 in 10"
        );
    }
}
