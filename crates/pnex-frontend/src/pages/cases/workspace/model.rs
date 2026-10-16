//! Pure logic of the case workspace: instants of events, their window,
//! signals and tracks. The time axis itself (scale, ticks, stacking) is the
//! shared one of `components::timeline`.

use dioxus_i18n::t;
use pnex_core::ontology::api::{CaseEvent, CaseTrack, ObjectView, SignalItem};
use pnex_core::ontology::EVENT_OCCURRED;
use pnex_core::track::{sample, TrackPoint};
use serde_json::{json, Value};

pub use crate::components::timeline::axis::*;

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

/// `s` cut to `n` characters, with an ellipsis when it was longer.
pub fn short(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}

/// Order of the storyline threads: the most present first, then each time
/// the thread that shares the most events with those already placed (the
/// latest weighing more), so threads that meet often run side by side.
/// `present[i]` = events of thread `i`; `meets` as in [`storyline`].
pub fn story_order(present: &[usize], meets: &[Vec<usize>]) -> Vec<usize> {
    let n = present.len();
    let mut together = vec![vec![0usize; n]; n];
    for on in meets {
        for a in on {
            for b in on {
                if a != b {
                    together[*a][*b] += 1;
                }
            }
        }
    }
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut left: Vec<usize> = (0..n).collect();
    while !left.is_empty() {
        let pick = left
            .iter()
            .copied()
            .max_by_key(|c| {
                let near: usize = order
                    .iter()
                    .rev()
                    .enumerate()
                    .map(|(age, p)| together[*c][*p] * 4 / (age + 1))
                    .sum();
                (near, present[*c], usize::MAX - *c)
            })
            .unwrap_or(left[0]);
        order.push(pick);
        left.retain(|c| *c != pick);
    }
    order
}

/// Storyline layout (xkcd "Movie Narrative Charts"): the y of each line at
/// each column. `meets[c]` lists the lines sharing event `c`: they gather
/// `tight` apart around their mean home row; the other lines step out of the
/// group's band. Home rows are `gap` apart, from 0.
pub fn storyline(lines: usize, meets: &[Vec<usize>], gap: f64, tight: f64) -> Vec<Vec<f64>> {
    let home: Vec<f64> = (0..lines).map(|i| i as f64 * gap).collect();
    meets
        .iter()
        .map(|on| {
            let mut y = home.clone();
            if on.len() < 2 {
                return y;
            }
            let mid = on.iter().map(|i| home[*i]).sum::<f64>() / on.len() as f64;
            let half = (on.len() - 1) as f64 / 2.0;
            for (j, i) in on.iter().enumerate() {
                y[*i] = mid + (j as f64 - half) * tight;
            }
            let lo = mid - (half + 1.0) * tight;
            let hi = mid + (half + 1.0) * tight;
            for (i, v) in y.iter_mut().enumerate() {
                if !on.contains(&i) && *v > lo && *v < hi {
                    *v = if *v < mid { lo } else { hi };
                }
            }
            y
        })
        .collect()
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

    #[test]
    fn story_order_keeps_companions_side_by_side() {
        // 0 is most present; 2 meets 0 often, 1 never meets anyone.
        let order = story_order(&[5, 4, 3], &[vec![0, 2], vec![0, 2], vec![1]]);
        assert_eq!(order, vec![0, 2, 1]);
    }

    #[test]
    fn storyline_gathers_the_lines_of_an_event() {
        let ys = storyline(4, &[vec![], vec![0, 3]], 40.0, 10.0);
        assert_eq!(ys[0], vec![0.0, 40.0, 80.0, 120.0]);
        let (a, b) = (ys[1][0], ys[1][3]);
        assert!((b - a - 10.0).abs() < 1e-9, "{a} {b}");
        // The lines in between step out of the group's band.
        for i in [1, 2] {
            assert!(
                ys[1][i] <= a - 10.0 + 1e-9 || ys[1][i] >= b + 10.0 - 1e-9,
                "{:?}",
                ys[1]
            );
        }
    }
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
        assert_eq!(
            rough_span(4 * YEAR_MS + 3 * DAY_MS),
            (4, "timeline-span-years")
        );
        assert_eq!(rough_span(90 * 60_000), (1, "timeline-span-hours"));
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
