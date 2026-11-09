//! Abnormal moves of a daily series (markets-research.md D735): a session
//! whose change is both large against the series' own recent volatility and
//! large in absolute terms. Pure code: no I/O, no clock.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// How a session's change is measured; fixed per series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveMode {
    /// `ln(p[t] / p[t-1])`, for prices and indices.
    LogReturn,
    /// `p[t] - p[t-1]` in the series' unit, for rates and the VIX.
    Diff,
    /// `ln(v[t])` against the mean of `ln(v)` over the window, for traded
    /// volumes (D810): only an unusually high volume is a move.
    Volume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveDirection {
    Up,
    Down,
}

/// Thresholds of the detector, settable per org and per series.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MoveParams {
    /// Trigger when |change| >= `k` × sigma.
    pub k: f64,
    /// ... and |change| >= `floor`, so a nearly flat series stays quiet.
    pub floor: f64,
    /// Changes before the session that sigma is computed on.
    pub window: usize,
    /// Fewer changes than this in the window: no detection at all.
    pub min_history: usize,
}

/// Floor of a currency series in log return (Q-MK7, 2026-11-06).
pub const FX_FLOOR: f64 = 0.0075;

impl MoveParams {
    /// Defaults of D735: k = 3, floor 2 % in log return, 0.15 in diff
    /// (15 bp on a rate quoted in percent).
    pub fn defaults(mode: MoveMode) -> Self {
        let floor = match mode {
            MoveMode::LogReturn => 0.02,
            MoveMode::Diff => 0.15,
            // At least twice the usual volume.
            MoveMode::Volume => std::f64::consts::LN_2,
        };
        Self {
            k: 3.0,
            floor,
            window: 250,
            min_history: 60,
        }
    }
}

/// One session that crossed both thresholds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Move {
    pub date: NaiveDate,
    /// The measured change (log return or difference).
    pub value: f64,
    pub direction: MoveDirection,
    /// Sample standard deviation of the changes in the window before `date`.
    pub sigma: f64,
    /// The bar crossed: max(k × sigma, floor).
    pub threshold: f64,
}

fn change(mode: MoveMode, prev: f64, cur: f64) -> Option<f64> {
    let v = match mode {
        MoveMode::LogReturn if prev > 0.0 && cur > 0.0 => (cur / prev).ln(),
        MoveMode::LogReturn => return None,
        MoveMode::Diff => cur - prev,
        MoveMode::Volume if cur > 0.0 => cur.ln(),
        MoveMode::Volume => return None,
    };
    v.is_finite().then_some(v)
}

fn sample_sd(xs: &[f64]) -> f64 {
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    (xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt()
}

/// Every abnormal session of `series` (dated values, any order). A day with
/// no quote is simply absent: changes run between consecutive quoted days,
/// nothing is interpolated. Non-finite values are dropped like missing days.
pub fn detect(series: &[(NaiveDate, f64)], mode: MoveMode, params: &MoveParams) -> Vec<Move> {
    let mut points: Vec<(NaiveDate, f64)> = series
        .iter()
        .copied()
        .filter(|(_, v)| v.is_finite())
        .collect();
    points.sort_by_key(|(d, _)| *d);
    points.dedup_by_key(|(d, _)| *d);

    // (date, change) between consecutive quoted days; a change that cannot be
    // measured (a non-positive price) breaks nothing, it is just left out.
    let changes: Vec<(NaiveDate, f64)> = points
        .windows(2)
        .filter_map(|w| change(mode, w[0].1, w[1].1).map(|c| (w[1].0, c)))
        .collect();

    let min_history = params.min_history.max(2);
    let mut out = Vec::new();
    for i in min_history..changes.len() {
        let past: Vec<f64> = changes[i.saturating_sub(params.window)..i]
            .iter()
            .map(|c| c.1)
            .collect();
        if past.len() < min_history {
            continue;
        }
        let (date, mut value) = changes[i];
        if mode == MoveMode::Volume {
            // A level, not a change: measured against the window's mean.
            value -= past.iter().sum::<f64>() / past.len() as f64;
        }
        let sigma = sample_sd(&past);
        let threshold = (params.k * sigma).max(params.floor);
        if value.abs() >= threshold && (mode != MoveMode::Volume || value > 0.0) {
            let direction = if value > 0.0 {
                MoveDirection::Up
            } else {
                MoveDirection::Down
            };
            out.push(Move {
                date,
                value,
                direction,
                sigma,
                threshold,
            });
        }
    }
    out
}

impl MoveMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LogReturn => "log_return",
            Self::Diff => "diff",
            Self::Volume => "volume",
        }
    }
}

impl MoveDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
        }
    }
}

/// What a watched series is (D734): an index, a commodity, a rate…
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetClass {
    EquityIndex,
    Commodity,
    Rates,
    Volatility,
    Fx,
    Crypto,
}

impl AssetClass {
    pub const ALL: [AssetClass; 6] = [
        Self::EquityIndex,
        Self::Commodity,
        Self::Rates,
        Self::Volatility,
        Self::Fx,
        Self::Crypto,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::EquityIndex => "equity_index",
            Self::Commodity => "commodity",
            Self::Rates => "rates",
            Self::Volatility => "volatility",
            Self::Fx => "fx",
            Self::Crypto => "crypto",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == raw)
    }
}

/// One entry of the `move_series` setting (D737).
#[derive(Debug, Clone, PartialEq)]
pub struct WatchedSeries {
    /// FRED code or ticker, upper-cased like every setting code.
    pub code: String,
    pub mode: MoveMode,
    pub asset_class: AssetClass,
    pub params: MoveParams,
}

/// The `move_series` setting: `[source:]CODE:mode:asset_class[:k[:floor]]`,
/// comma or semicolon separated. The source prefix (`bis`, D888) only tells
/// which import reads the series. A malformed entry is left out, never guessed.
pub fn watched_series(raw: &str) -> Vec<WatchedSeries> {
    raw.split([',', ';'])
        .filter_map(|entry| {
            let mut parts: Vec<&str> = entry.split(':').map(str::trim).collect();
            if crate::ontology::import::source_prefix(entry).is_some() {
                parts.remove(0);
            }
            let code = parts.first()?.to_uppercase();
            let mode = match *parts.get(1)? {
                "log_return" => MoveMode::LogReturn,
                "diff" => MoveMode::Diff,
                "volume" => MoveMode::Volume,
                _ => return None,
            };
            let asset_class = AssetClass::parse(parts.get(2)?)?;
            let mut params = MoveParams::defaults(mode);
            // Measured over a year of the trade-weighted dollar (Q-MK7): a
            // currency never moves 2 % a day, its floor is lower.
            if asset_class == AssetClass::Fx && mode == MoveMode::LogReturn {
                params.floor = FX_FLOOR;
            }
            let num = |i: usize| -> Option<Option<f64>> {
                match parts.get(i) {
                    None => Some(None),
                    Some(s) => s.parse::<f64>().ok().filter(|v| *v > 0.0).map(Some),
                }
            };
            if let Some(k) = num(3)? {
                params.k = k;
            }
            if let Some(floor) = num(4)? {
                params.floor = floor;
            }
            if code.is_empty() || parts.len() > 5 {
                return None;
            }
            Some(WatchedSeries {
                code,
                mode,
                asset_class,
                params,
            })
        })
        .collect()
}

/// Unique key of a move event: one per series, session and mode (D735).
pub fn event_ref(code: &str, date: NaiveDate, mode: MoveMode) -> String {
    format!("move:{code}:{date}:{}", mode.as_str())
}

/// How a series moved around `event`, in the event-study windows
/// (`market_study::WINDOWS`), for a series without a benchmark (a macro
/// series): the change from the session before the window to its last
/// session, a simple return in `LogReturn` mode, a difference in `Diff`.
/// `abnormal` is that same change: nothing explains it away (D743).
pub fn series_windows(
    series: &[(NaiveDate, f64)],
    mode: MoveMode,
    event: NaiveDate,
) -> Option<Vec<crate::market_study::WindowResult>> {
    let mut points: Vec<(NaiveDate, f64)> = series
        .iter()
        .copied()
        .filter(|(_, v)| v.is_finite())
        .collect();
    points.sort_by_key(|(d, _)| *d);
    points.dedup_by_key(|(d, _)| *d);
    let zero = points.iter().position(|(d, _)| *d >= event)? as i32;
    let windows: Vec<_> = crate::market_study::WINDOWS
        .iter()
        .filter_map(|&(from, to)| {
            let (a, b) = (zero + from - 1, zero + to);
            if a < 0 || b as usize >= points.len() {
                return None;
            }
            let (p0, p1) = (points[a as usize].1, points[b as usize].1);
            let raw = match mode {
                MoveMode::LogReturn if p0 > 0.0 => p1 / p0 - 1.0,
                MoveMode::LogReturn => return None,
                MoveMode::Diff => p1 - p0,
                // A volume has no price window to measure.
                MoveMode::Volume => return None,
            };
            Some(crate::market_study::WindowResult {
                from,
                to,
                raw,
                abnormal: raw,
            })
        })
        .collect();
    (!windows.is_empty()).then_some(windows)
}

/// The days "in the same window" as `date` (D742): one session before and
/// after, so a Monday reaches back to Friday and a Friday on to Monday.
// shortcut: weekends only, holidays not known; upgrade with a session calendar.
pub fn session_window(date: NaiveDate) -> (NaiveDate, NaiveDate) {
    use chrono::{Datelike, Duration, Weekday};
    let back = match date.weekday() {
        Weekday::Mon => 3,
        Weekday::Sun => 2,
        _ => 1,
    };
    let ahead = match date.weekday() {
        Weekday::Fri => 3,
        Weekday::Sat => 2,
        _ => 1,
    };
    (date - Duration::days(back), date + Duration::days(ahead))
}

/// A move of the session view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SessionMove {
    /// Object id of the `market_event`.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub title: String,
    #[cfg_attr(feature = "openapi", schema(format = "date"))]
    pub event_at: String,
    pub series_code: String,
    pub asset_class: String,
    pub move_direction: String,
    pub mode: String,
    pub measure: f64,
    pub sigma: f64,
    /// |measure| / sigma: the order of the view.
    pub score: f64,
    pub backfilled: bool,
}

/// A world event of the same window: shown as such, never as a cause.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SessionWorldEvent {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub title: String,
    #[cfg_attr(feature = "openapi", schema(format = "date"))]
    pub event_at: String,
    pub kind: String,
    pub country: String,
    /// `unconfirmed` or `confirmed`; dismissed events are left out.
    pub status: String,
}

/// The moves and world events of the window around one session (D738,
/// D742): computed at reading time, nothing is stored between them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MarketSession {
    #[cfg_attr(feature = "openapi", schema(format = "date"))]
    pub date: String,
    #[cfg_attr(feature = "openapi", schema(format = "date"))]
    pub window_from: String,
    #[cfg_attr(feature = "openapi", schema(format = "date"))]
    pub window_to: String,
    /// Moves of the window, `date` first, then by score.
    pub moves: Vec<SessionMove>,
    pub world_events: Vec<SessionWorldEvent>,
}

/// A world event placed on the general map (D739).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WorldEventPoint {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub title: String,
    pub kind: String,
    /// `unconfirmed` or `confirmed`; dismissed events are left out.
    pub status: String,
    #[cfg_attr(feature = "openapi", schema(format = "date"))]
    pub event_at: String,
    pub lon: f64,
    pub lat: f64,
}

/// Calendar days that cover the longest measure window (20 sessions).
pub const CLUSTER_DAYS: i64 = 28;

/// Dates closer than `CLUSTER_DAYS` to the previous kept one count once: the
/// same shock coded several days must not count several times (D743).
/// Returns the kept dates, oldest first, and how many were merged.
pub fn cluster(dates: &[NaiveDate]) -> (Vec<NaiveDate>, usize) {
    let mut sorted = dates.to_vec();
    sorted.sort();
    sorted.dedup();
    let mut kept: Vec<NaiveDate> = Vec::new();
    for d in &sorted {
        if kept
            .last()
            .is_none_or(|k| (*d - *k).num_days() >= CLUSTER_DAYS)
        {
            kept.push(*d);
        }
    }
    let merged = sorted.len() - kept.len();
    (kept, merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_volume_spike_is_an_up_move_and_a_quiet_day_is_not() {
        let d0 = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let mut series: Vec<(NaiveDate, f64)> = (0..100)
            .map(|i| {
                (
                    d0 + chrono::Duration::days(i),
                    1_000.0 + 50.0 * ((i % 7) as f64),
                )
            })
            .collect();
        series.push((d0 + chrono::Duration::days(100), 5_000.0));
        series.push((d0 + chrono::Duration::days(101), 100.0));
        let got = detect(
            &series,
            MoveMode::Volume,
            &MoveParams::defaults(MoveMode::Volume),
        );
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].date, d0 + chrono::Duration::days(100));
        assert_eq!(got[0].direction, MoveDirection::Up);
        assert!(
            (got[0].value.exp() - 4.0).abs() < 0.5,
            "about four times the usual volume"
        );
        assert_eq!(
            watched_series("SPY:volume:equity_index")[0].mode,
            MoveMode::Volume
        );
    }

    #[test]
    fn the_setting_reads_entries_and_overrides() {
        let got = watched_series(
            "sp500:log_return:equity_index, VIXCLS:diff:volatility:4:2 ; bad:x:rates, DGS10:diff, X:diff:rates:-1",
        );
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].code, "SP500");
        assert_eq!(got[0].params, MoveParams::defaults(MoveMode::LogReturn));
        assert_eq!(got[1].asset_class, AssetClass::Volatility);
        assert_eq!((got[1].params.k, got[1].params.floor), (4.0, 2.0));
        let fx = watched_series("DTWEXBGS:log_return:fx, X:log_return:fx:3:0.01");
        assert_eq!(fx[0].params.floor, FX_FLOOR);
        assert_eq!(fx[1].params.floor, 0.01, "an explicit floor wins");
        // D888: a BIS series is read by its own import, watched the same way.
        let bis = watched_series("bis:WS_CBPOL.D.US:diff:rates:1:0.05, bis:X");
        assert_eq!(bis.len(), 1);
        assert_eq!(bis[0].code, "WS_CBPOL.D.US");
        assert_eq!((bis[0].params.k, bis[0].params.floor), (1.0, 0.05));
        assert_eq!(
            crate::ontology::import::PerObjectSetting::codes(
                "DGS10:diff:rates, BIS:WS_CBPOL.D.US:diff:rates"
            ),
            vec!["DGS10"],
            "the FRED import never asks FRED for a BIS series"
        );
    }

    #[test]
    fn a_policy_rate_decision_is_a_move_and_a_flat_rate_none() {
        // A step series: flat, then one 25 bp cut.
        let mut s: Vec<_> = (0..200).map(|i| (day(i), 4.0)).collect();
        s.extend((200..260).map(|i| (day(i), 3.75)));
        let w = &watched_series("bis:WS_CBPOL.D.US:diff:rates:1:0.05")[0];
        let got = detect(&s, w.mode, &w.params);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].date, day(200));
        assert_eq!(got[0].direction, MoveDirection::Down);
        let flat: Vec<_> = (0..260).map(|i| (day(i), 4.0)).collect();
        assert!(detect(&flat, w.mode, &w.params).is_empty());
    }

    #[test]
    fn series_windows_measure_from_the_session_before() {
        let s: Vec<_> = (0..30).map(|i| (day(i), 100.0 + i as f64)).collect();
        let w = series_windows(&s, MoveMode::Diff, day(5)).unwrap();
        let first = w.iter().find(|w| (w.from, w.to) == (-1, 1)).unwrap();
        // From day 3 (before day 4) to day 6.
        assert!((first.raw - 3.0).abs() < 1e-12);
        let five = w.iter().find(|w| (w.from, w.to) == (0, 5)).unwrap();
        assert!((five.raw - 6.0).abs() < 1e-12);
        let w = series_windows(&s, MoveMode::LogReturn, day(5)).unwrap();
        let first = w.iter().find(|w| (w.from, w.to) == (-1, 1)).unwrap();
        assert!((first.raw - (106.0 / 103.0 - 1.0)).abs() < 1e-12);
        assert!(series_windows(&s, MoveMode::Diff, day(40)).is_none());
    }

    #[test]
    fn the_window_steps_over_weekends() {
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        // 2026-11-05 is a Thursday, 11-06 a Friday, 11-09 a Monday.
        assert_eq!(
            session_window(d("2026-11-05")),
            (d("2026-11-04"), d("2026-11-06"))
        );
        assert_eq!(
            session_window(d("2026-11-06")),
            (d("2026-11-05"), d("2026-11-09"))
        );
        assert_eq!(
            session_window(d("2026-11-09")),
            (d("2026-11-06"), d("2026-11-10"))
        );
    }

    #[test]
    fn close_dates_count_once() {
        let (kept, merged) = cluster(&[day(40), day(0), day(3), day(27), day(28), day(0)]);
        assert_eq!(kept, vec![day(0), day(28)]);
        assert_eq!(merged, 3);
    }

    fn day(i: i64) -> NaiveDate {
        NaiveDate::from_ymd_opt(2025, 1, 1).unwrap() + chrono::Duration::days(i)
    }

    /// A deterministic zig-zag: alternating changes of ±`step`.
    fn zigzag(n: usize, start: f64, step: f64, mode: MoveMode) -> Vec<(NaiveDate, f64)> {
        let mut p = start;
        (0..n)
            .map(|i| {
                if i > 0 {
                    let s = if i % 2 == 0 { step } else { -step };
                    p = match mode {
                        MoveMode::LogReturn | MoveMode::Volume => p * s.exp(),
                        MoveMode::Diff => p + s,
                    };
                }
                (day(i as i64), p)
            })
            .collect()
    }

    #[test]
    fn a_crash_is_found_once_and_signed() {
        let mode = MoveMode::LogReturn;
        let mut s = zigzag(200, 100.0, 0.005, mode);
        let last = s.last().unwrap().1;
        s.push((day(200), last * (-0.055f64).exp()));
        let moves = detect(&s, mode, &MoveParams::defaults(mode));
        assert_eq!(moves.len(), 1);
        let m = &moves[0];
        assert_eq!(m.date, day(200));
        assert_eq!(m.direction, MoveDirection::Down);
        assert!((m.value + 0.055).abs() < 1e-9);
        assert!((m.sigma - 0.005).abs() < 1e-4);
        assert!(
            (m.threshold - 0.02).abs() < 1e-12,
            "floor wins over 3 sigma"
        );
    }

    #[test]
    fn floor_keeps_a_flat_series_quiet() {
        // 1 % is 20 sigma here, but under the 2 % floor.
        let mode = MoveMode::LogReturn;
        let mut s = zigzag(100, 50.0, 0.0005, mode);
        let last = s.last().unwrap().1;
        s.push((day(100), last * 1.01));
        assert!(detect(&s, mode, &MoveParams::defaults(mode)).is_empty());
    }

    #[test]
    fn sigma_wins_on_a_volatile_series() {
        // sigma 3 %: a 5 % move is over the floor but under 3 sigma.
        let mode = MoveMode::LogReturn;
        let mut s = zigzag(100, 50.0, 0.03, mode);
        let last = s.last().unwrap().1;
        s.push((day(100), last * 0.05f64.exp()));
        assert!(detect(&s, mode, &MoveParams::defaults(mode)).is_empty());
    }

    #[test]
    fn rates_are_measured_in_difference() {
        // A 10-year rate at 4 % rising 25 bp in a day, after 2 bp swings.
        let mode = MoveMode::Diff;
        let mut s = zigzag(120, 4.0, 0.02, mode);
        let last = s.last().unwrap().1;
        s.push((day(120), last + 0.25));
        let moves = detect(&s, mode, &MoveParams::defaults(mode));
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].direction, MoveDirection::Up);
        assert!((moves[0].value - 0.25).abs() < 1e-9);
    }

    #[test]
    fn short_history_detects_nothing() {
        let mode = MoveMode::LogReturn;
        let mut s = zigzag(50, 100.0, 0.005, mode);
        s.push((day(50), 50.0));
        assert!(detect(&s, mode, &MoveParams::defaults(mode)).is_empty());
    }

    #[test]
    fn gaps_are_skipped_not_interpolated() {
        // Drop every third day: the crash is still one change from the last
        // quoted day, and order of input does not matter.
        let mode = MoveMode::LogReturn;
        let mut s: Vec<_> = zigzag(200, 100.0, 0.005, mode)
            .into_iter()
            .enumerate()
            .filter(|(i, _)| i % 3 != 1)
            .map(|(_, p)| p)
            .collect();
        let last = s.last().unwrap().1;
        s.push((day(205), last * 0.9));
        s.reverse();
        let moves = detect(&s, mode, &MoveParams::defaults(mode));
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].date, day(205));
        assert!((moves[0].value - 0.9f64.ln()).abs() < 1e-9);
    }

    #[test]
    fn the_tested_day_is_out_of_its_own_sigma() {
        // The crash itself must not widen the sigma it is tested against.
        let mode = MoveMode::LogReturn;
        let mut s = zigzag(100, 100.0, 0.005, mode);
        let last = s.last().unwrap().1;
        s.push((day(100), last * 0.9));
        let moves = detect(&s, mode, &MoveParams::defaults(mode));
        assert_eq!(moves.len(), 1);
        assert!((moves[0].sigma - 0.005).abs() < 1e-4);
    }

    #[test]
    fn bad_values_do_not_poison_the_series() {
        let mode = MoveMode::LogReturn;
        let mut s = zigzag(100, 100.0, 0.005, mode);
        s.push((day(100), f64::NAN));
        s.push((day(101), 0.0));
        assert!(detect(&s, mode, &MoveParams::defaults(mode)).is_empty());
    }
}
