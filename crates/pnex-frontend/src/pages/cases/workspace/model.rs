//! Pure logic of the case workspace: instants of events, the visible
//! window and its time axis (compacted over long empty stretches), stacking
//! of labels and their overflow, calendar ruler ticks, positions at the
//! cursor.

use dioxus_i18n::t;
use pnex_core::ontology::api::{CaseEvent, CaseTrack, ObjectView, SignalItem};
use pnex_core::ontology::EVENT_OCCURRED;
use pnex_core::track::{sample, TrackPoint};
use serde_json::{json, Value};

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;
const YEAR_MS: i64 = 365 * DAY_MS;

/// Unix ms of an RFC 3339 instant.
pub fn ms(rfc3339: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|t| t.timestamp_millis())
}

/// RFC 3339 UTC (`Z`, whole seconds) of Unix ms.
pub fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

/// `(from, to)` of an event's `occurred`, `None` when undated.
pub fn span(event: &ObjectView) -> Option<(i64, i64)> {
    let v = event.properties.get(EVENT_OCCURRED)?;
    let from = ms(v.get("from")?.as_str()?)?;
    let to = v
        .get("to")
        .and_then(Value::as_str)
        .and_then(ms)
        .unwrap_or(from);
    Some((from, to.max(from)))
}

/// `fuzzy_time` of an instant, or of a period when `to > from`.
pub fn occurred(from: i64, to: i64) -> Value {
    if to > from {
        json!({"from": rfc3339(from), "to": rfc3339(to), "precision": "range"})
    } else {
        json!({"from": rfc3339(from), "to": rfc3339(from), "precision": "instant"})
    }
}

/// Window shown at first: the events padded by a tenth of their span and at
/// least an hour on each side; the last two hours (and the next ten minutes)
/// when nothing is dated.
pub fn initial_window(events: &[CaseEvent], now: i64) -> (i64, i64) {
    let spans: Vec<(i64, i64)> = events.iter().filter_map(|e| span(&e.event)).collect();
    let (Some(from), Some(to)) = (
        spans.iter().map(|s| s.0).min(),
        spans.iter().map(|s| s.1).max(),
    ) else {
        return (now - 2 * HOUR_MS, now + HOUR_MS / 6);
    };
    let pad = ((to - from) / 10).max(HOUR_MS);
    (from - pad, to + pad)
}

/// Window zoomed by `factor` (< 1 zooms in) around `center`, at least a
/// minute and at most two centuries wide.
pub fn zoom(window: (i64, i64), center: i64, factor: f64) -> (i64, i64) {
    let width =
        (((window.1 - window.0) as f64) * factor).clamp(60_000.0, 200.0 * YEAR_MS as f64) as i64;
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
            "cw-span-years",
        )
    } else if ms >= 30 * DAY_MS {
        (ms / (30 * DAY_MS), "cw-span-months")
    } else if ms >= DAY_MS {
        (ms / DAY_MS, "cw-span-days")
    } else if ms >= HOUR_MS {
        (ms / HOUR_MS, "cw-span-hours")
    } else {
        ((ms / 60_000).max(1), "cw-span-minutes")
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

/// Short text of a raw signal, as the timeline and the inspector show it.
pub fn signal_title(s: &SignalItem) -> String {
    if let Some(n) = &s.notification {
        return n.subject.clone().unwrap_or_default();
    }
    if let Some(r) = &s.range {
        return r.label.clone();
    }
    if let Some(l) = &s.link {
        return format!("{} → {}", l.source.title, l.target.title);
    }
    if let Some(c) = &s.change {
        let title = s.title.clone().unwrap_or_default();
        return match c.action.as_str() {
            "object.create" => t!("cw-sig-object-create", title : title),
            "object.update" => t!("cw-sig-object-update", title : title),
            "object.archive" => t!("cw-sig-object-archive", title : title),
            "object.purge" => t!("cw-sig-object-purge"),
            other => other.into(),
        }
        .to_string();
    }
    s.title.clone().unwrap_or_else(|| s.kind.clone())
}

/// Label of a group of like signals (`count` > 1): "127 objects created".
pub fn group_title(s: &SignalItem, count: usize) -> String {
    match s.change.as_ref().map(|c| c.action.as_str()) {
        Some("object.create") => t!("cw-sig-object-create-n", count : count).to_string(),
        Some("object.update") => t!("cw-sig-object-update-n", count : count).to_string(),
        Some("object.archive") => t!("cw-sig-object-archive-n", count : count).to_string(),
        _ => format!("{} ×{count}", signal_title(s)),
    }
}

/// Like signals close in time gathered: same kind and action, each within
/// a minute of the group's first. Order kept; each with its count.
pub fn group_signals(items: &[SignalItem]) -> Vec<(SignalItem, usize)> {
    let key = |s: &SignalItem| (s.kind.clone(), s.change.as_ref().map(|c| c.action.clone()));
    let mut out: Vec<(SignalItem, usize, i64)> = Vec::new();
    for s in items {
        let t = ms(&s.at).unwrap_or_default();
        match out.last_mut() {
            Some((g, n, t0))
                if key(g) == key(s) && (t - *t0).abs() <= 60_000 && s.change.is_some() =>
            {
                *n += 1
            }
            _ => out.push((s.clone(), 1, t)),
        }
    }
    out.into_iter().map(|(s, n, _)| (s, n)).collect()
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

/// Track points of a member, by gap-free segment.
pub fn segments(track: &CaseTrack) -> Vec<Vec<TrackPoint>> {
    track
        .segments
        .iter()
        .map(|seg| {
            seg.iter()
                .map(|(lon, lat, alt, dt)| TrackPoint {
                    t_ms: track.t0_ms + dt,
                    lon: *lon,
                    lat: *lat,
                    alt: *alt,
                })
                .collect()
        })
        .collect()
}

/// Position `(lon, lat)` of a member at `t`; `None` in a signal gap.
pub fn position(track: &CaseTrack, t: i64) -> Option<(f64, f64)> {
    segments(track).iter().find_map(|seg| sample(seg, t))
}

/// Covered spans `(from, to)` of a track (one per segment).
pub fn coverage(track: &CaseTrack) -> Vec<(i64, i64)> {
    track
        .segments
        .iter()
        .filter_map(|seg| Some((track.t0_ms + seg.first()?.3, track.t0_ms + seg.last()?.3)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::ontology::api::ObjectRef;

    fn event(occurred: Option<Value>) -> CaseEvent {
        let mut properties = serde_json::Map::new();
        if let Some(v) = occurred {
            properties.insert(EVENT_OCCURRED.into(), v);
        }
        CaseEvent {
            event: ObjectView {
                id: "e".into(),
                type_key: "event".into(),
                title: "E".into(),
                native_id: "e".into(),
                system: false,
                type_version: None,
                properties,
                source_ref: None,
                version: 1,
                valid_from: String::new(),
                valid_to: None,
                created_at: String::new(),
                updated_at: String::new(),
            },
            attachments: Vec::new(),
        }
    }

    #[test]
    fn occurred_round_trips_instants_and_periods() {
        let t = ms("2026-10-10T14:32:00Z").unwrap();
        assert_eq!(span(&event(Some(occurred(t, t))).event).unwrap(), (t, t));
        assert_eq!(
            span(&event(Some(occurred(t, t + 60_000))).event).unwrap(),
            (t, t + 60_000)
        );
        assert_eq!(occurred(t, t)["precision"], "instant");
        assert!(span(&event(None).event).is_none());
    }

    #[test]
    fn initial_window_pads_events_or_shows_the_last_hours() {
        let t = ms("2026-10-10T14:00:00Z").unwrap();
        let w = initial_window(&[event(Some(occurred(t, t)))], 0);
        assert_eq!(w, (t - HOUR_MS, t + HOUR_MS));
        assert_eq!(
            initial_window(&[event(None)], 10 * HOUR_MS),
            (8 * HOUR_MS, 10 * HOUR_MS + HOUR_MS / 6)
        );
    }

    #[test]
    fn zoom_keeps_the_center_in_place() {
        let w = zoom((0, 1_000_000), 250_000, 0.5);
        assert_eq!(w, (125_000, 625_000));
        assert!((Scale::linear(w).pct(250_000) - 25.0).abs() < 1e-9);
        // Never narrower than a minute.
        assert_eq!(
            zoom((0, 100_000), 0, 0.01).1 - zoom((0, 100_000), 0, 0.01).0,
            60_000
        );
    }

    #[test]
    fn rows_stack_overlapping_labels_only() {
        let (r, n) = rows(&[(0.0, 30.0), (10.0, 5.0), (40.0, 10.0), (12.0, 2.0)]);
        assert_eq!((r, n), (vec![0, 1, 0, 2], 3));
    }

    #[test]
    fn ticks_are_aligned_on_the_step() {
        let w = (
            ms("2026-10-10T13:20:00Z").unwrap(),
            ms("2026-10-10T16:00:00Z").unwrap(),
        );
        let step = tick_step(w, 8);
        assert_eq!(step, 1_800_000);
        assert_eq!(rfc3339(ticks(w, step)[0]), "2026-10-10T13:30:00Z");
    }

    #[test]
    fn long_windows_tick_on_calendar_boundaries() {
        let w = (
            ms("2005-03-10T00:00:00Z").unwrap(),
            ms("2026-06-01T00:00:00Z").unwrap(),
        );
        let step = tick_step(w, 8);
        assert_eq!(step, 5 * YEAR_MS);
        let t: Vec<String> = ticks(w, step).into_iter().map(rfc3339).collect();
        assert_eq!(
            t,
            [
                "2010-01-01T00:00:00Z",
                "2015-01-01T00:00:00Z",
                "2020-01-01T00:00:00Z",
                "2025-01-01T00:00:00Z"
            ]
        );
        assert_eq!(
            tick_label(ms("2026-04-01T00:00:00Z").unwrap(), 90 * DAY_MS, false),
            "2026-04"
        );
        // Twenty years still zoom out.
        assert!(zoom(w, w.0, 1.5).1 - zoom(w, w.0, 1.5).0 > 30 * YEAR_MS);
    }

    #[test]
    fn compact_axis_folds_long_empty_stretches_only() {
        let y = YEAR_MS;
        let w = (0, 20 * y);
        // Busy in year 1 and year 19: the 17 years between fold, and the
        // empty year at each end.
        let s = Scale::compact(w, &[(y, y + DAY_MS), (19 * y, 19 * y)], BREAK_PCT);
        let folded: Vec<_> = s.stretches.iter().filter(|x| x.folded).collect();
        assert_eq!(folded.len(), 3, "{s:?}");
        assert!(folded.iter().all(|f| f.p1 - f.p0 == BREAK_PCT));
        assert!((s.stretches.last().unwrap().p1 - 100.0).abs() < 1e-9);
        // Each busy stretch gets almost half the width (about 3 % when linear).
        assert!(s.stretches[1].p1 - s.stretches[1].p0 > 40.0, "{s:?}");
        // With the fit's 10 % margins, the ends fold too.
        let fit = (-2 * y, 22 * y);
        let s2 = Scale::compact(fit, &[(0, 0), (20 * y, 20 * y)], BREAK_PCT);
        assert_eq!(
            s2.stretches.iter().filter(|x| x.folded).count(),
            3,
            "{s2:?}"
        );
        // pct and time_at invert each other off the breaks.
        for t in [y, y + DAY_MS / 2, 19 * y] {
            assert!((s.time_at(s.pct(t)) - t).abs() < 60_000, "{t}");
        }
        // Spread out: nothing folds, the axis stays linear.
        let even: Vec<(i64, i64)> = (0..40).map(|i| (i * y / 2, i * y / 2)).collect();
        assert_eq!(Scale::compact(w, &even, BREAK_PCT), Scale::linear(w));
        assert_eq!(Scale::compact(w, &[], BREAK_PCT), Scale::linear(w));
    }

    #[test]
    fn overflow_groups_what_does_not_fit() {
        let rows = [0, 1, 2, 2, 0, 3];
        let xs = [10.0, 10.5, 11.0, 50.0, 60.0, 11.5];
        let g = overflow(&rows, &xs, 2, 4.0);
        assert_eq!(g, vec![(11.0, vec![2, 5]), (50.0, vec![3])]);
    }

    #[test]
    fn datetime_inputs_round_trip_in_utc() {
        let t = ms("2026-10-12T08:45:00Z").unwrap();
        assert_eq!(input_value(t, false), "2026-10-12T08:45");
        assert_eq!(parse_input("2026-10-12T08:45", false), Some(t));
        assert_eq!(parse_input("nope", false), None);
        assert_eq!(rough_span(4 * YEAR_MS + 3 * DAY_MS), (4, "cw-span-years"));
        assert_eq!(rough_span(90 * 60_000), (1, "cw-span-hours"));
    }

    #[test]
    fn like_signals_close_in_time_gather() {
        let sig = |at: &str, action: &str| SignalItem {
            at: at.into(),
            kind: "change".into(),
            change: Some(pnex_core::ontology::api::ChangeView {
                at: at.into(),
                action: action.into(),
                source_ref: String::new(),
                before: Value::Null,
                after: Value::Null,
            }),
            ..Default::default()
        };
        let items = [
            sig("2026-10-12T15:52:00Z", "object.create"),
            sig("2026-10-12T15:52:20Z", "object.create"),
            sig("2026-10-12T15:52:40Z", "object.create"),
            sig("2026-10-12T15:52:50Z", "object.update"),
            sig("2026-10-12T16:10:00Z", "object.create"),
        ];
        let g: Vec<usize> = group_signals(&items).iter().map(|(_, n)| *n).collect();
        assert_eq!(g, [3, 1, 1]);
    }

    #[test]
    fn position_is_none_in_a_gap() {
        let track = CaseTrack {
            object: ObjectRef {
                id: "a".into(),
                type_key: "device".into(),
                title: "A".into(),
                native_id: "1".into(),
            },
            t0_ms: 1_000,
            segments: vec![
                vec![(2.0, 48.0, None, 0), (2.2, 48.2, None, 10_000)],
                vec![(3.0, 49.0, None, 60_000)],
            ],
            raw_points: 3,
        };
        assert_eq!(position(&track, 6_000), Some((2.1, 48.1)));
        assert_eq!(position(&track, 30_000), None);
        assert_eq!(coverage(&track), vec![(1_000, 11_000), (61_000, 61_000)]);
    }
}
