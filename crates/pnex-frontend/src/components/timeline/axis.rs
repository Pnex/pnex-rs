//! Time axis of every timeline (cases, Vision Lab, media analyses): the
//! window, its scale (compacted over long empty stretches), ruler ticks
//! aligned on calendar or media time, label stacking and overflow, and the
//! clock texts of a [`TimeBase`].

use dioxus_i18n::t;

use crate::components::transport::media_clock;

pub const HOUR_MS: i64 = 3_600_000;
pub const DAY_MS: i64 = 24 * HOUR_MS;
pub const YEAR_MS: i64 = 365 * DAY_MS;

/// Window zoomed by `factor` (< 1 zooms in) around `center`, at least a
/// minute and at most two centuries wide.
pub fn zoom(window: (i64, i64), center: i64, factor: f64) -> (i64, i64) {
    zoom_min(window, center, factor, 60_000)
}

/// [`zoom`] down to `min` ms wide (a short video zooms to the second).
pub fn zoom_min(window: (i64, i64), center: i64, factor: f64, min: i64) -> (i64, i64) {
    let width =
        (((window.1 - window.0) as f64) * factor).clamp(min as f64, 200.0 * YEAR_MS as f64) as i64;
    let k = ((center - window.0) as f64 / (window.1 - window.0).max(1) as f64).clamp(0.0, 1.0);
    let from = center - (width as f64 * k) as i64;
    (from, from + width)
}

/// CSS that keeps a label inside the timeline: past 80 % of the width it is
/// drawn to the left of its point.
pub fn anchor(left_pct: f64) -> &'static str {
    if left_pct > 80.0 {
        "transform: translateX(-100%);"
    } else {
        ""
    }
}

/// Row of each item so that labels never overlap: items sorted by start,
/// each takes the first row free at its start. `extent` is the item's
/// width in percent of the window (bar + label).
pub fn rows(starts: &[(f64, f64)]) -> (Vec<usize>, usize) {
    let mut order: Vec<usize> = (0..starts.len()).collect();
    order.sort_by(|a, b| starts[*a].0.total_cmp(&starts[*b].0));
    let mut ends: Vec<f64> = Vec::new();
    let mut out = vec![0; starts.len()];
    for i in order {
        let (x, w) = starts[i];
        let r = ends.iter().position(|e| *e <= x).unwrap_or_else(|| {
            ends.push(f64::MIN);
            ends.len() - 1
        });
        ends[r] = x + w;
        out[i] = r;
    }
    (out, ends.len().max(1))
}

/// Tick step (ms) giving about `target` ticks over the window.
pub fn tick_step(window: (i64, i64), target: i64) -> i64 {
    const STEPS: [i64; 20] = [
        1_000,
        5_000,
        15_000,
        60_000,
        300_000,
        900_000,
        1_800_000,
        HOUR_MS,
        3 * HOUR_MS,
        6 * HOUR_MS,
        12 * HOUR_MS,
        DAY_MS,
        7 * DAY_MS,
        30 * DAY_MS,
        90 * DAY_MS,
        YEAR_MS,
        5 * YEAR_MS,
        10 * YEAR_MS,
        25 * YEAR_MS,
        100 * YEAR_MS,
    ];
    let ideal = (window.1 - window.0) / target.max(1);
    STEPS
        .iter()
        .copied()
        .find(|s| *s >= ideal)
        .unwrap_or(STEPS[STEPS.len() - 1])
}

/// Months per tick for steps of a month or more (calendar ticks).
fn months_of(step: i64) -> Option<i64> {
    (step >= 28 * DAY_MS).then(|| ((step as f64) / (30.44 * DAY_MS as f64)).round().max(1.0) as i64)
}

/// Ticks of the window at `step`, aligned on the step (UTC); from a month
/// up, on the first of a month (years on 1 January), never drifting.
pub fn ticks(window: (i64, i64), step: i64) -> Vec<i64> {
    use chrono::Datelike;
    if let Some(m) = months_of(step) {
        let d = chrono::DateTime::from_timestamp_millis(window.0).unwrap_or_default();
        let at = |idx: i64| {
            chrono::NaiveDate::from_ymd_opt(
                idx.div_euclid(12) as i32,
                idx.rem_euclid(12) as u32 + 1,
                1,
            )
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| d.and_utc().timestamp_millis())
        };
        let mut idx = (d.year() as i64 * 12 + d.month0() as i64).div_euclid(m) * m;
        while at(idx).is_some_and(|t| t < window.0) {
            idx += m;
        }
        return (0..)
            .map_while(|i| at(idx + i * m))
            .take_while(|t| *t <= window.1)
            .take(200)
            .collect();
    }
    let first = window.0.div_euclid(step) * step
        + if window.0.rem_euclid(step) == 0 {
            0
        } else {
            step
        };
    (0..)
        .map(|i| first + i * step)
        .take_while(|t| *t <= window.1)
        .take(200)
        .collect()
}

/// A stretch of the time axis: `[t0, t1]` drawn over `[p0, p1]` percent.
/// A folded stretch held nothing dated and is drawn as a narrow break.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stretch {
    pub t0: i64,
    pub t1: i64,
    pub p0: f64,
    pub p1: f64,
    pub folded: bool,
}

/// Piecewise-linear time axis of the window. Compacted, every empty stretch
/// between items longer than [`FOLD_MIN`] of the window, and the empty ends,
/// shrink to [`BREAK_PCT`]; the rest shares the width in proportion to its
/// duration.
#[derive(Debug, Clone, PartialEq)]
pub struct Scale {
    pub window: (i64, i64),
    pub stretches: Vec<Stretch>,
}

/// Width of a folded stretch, in percent of the timeline, by default.
pub const BREAK_PCT: f64 = 3.0;
/// An empty stretch folds past this share of the window.
const FOLD_MIN: f64 = 0.08;
/// Room kept around each dated item, as a share of the window.
const FOLD_PAD: f64 = 0.015;

impl Scale {
    pub fn linear(window: (i64, i64)) -> Self {
        Self {
            window,
            stretches: vec![Stretch {
                t0: window.0,
                t1: window.1,
                p0: 0.0,
                p1: 100.0,
                folded: false,
            }],
        }
    }

    /// The axis with the empty stretches between `items` folded, each
    /// drawn `break_pct` wide.
    pub fn compact(window: (i64, i64), items: &[(i64, i64)], break_pct: f64) -> Self {
        let dur = (window.1 - window.0).max(1);
        let pad = (dur as f64 * FOLD_PAD) as i64;
        let mut busy: Vec<(i64, i64)> = items
            .iter()
            .filter(|(a, b)| *b >= window.0 && *a <= window.1)
            .map(|(a, b)| ((a - pad).max(window.0), (b + pad).min(window.1)))
            .collect();
        busy.sort();
        let mut merged: Vec<(i64, i64)> = Vec::new();
        for (a, b) in busy {
            match merged.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        let mut pieces: Vec<(i64, i64, bool)> = Vec::new();
        let mut t = window.0;
        // Between two items, a long stretch folds; at an end of the window,
        // anything beyond the room kept around the items does.
        let fold = |a: i64, b: i64, edge: bool| {
            let min = if edge { 2.0 * FOLD_PAD } else { FOLD_MIN };
            (b - a) as f64 > dur as f64 * min
        };
        for (a, b) in merged {
            if a > t {
                pieces.push((t, a, fold(t, a, t == window.0)));
            }
            pieces.push((a, b, false));
            t = b;
        }
        if t < window.1 {
            pieces.push((t, window.1, fold(t, window.1, true)));
        }
        let folds = pieces.iter().filter(|p| p.2).count();
        let open: i64 = pieces.iter().filter(|p| !p.2).map(|p| p.1 - p.0).sum();
        if folds == 0 || open == 0 {
            return Self::linear(window);
        }
        let free = 100.0 - break_pct * folds as f64;
        if free <= 0.0 {
            return Self::linear(window);
        }
        let mut p = 0.0;
        let stretches = pieces
            .into_iter()
            .map(|(t0, t1, folded)| {
                let w = if folded {
                    break_pct
                } else {
                    free * (t1 - t0) as f64 / open as f64
                };
                let s = Stretch {
                    t0,
                    t1,
                    p0: p,
                    p1: p + w,
                    folded,
                };
                p += w;
                s
            })
            .collect();
        Self { window, stretches }
    }

    /// Position of `t`, in percent (outside the window: at the mean rate).
    pub fn pct(&self, t: i64) -> f64 {
        let rate = 100.0 / (self.window.1 - self.window.0).max(1) as f64;
        if t < self.window.0 {
            return (t - self.window.0) as f64 * rate;
        }
        if t > self.window.1 {
            return 100.0 + (t - self.window.1) as f64 * rate;
        }
        let s = self
            .stretches
            .iter()
            .find(|s| t <= s.t1)
            .unwrap_or(&self.stretches[self.stretches.len() - 1]);
        s.p0 + (t - s.t0) as f64 / (s.t1 - s.t0).max(1) as f64 * (s.p1 - s.p0)
    }

    /// Instant at `p` percent of the timeline (clamped to the window).
    pub fn time_at(&self, p: f64) -> i64 {
        let p = p.clamp(0.0, 100.0);
        let s = self
            .stretches
            .iter()
            .find(|s| p <= s.p1)
            .unwrap_or(&self.stretches[self.stretches.len() - 1]);
        s.t0 + ((p - s.p0) / (s.p1 - s.p0).max(1e-9) * (s.t1 - s.t0) as f64) as i64
    }

    /// Ticks of the unfolded stretches, each at a step fitting its width.
    pub fn ticks(&self, width_px: f64) -> Vec<(i64, i64)> {
        self.stretches
            .iter()
            .filter(|s| !s.folded)
            .flat_map(|s| {
                let px = (s.p1 - s.p0) / 100.0 * width_px;
                let step = tick_step((s.t0, s.t1), (px / 110.0).round().clamp(1.0, 10.0) as i64);
                ticks((s.t0, s.t1), step)
                    .into_iter()
                    .map(move |t| (t, step))
            })
            .collect()
    }
}

/// Items stacked past `max` rows, grouped where they sit (within `near`
/// percent of the first of the group): left of the group and its items.
pub fn overflow(rows: &[usize], xs: &[f64], max: usize, near: f64) -> Vec<(f64, Vec<usize>)> {
    let mut hidden: Vec<usize> = (0..rows.len()).filter(|i| rows[*i] >= max).collect();
    hidden.sort_by(|a, b| xs[*a].total_cmp(&xs[*b]));
    let mut groups: Vec<(f64, Vec<usize>)> = Vec::new();
    for i in hidden {
        match groups.last_mut() {
            Some((x, g)) if xs[i] - *x <= near => g.push(i),
            _ => groups.push((xs[i], vec![i])),
        }
    }
    groups
}

/// Rough length of a duration: a count and the fluent key of its unit.
pub fn rough_span(ms: i64) -> (i64, &'static str) {
    let ms = ms.max(0);
    if ms >= YEAR_MS {
        (
            ((ms as f64) / YEAR_MS as f64).round() as i64,
            "timeline-span-years",
        )
    } else if ms >= 30 * DAY_MS {
        (ms / (30 * DAY_MS), "timeline-span-months")
    } else if ms >= DAY_MS {
        (ms / DAY_MS, "timeline-span-days")
    } else if ms >= HOUR_MS {
        (ms / HOUR_MS, "timeline-span-hours")
    } else {
        ((ms / 60_000).max(1), "timeline-span-minutes")
    }
}

/// `datetime-local` value of an instant, in UTC or local time.
pub fn input_value(t: i64, local: bool) -> String {
    shifted(t, local).format("%Y-%m-%dT%H:%M").to_string()
}

/// Instant of a `datetime-local` value read in UTC or local time.
pub fn parse_input(s: &str, local: bool) -> Option<i64> {
    let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"))
        .ok()?;
    let t = naive.and_utc().timestamp_millis();
    Some(if local { t - local_offset(t) } else { t })
}

/// Offset of the browser's local time to UTC at `t`, in ms (0 off a browser).
pub fn local_offset(t: i64) -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(t as f64));
        -(d.get_timezone_offset() as i64) * 60_000
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = t;
        0
    }
}

fn shifted(t: i64, local: bool) -> chrono::DateTime<chrono::Utc> {
    let t = if local { t + local_offset(t) } else { t };
    chrono::DateTime::from_timestamp_millis(t).unwrap_or_default()
}

/// `HH:MM:SS` of an instant, in UTC or local time.
pub fn clock(t: i64, local: bool) -> String {
    shifted(t, local).format("%H:%M:%S").to_string()
}

/// `YYYY-MM-DD` of an instant, in UTC or local time.
pub fn day(t: i64, local: bool) -> String {
    shifted(t, local).format("%Y-%m-%d").to_string()
}

/// `YYYY-MM-DD HH:MM` of an instant, in UTC or local time.
pub fn stamp(t: i64, local: bool) -> String {
    shifted(t, local).format("%Y-%m-%d %H:%M").to_string()
}

/// Ruler label of a tick: the time under a day, the date under a year.
pub fn tick_label(t: i64, step: i64, local: bool) -> String {
    let d = shifted(t, local);
    let f = if step >= YEAR_MS {
        "%Y"
    } else if step >= 28 * DAY_MS {
        "%Y-%m"
    } else if step >= DAY_MS {
        "%Y-%m-%d"
    } else if step >= 60_000 {
        "%H:%M"
    } else {
        "%H:%M:%S"
    };
    d.format(f).to_string()
}

/// What the instants of a timeline count: wall-clock Unix ms (cases,
/// camera recordings), ms from the start of a media file, or a plain rank
/// (pictures without a date, one per second of axis).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimeBase {
    Wall { local: bool },
    Media,
    Rank,
}

impl TimeBase {
    /// Ruler label of a tick at `step`.
    pub fn tick_label(self, t: i64, step: i64) -> String {
        match self {
            Self::Wall { local } => tick_label(t, step, local),
            Self::Media => media_clock(t as f64 / 1000.0),
            Self::Rank => format!("{}", t / 1000 + 1),
        }
    }

    /// The cursor's time, big in the transport.
    pub fn clock(self, t: i64) -> String {
        match self {
            Self::Wall { local } => clock(t, local),
            Self::Media => media_clock(t as f64 / 1000.0),
            Self::Rank => format!("#{}", t / 1000 + 1),
        }
    }

    /// Text of an instant in a tooltip.
    pub fn stamp(self, t: i64) -> String {
        match self {
            Self::Wall { local } => stamp(t, local),
            _ => self.clock(t),
        }
    }

    /// Rough length of a folded stretch.
    pub fn span_text(self, ms: i64) -> String {
        match self {
            Self::Wall { .. } => {
                let (n, unit) = rough_span(ms);
                format!("≈{}", t!(unit, count : n))
            }
            _ => format!("≈{}", media_clock(ms as f64 / 1000.0)),
        }
    }
}

/// Window framing `spans`, padded by a tenth of their span and at least
/// `min_pad` (a "+N" chip zooms onto what it holds).
pub fn frame(spans: impl Iterator<Item = (i64, i64)>, min_pad: i64) -> Option<(i64, i64)> {
    let (mut a, mut b) = (i64::MAX, i64::MIN);
    for (f, t) in spans {
        a = a.min(f);
        b = b.max(t);
    }
    (a <= b).then(|| {
        let pad = ((b - a) / 10).max(min_pad);
        (a - pad, b + pad)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_and_rank_bases_label_from_zero() {
        assert_eq!(TimeBase::Media.tick_label(75_000, 15_000), "1:15");
        assert_eq!(TimeBase::Rank.clock(2_000), "#3");
        assert_eq!(zoom_min((0, 10_000), 5_000, 0.1, 2_000), (4_000, 6_000));
    }
}
