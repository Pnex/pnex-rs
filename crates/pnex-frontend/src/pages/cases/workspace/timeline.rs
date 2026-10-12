//! The timeline, always on top (cases-workspace.md K23): the ruler with the
//! density of what happens, the events (click = instant, drag = period,
//! drag an event = move it), the raw signals, one lane per tracked member
//! (covered spans, signal gaps), and the cursor across every lane. Long
//! empty stretches fold into breaks (compact axis); labels past a few rows
//! gather into "+N" chips that zoom onto what they hold.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{CaseProposal, CaseTrack, SignalItem};
use pnex_core::ontology::EVENT_OCCURRED;

use super::{model, Sel, Target, Ws};
use crate::components::icons;
use crate::components::ui::BTN_SECONDARY_SM;

/// Element id of the lanes column: pointer x → instant goes through its rect.
const CANVAS: &str = "cw-frise";
/// Element id of the folded timeline's strip.
const MINI: &str = "cw-mini";
const RULER_H: f64 = 36.0;
const ROW_H: f64 = 26.0;
const LANE_H: f64 = 30.0;
/// Label rows shown before the rest gathers into "+N" chips.
const MAX_EVENT_ROWS: usize = 6;
const MAX_SIGNAL_ROWS: usize = 4;
/// Items within this many percent of each other share one chip.
const CHIP_NEAR: f64 = 3.0;
/// Categorical colours of the tracked members (distinct from state colours).
pub(super) const ACTOR_COLORS: [&str; 8] = [
    "#5b4fc4", "#a3478f", "#7a6a12", "#9a4d1c", "#3d6b2f", "#6b4f8a", "#8a3b3b", "#2f5f7a",
];

/// Colour of a tracked member, by its order in the tracks.
pub(super) fn actor_color(i: usize) -> &'static str {
    ACTOR_COLORS[i % ACTOR_COLORS.len()]
}

/// Approximate width of a label, in percent of a timeline `width_px` wide.
fn label_pct(title: &str, width_px: f64) -> f64 {
    (title.chars().count().min(32) as f64 * 7.0 + 30.0) / width_px * 100.0
}

/// A pointer gesture inside the lanes.
#[derive(Clone, PartialEq)]
enum Gesture {
    Scrub,
    New {
        t0: i64,
        t1: i64,
    },
    Move {
        id: String,
        x0: f64,
        from: i64,
        to: i64,
        shift: i64,
    },
}

fn pointer_time(scale: &model::Scale, x: f64) -> Option<i64> {
    let (l, _, w, _) = crate::util::element_rect(CANVAS)?;
    Some(scale.time_at((x - l) / w.max(1.0) * 100.0))
}

/// Window framing `spans`, padded, for a "+N" chip.
fn frame(spans: impl Iterator<Item = (i64, i64)>) -> Option<(i64, i64)> {
    let (mut a, mut b) = (i64::MAX, i64::MIN);
    for (f, t) in spans {
        a = a.min(f);
        b = b.max(t);
    }
    (a <= b).then(|| {
        let pad = ((b - a) / 10).max(60_000);
        (a - pad, b + pad)
    })
}

#[component]
pub(super) fn Timeline() -> Element {
    let ws = use_context::<Ws>();
    let mut gesture = use_signal(|| None::<Gesture>);
    let Some(w) = (ws.data)() else {
        return rsx! {};
    };
    if !(ws.dock_open)() {
        return rsx! {
            MiniBar {}
        };
    }
    let win = (ws.window)();
    let cursor = (ws.cursor)();
    let local = (ws.local_time)();
    let follow = (ws.follow)();
    // Width of the lanes once laid out (labels and ticks adapt to it).
    let width_px = crate::util::element_rect(CANVAS)
        .map_or(1000.0, |r| r.2)
        .max(200.0);
    let tracks: Vec<CaseTrack> = (ws.tracks)().map(|t| t.tracks).unwrap_or_default();
    let tracks_available = (ws.tracks)().is_none_or(|t| t.available);
    // Raw signals on demand; pending detections are always drawn.
    let show_signals = (ws.show_signals)();
    let raw: Vec<SignalItem> = if show_signals {
        (ws.signals)().map(|s| s.items).unwrap_or_default()
    } else {
        Vec::new()
    };
    // Like signals close in time drawn as one ("127 objects created").
    let grouped = model::group_signals(&raw);
    let counts: Vec<usize> = grouped.iter().map(|g| g.1).collect();
    let signals: Vec<SignalItem> = grouped.into_iter().map(|g| g.0).collect();
    let sig_label = |i: usize, s: &SignalItem| {
        if counts[i] > 1 {
            model::group_title(s, counts[i])
        } else {
            model::signal_title(s)
        }
    };
    let pending: Vec<CaseProposal> = (ws.proposals)()
        .into_iter()
        .filter(|p| p.status == "proposed")
        .collect();

    // Time axis: long empty stretches between events fold when compact.
    let scale = if (ws.compact)() {
        let items: Vec<(i64, i64)> = w
            .events
            .iter()
            .filter_map(|e| model::span(&e.event))
            .chain(pending.iter().filter_map(|p| {
                let a = model::ms(&p.t_start)?;
                Some((a, p.t_end.as_deref().and_then(model::ms).unwrap_or(a)))
            }))
            .collect();
        // A break wide enough for its label, never a large share on a phone.
        let break_pct = (56.0 / width_px * 100.0).clamp(model::BREAK_PCT, 8.0);
        model::Scale::compact(win, &items, break_pct)
    } else {
        model::Scale::linear(win)
    };
    let pct = |t: i64| scale.pct(t);

    // Events on the window, stacked so labels never overlap.
    let events: Vec<_> = w
        .events
        .iter()
        .filter(|e| {
            follow
                .as_ref()
                .is_none_or(|f| e.attachments.iter().any(|a| &a.object.id == f))
        })
        .filter_map(|e| Some((e, model::span(&e.event)?)))
        // Only what the window shows (outside, it would spill over).
        .filter(|(_, (from, to))| *to >= win.0 && *from <= win.1)
        .collect();
    let starts: Vec<(f64, f64)> = events
        .iter()
        .map(|(e, (from, to))| {
            let x = pct(*from);
            (
                x,
                (pct(*to) - x).max(0.0) + label_pct(&e.event.title, width_px),
            )
        })
        .collect();
    let (rows, n_rows) = model::rows(&starts);
    let xs: Vec<f64> = starts.iter().map(|s| s.0).collect();
    let event_chips = model::overflow(&rows, &xs, MAX_EVENT_ROWS, CHIP_NEAR);
    let shown_rows = n_rows.min(MAX_EVENT_ROWS) + usize::from(!event_chips.is_empty());
    let events_h = shown_rows as f64 * ROW_H + 10.0;

    let sig_starts: Vec<(f64, f64)> = signals
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            Some((
                pct(model::ms(&s.at)?),
                label_pct(&sig_label(i, s), width_px),
            ))
        })
        .chain(pending.iter().filter_map(|p| {
            Some((
                pct(model::ms(&p.t_start)?),
                label_pct(&super::proposal_title(p), width_px),
            ))
        }))
        .collect();
    let (sig_rows, n_sig) = model::rows(&sig_starts);
    // Rows of the proposals come after those of the signals with a date.
    let n_dated = signals
        .iter()
        .filter(|s| model::ms(&s.at).is_some())
        .count();
    let sig_xs: Vec<f64> = sig_starts.iter().map(|s| s.0).collect();
    let signal_chips = model::overflow(&sig_rows, &sig_xs, MAX_SIGNAL_ROWS, CHIP_NEAR);
    let sig_shown = n_sig.min(MAX_SIGNAL_ROWS) + usize::from(!signal_chips.is_empty());
    let signals_h = sig_shown as f64 * 24.0 + 8.0;
    // Instants of the signal-line items, in `sig_starts` order.
    let sig_times: Vec<i64> = signals
        .iter()
        .filter_map(|s| model::ms(&s.at))
        .chain(pending.iter().filter_map(|p| model::ms(&p.t_start)))
        .collect();

    let lanes: Vec<(usize, &CaseTrack)> = tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| follow.as_ref().is_none_or(|f| &t.object.id == f))
        .collect();
    let lanes_h = if lanes.is_empty() {
        LANE_H
    } else {
        lanes.len() as f64 * LANE_H
    };

    // Ruler ticks (per unfolded stretch) and the density of what happens.
    let breaks: Vec<model::Stretch> = scale
        .stretches
        .iter()
        .filter(|s| s.folded)
        .copied()
        .collect();
    // A tick too close to the previous one, or to a break, would overlap it.
    let min_gap = 80.0 / width_px * 100.0;
    let mut ticks: Vec<(i64, i64)> = Vec::new();
    for (t, step) in scale.ticks(width_px) {
        let p = scale.pct(t);
        let near_break = breaks
            .iter()
            .any(|b| p > b.p0 - min_gap && p < b.p1 + min_gap * 0.25);
        let near_prev = ticks
            .last()
            .is_some_and(|(q, _)| p - scale.pct(*q) < min_gap);
        if !near_break && !near_prev {
            ticks.push((t, step));
        }
    }
    let too_wide = super::Ws::too_wide(win);
    let mut bins = [0u32; 60];
    for t in events
        .iter()
        .map(|(_, s)| s.0)
        .chain(signals.iter().filter_map(|s| model::ms(&s.at)))
    {
        let p = pct(t);
        if (0.0..100.0).contains(&p) {
            bins[(p / 100.0 * 60.0) as usize] += 1;
        }
    }
    let max_bin = bins.iter().copied().max().unwrap_or(0).max(1) as f64;

    let sc = scale.clone();
    let on_down = move |e: PointerEvent| {
        if e.trigger_button() != Some(dioxus::html::input_data::MouseButton::Primary) {
            return;
        }
        let Some(t) = pointer_time(&sc, e.client_coordinates().x) else {
            return;
        };
        let mut cur = ws.cursor;
        cur.set(t);
        gesture.set(Some(Gesture::Scrub));
    };
    let sc = scale.clone();
    let on_move = move |e: PointerEvent| {
        let Some(t) = pointer_time(&sc, e.client_coordinates().x) else {
            return;
        };
        match gesture() {
            Some(Gesture::Scrub) => {
                let mut cur = ws.cursor;
                cur.set(t);
            }
            Some(Gesture::New { t0, .. }) => gesture.set(Some(Gesture::New { t0, t1: t })),
            Some(Gesture::Move {
                id, x0, from, to, ..
            }) => {
                let shift = t - pointer_time(&sc, x0).unwrap_or(t);
                gesture.set(Some(Gesture::Move {
                    id,
                    x0,
                    from,
                    to,
                    shift,
                }));
            }
            None => {}
        }
    };
    let sc = scale.clone();
    let on_up = move |_| {
        let g = gesture();
        gesture.set(None);
        match g {
            Some(Gesture::New { t0, t1 }) if ws.can_write => {
                let (a, b) = (t0.min(t1), t0.max(t1));
                // Less than 0.6 % of the width: a click, an instant.
                let period = sc.pct(b) - sc.pct(a) > 0.6;
                let title = if period {
                    t!("cw-new-period")
                } else {
                    t!("cw-new-event")
                };
                ws.create_event(title.to_string(), a, if period { b } else { a }, Vec::new());
            }
            Some(Gesture::Move {
                id,
                from,
                to,
                shift,
                ..
            }) => {
                if (sc.pct(from + shift) - sc.pct(from)).abs() <= 0.3 {
                    ws.select_event(id);
                    return;
                }
                if !ws.can_write {
                    return;
                }
                let Some(ev) = ws
                    .data
                    .peek()
                    .as_ref()
                    .and_then(|w| w.events.iter().find(|e| e.event.id == id).cloned())
                else {
                    return;
                };
                let mut props = ev.event.properties.clone();
                props.insert(
                    EVENT_OCCURRED.into(),
                    model::occurred(from + shift, to + shift),
                );
                let mut cursor = ws.cursor;
                cursor.set(from + shift);
                ws.save_object(ev.event.id, ev.event.title, props, ev.event.version);
            }
            _ => {}
        }
    };

    let sc = scale.clone();
    let lane_down = move |e: PointerEvent| {
        if !ws.can_write {
            return;
        }
        if let Some(t) = pointer_time(&sc, e.client_coordinates().x) {
            gesture.set(Some(Gesture::New { t0: t, t1: t }));
        }
    };
    let sc = scale.clone();
    let lane_move = move |e: PointerEvent| {
        if (ws.drag)().is_some() {
            let mut target = ws.target;
            if let Some(t) = pointer_time(&sc, e.client_coordinates().x) {
                target.set(Some(Target::Lane { at: t, actor: None }));
            }
        }
    };

    rsx! {
        section {
            class: "shrink-0 border-t border-line-strong bg-surface",
            "aria-label": t!("cw-timeline"),
            Toolbar {}
            div { class: "grid max-h-[42vh] grid-cols-[9rem_minmax(0,1fr)] overflow-y-auto sm:grid-cols-[11rem_minmax(0,1fr)]",
                // Labels column.
                div { class: "border-r border-line text-xs",
                    div {
                        class: "flex items-center px-2 border-b border-line bg-gray-50 text-slate",
                        style: "height: {RULER_H}px",
                        if local {
                            {t!("cw-local-time")}
                        } else {
                            "UTC"
                        }
                    }
                    div {
                        class: "flex items-center gap-2 px-2 border-b border-line font-medium text-ink",
                        style: "height: {events_h}px",
                        {t!("cw-events")}
                        span { class: "ml-auto font-mono text-slate", "{events.len()}" }
                    }
                    div {
                        class: "flex items-center gap-2 px-2 border-b border-line text-slate",
                        style: "height: {signals_h}px",
                        if show_signals {
                            {t!("cw-signals")}
                        } else {
                            {t!("cw-detections")}
                        }
                        span { class: "ml-auto font-mono", "{signals.len() + pending.len()}" }
                    }
                    if lanes.is_empty() {
                        div {
                            class: "flex items-center px-2 text-slate",
                            style: "height: {LANE_H}px",
                            {t!("cw-no-actor")}
                        }
                    }
                    for (i, track) in lanes.iter().cloned() {
                        button {
                            key: "{track.object.id}",
                            r#type: "button",
                            class: if follow.as_ref() == Some(&track.object.id) { "flex w-full items-center gap-2 px-2 border-b border-line bg-signal-soft text-left" } else { "flex w-full items-center gap-2 px-2 border-b border-line text-left hover:bg-gray-50" },
                            style: "height: {LANE_H}px",
                            title: t!("cw-follow"),
                            onclick: {
                                let id = track.object.id.clone();
                                move |_| {
                                    let mut f = ws.follow;
                                    let next = if f().as_ref() == Some(&id) { None } else { Some(id.clone()) };
                                    f.set(next);
                                }
                            },
                            span {
                                class: "h-2.5 w-2.5 shrink-0 rounded-full",
                                style: "background: {actor_color(i)}",
                            }
                            span { class: "truncate text-ink", "{track.object.title}" }
                        }
                    }
                }
                // Lanes column: one relative block, the cursor spans it.
                div {
                    id: CANVAS,
                    class: "relative select-none touch-none",
                    onpointermove: on_move,
                    onpointerup: on_up,
                    onpointerleave: move |_| gesture.set(None),
                    // Ruler.
                    div {
                        class: "relative border-b border-line bg-gray-50 cursor-ew-resize",
                        style: "height: {RULER_H}px",
                        onpointerdown: on_down,
                        for (t, step) in ticks.iter().copied() {
                            div {
                                key: "{t}",
                                class: "absolute top-0 h-6 border-l border-line-strong pl-1 pt-0.5 font-mono text-xs text-slate whitespace-nowrap",
                                style: "left: {pct(t)}%",
                                {model::tick_label(t, step, local)}
                            }
                        }
                        for b in breaks.iter().copied() {
                            BreakMark { key: "b{b.t0}", stretch: b, local }
                        }
                        svg {
                            class: "absolute inset-x-0 bottom-0 h-2.5 w-full",
                            view_box: "0 0 60 10",
                            preserve_aspect_ratio: "none",
                            "aria-hidden": "true",
                            for (i, c) in bins.iter().copied().enumerate() {
                                if c > 0 {
                                    rect {
                                        key: "{i}",
                                        x: "{i as f64 + 0.1}",
                                        y: "{10.0 - c as f64 / max_bin * 9.0}",
                                        width: "0.8",
                                        height: "{c as f64 / max_bin * 9.0}",
                                        fill: "var(--color-slate, #4c5966)",
                                        opacity: "0.35",
                                    }
                                }
                            }
                        }
                    }
                    // Events.
                    div {
                        class: "relative border-b border-line",
                        class: if ws.can_write { "cursor-crosshair" },
                        "data-testid": "case-events-lane",
                        style: "height: {events_h}px",
                        onpointerdown: lane_down,
                        onpointermove: lane_move,
                        if events.is_empty() {
                            p { class: "absolute left-3 top-2 text-xs text-slate pointer-events-none",
                                {t!("cw-events-empty")}
                            }
                        }
                        for (x, group) in event_chips.iter().cloned() {
                            MoreChip {
                                key: "e{x}",
                                left: x,
                                top: 5.0 + MAX_EVENT_ROWS as f64 * ROW_H,
                                count: group.len(),
                                frame: frame(group.iter().map(|i| events[*i].1)),
                            }
                        }
                        if let Some(Gesture::New { t0, t1 }) = gesture() {
                            div {
                                class: "absolute top-1.5 h-3 rounded-sm border border-dashed border-signal bg-signal-soft pointer-events-none",
                                style: "left: {pct(t0.min(t1))}%; width: {pct(t0.max(t1)) - pct(t0.min(t1))}%",
                            }
                        }
                        for (i, (e, (from, to))) in events.iter().enumerate().filter(|(i, _)| rows[*i] < MAX_EVENT_ROWS) {
                            div { key: "{e.event.id}", class: "contents",
                                if to > from {
                                    div {
                                        class: "pointer-events-none absolute h-3 rounded-sm border",
                                        class: if (ws.sel)() == Sel::Event(e.event.id.clone()) { "border-signal bg-signal-soft" } else { "border-ink bg-gray-100" },
                                        style: "left: {pct(*from + moved(&gesture(), &e.event.id))}%; width: max(6px, {pct(*to) - pct(*from)}%); top: {9.0 + rows[i] as f64 * ROW_H}px",
                                    }
                                }
                                EventMark {
                                    id: e.event.id.clone(),
                                    title: e.event.title.clone(),
                                    count: e.attachments.len(),
                                    left: pct(if to > from { *to } else { *from } + moved(&gesture(), &e.event.id)),
                                    period: to > from,
                                    top: 5.0 + rows[i] as f64 * ROW_H,
                                    future: *from > cursor,
                                    detection: e.event.properties.get("origin").and_then(|v| v.as_str()) == Some("detection"),
                                    on_grab: move |(id, x, from, to): (String, f64, i64, i64)| {
                                        gesture
                                            .set(
                                                Some(Gesture::Move {
                                                    id,
                                                    x0: x,
                                                    from,
                                                    to,
                                                    shift: 0,
                                                }),
                                            );
                                    },
                                    from: *from,
                                    to: *to,
                                }
                            }
                        }
                    }
                    // Raw signals.
                    div {
                        class: "relative border-b border-line bg-gray-50",
                        "data-testid": "case-signals-lane",
                        style: "height: {signals_h}px",
                        for (i, s) in signals
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| sig_rows.get(*i).is_some_and(|r| *r < MAX_SIGNAL_ROWS))
                        {
                            if let Some(at) = model::ms(&s.at) {
                                button {
                                    key: "{i}",
                                    r#type: "button",
                                    class: "absolute flex max-w-48 -translate-x-1 items-center gap-1 rounded border border-dashed border-line-strong bg-surface px-1.5 text-xs leading-5 text-slate hover:border-signal",
                                    class: if at > cursor { "opacity-40" },
                                    style: "left: {pct(at)}%; top: {4.0 + sig_rows[i] as f64 * 24.0}px; {model::anchor(pct(at))}",
                                    title: "{s.kind} · {model::stamp(at, local)}",
                                    onpointerdown: move |e: PointerEvent| e.stop_propagation(),
                                    onclick: move |_| {
                                        let (mut cur, mut sel) = (ws.cursor, ws.sel);
                                        cur.set(at);
                                        sel.set(Sel::None);
                                    },
                                    span { class: "truncate", {sig_label(i, s)} }
                                }
                            }
                        }
                        for (k, p) in pending
                            .iter()
                            .enumerate()
                            .filter(|(k, _)| {
                                sig_rows.get(n_dated + *k).is_some_and(|r| *r < MAX_SIGNAL_ROWS)
                            })
                        {
                            if let Some(at) = model::ms(&p.t_start) {
                                ProposalChip {
                                    key: "{p.id}",
                                    title: super::proposal_title(p),
                                    hint: p.detector.clone(),
                                    at,
                                    left: pct(at),
                                    top: 4.0 + sig_rows[n_dated + k] as f64 * 24.0,
                                }
                            }
                        }
                        for (x, group) in signal_chips.iter().cloned() {
                            MoreChip {
                                key: "s{x}",
                                left: x,
                                top: 4.0 + MAX_SIGNAL_ROWS as f64 * 24.0,
                                count: group.len(),
                                frame: frame(group.iter().filter_map(|i| sig_times.get(*i).map(|t| (*t, *t)))),
                            }
                        }
                    }
                    // Tracked members.
                    if lanes.is_empty() {
                        div {
                            class: "flex items-center overflow-hidden whitespace-nowrap px-3 text-xs text-slate",
                            style: "height: {LANE_H}px",
                            if too_wide {
                                {t!("cw-tracks-too-wide")}
                            } else if tracks_available {
                                {t!("cw-no-actor-help")}
                            } else {
                                {t!("cw-tracks-unavailable")}
                            }
                        }
                    }
                    for (i, track) in lanes.iter().cloned() {
                        Lane {
                            key: "{track.object.id}",
                            scale: scale.clone(),
                            track: track.clone(),
                            color: actor_color(i),
                            marks: w.events
                                .iter()
                                .filter(|e| e.attachments.iter().any(|a| a.object.id == track.object.id))
                                .filter_map(|e| model::span(&e.event).map(|s| s.0))
                                .collect::<Vec<_>>(),
                        }
                    }
                    // Breaks across the lanes, under the items.
                    for b in breaks.iter().copied() {
                        div {
                            key: "x{b.t0}",
                            class: "pointer-events-none absolute",
                            style: "left: {b.p0}%; width: {b.p1 - b.p0}%; top: {RULER_H}px; height: {events_h + signals_h + lanes_h}px; background: repeating-linear-gradient(90deg, transparent 0 3px, color-mix(in srgb, var(--color-line-strong, #c3cbd3) 45%, transparent) 3px 4px)",
                        }
                    }
                    // Cursor.
                    div {
                        class: "pointer-events-none absolute top-0 bottom-0 w-0.5 bg-signal",
                        style: "left: {pct(cursor)}%; height: {RULER_H + events_h + signals_h + lanes_h}px",
                    }
                }
            }
        }
    }
}

/// Shift being applied to `id` by a move gesture (0 otherwise).
fn moved(g: &Option<Gesture>, id: &str) -> i64 {
    match g {
        Some(Gesture::Move { id: m, shift, .. }) if m == id => *shift,
        _ => 0,
    }
}

#[component]
fn EventMark(
    id: String,
    title: String,
    count: usize,
    left: f64,
    period: bool,
    top: f64,
    future: bool,
    detection: bool,
    from: i64,
    to: i64,
    on_grab: EventHandler<(String, f64, i64, i64)>,
) -> Element {
    let ws = use_context::<Ws>();
    let selected = (ws.sel)() == Sel::Event(id.clone());
    let targeted = (ws.target)() == Some(Target::Event(id.clone()));
    let grab_id = id.clone();
    let hover_id = id.clone();
    rsx! {
        div {
            class: "absolute flex h-5 items-center gap-1.5 whitespace-nowrap text-xs cursor-grab",
            class: if future { "opacity-40" },
            style: "left: {left}%; top: {top}px; {model::anchor(left)}",
            "data-testid": "case-event",
            tabindex: 0,
            role: "button",
            "aria-label": "{title}",
            onpointerdown: move |e: PointerEvent| {
                e.stop_propagation();
                on_grab.call((grab_id.clone(), e.client_coordinates().x, from, to));
            },
            onpointermove: move |e: PointerEvent| {
                if (ws.drag)().is_some() {
                    e.stop_propagation();
                    let mut target = ws.target;
                    target.set(Some(Target::Event(hover_id.clone())));
                }
            },
            onkeydown: {
                let id = id.clone();
                move |e: KeyboardEvent| {
                    if e.key() == Key::Enter {
                        ws.select_event(id.clone())
                    }
                }
            },
            if !period {
                span {
                    class: "h-3 w-3 -translate-x-1.5 rotate-45 rounded-sm border-2",
                    class: if selected { "border-signal bg-signal" } else { "border-ink bg-surface" },
                    class: if detection { "border-dotted" },
                }
            }
            span {
                class: "relative rounded-md border bg-surface px-1.5 leading-5",
                class: if selected || targeted { "border-signal text-signal" } else { "border-line-strong text-ink" },
                class: if targeted { "ring-2 ring-signal" },
                class: if period { "ml-1.5" },
                "{title} "
                span { class: "font-mono text-slate", "{count}" }
            }
        }
    }
}

/// Lane of a tracked member: covered spans in its colour, gaps hatched,
/// marks where the case's events involve it; a drop creates an event with
/// the member attached.
#[component]
fn Lane(scale: model::Scale, track: CaseTrack, color: &'static str, marks: Vec<i64>) -> Element {
    let ws = use_context::<Ws>();
    let sc = scale.clone();
    let spans = model::coverage(&track);
    let gaps: Vec<(i64, i64)> = spans.windows(2).map(|p| (p[0].1, p[1].0)).collect();
    let actor = track.object.id.clone();
    rsx! {
        div {
            class: "relative border-b border-line",
            style: "height: {LANE_H}px",
            onpointermove: move |e: PointerEvent| {
                if (ws.drag)().is_some() {
                    let mut target = ws.target;
                    if let Some(t) = pointer_time(&sc, e.client_coordinates().x) {
                        target
                            .set(
                                Some(Target::Lane {
                                    at: t,
                                    actor: Some(actor.clone()),
                                }),
                            );
                    }
                }
            },
            for (i, (a, b)) in spans.iter().copied().enumerate() {
                div {
                    key: "s{i}",
                    class: "absolute top-1.5 h-4 rounded",
                    style: "left: {scale.pct(a)}%; width: max(2px, {scale.pct(b) - scale.pct(a)}%); background: color-mix(in srgb, {color} 28%, transparent); border: 1px solid color-mix(in srgb, {color} 60%, transparent)",
                }
            }
            for (i, (a, b)) in gaps.iter().copied().enumerate() {
                div {
                    key: "g{i}",
                    class: "absolute top-1.5 h-4 rounded text-xs leading-4 text-slate px-1 overflow-hidden whitespace-nowrap",
                    style: "left: {scale.pct(a)}%; width: {scale.pct(b) - scale.pct(a)}%; background: repeating-linear-gradient(135deg, transparent 0 4px, var(--color-line-strong, #c3cbd3) 4px 5px)",
                    title: t!("cw-signal-gap"),
                }
            }
            for (i, m) in marks.iter().copied().enumerate() {
                div {
                    key: "m{i}",
                    class: "pointer-events-none absolute top-1 h-5 w-0.5 bg-ink opacity-50",
                    style: "left: {scale.pct(m)}%",
                }
            }
        }
    }
}

/// A folded stretch on the ruler: its rough length, its dates on hover.
#[component]
fn BreakMark(stretch: model::Stretch, local: bool) -> Element {
    let (n, unit) = model::rough_span(stretch.t1 - stretch.t0);
    let length = format!("≈{}", t!(unit, count : n));
    let tip = format!(
        "{} → {}",
        model::stamp(stretch.t0, local),
        model::stamp(stretch.t1, local)
    );
    rsx! {
        div {
            class: "absolute inset-y-0 flex items-center justify-center overflow-hidden border-x border-dashed border-line-strong bg-gray-100 text-xs text-slate",
            style: "left: {stretch.p0}%; width: {stretch.p1 - stretch.p0}%",
            title: "{tip}",
            "data-testid": "case-break",
            span { class: "whitespace-nowrap px-0.5", "{length}" }
        }
    }
}

/// Items that did not fit: a click frames them.
#[component]
fn MoreChip(left: f64, top: f64, count: usize, frame: Option<(i64, i64)>) -> Element {
    let ws = use_context::<Ws>();
    rsx! {
        button {
            r#type: "button",
            class: "absolute -translate-x-1 rounded-full border border-line-strong bg-surface px-2 text-xs leading-5 text-ink hover:border-signal",
            style: "left: {left}%; top: {top}px; {model::anchor(left)}",
            title: t!("cw-more-help"),
            "data-testid": "case-more",
            onpointerdown: move |e: PointerEvent| e.stop_propagation(),
            onclick: move |_| {
                if let Some(f) = frame {
                    let mut window = ws.window;
                    window.set(f);
                }
            },
            "+{count}"
        }
    }
}

/// A pending detection on the signals line: solid border, the signal colour.
#[component]
fn ProposalChip(title: String, hint: String, at: i64, left: f64, top: f64) -> Element {
    let ws = use_context::<Ws>();
    let tip = format!(
        "{} · {}",
        t!("cw-detection-hint", detector : hint),
        model::stamp(at, (ws.local_time)())
    );
    rsx! {
        button {
            r#type: "button",
            class: "absolute flex max-w-48 -translate-x-1 items-center gap-1 rounded border border-signal bg-signal-soft px-1.5 text-xs leading-5 text-ink",
            style: "left: {left}%; top: {top}px; {model::anchor(left)}",
            title: "{tip}",
            "data-testid": "case-proposal",
            onpointerdown: move |e: PointerEvent| e.stop_propagation(),
            onclick: move |_| {
                let (mut cur, mut sel) = (ws.cursor, ws.sel);
                cur.set(at);
                sel.set(Sel::None);
            },
            span { class: "truncate", "{title}" }
        }
    }
}

/// Play / pause, speed, clock, time zone, window bounds, detections,
/// compact axis, zoom.
#[component]
fn Toolbar() -> Element {
    let ws = use_context::<Ws>();
    let (mut playing, mut speed, mut window, mut local, mut compact) =
        (ws.playing, ws.speed, ws.window, ws.local_time, ws.compact);
    let mut show_signals = ws.show_signals;
    let win = window();
    let too_wide = Ws::too_wide(win);
    // A typed bound moves that edge only, never past the other one.
    let set_from = move |e: FormEvent| {
        let w = window();
        if let Some(t) = model::parse_input(&e.value(), local()).filter(|t| *t < w.1) {
            window.set((t, w.1));
        }
    };
    let set_to = move |e: FormEvent| {
        let w = window();
        if let Some(t) = model::parse_input(&e.value(), local()).filter(|t| *t > w.0) {
            window.set((w.0, t));
        }
    };
    let cursor = (ws.cursor)();
    let zone = if local() {
        t!("cw-local-time").to_string()
    } else {
        "UTC".to_string()
    };
    // One height for every control of the bar.
    let field = "h-8 rounded-md border border-line-strong bg-surface px-2 text-sm text-ink";
    let fit_window = move |_| {
        let events = ws
            .data
            .peek()
            .as_ref()
            .map(|w| w.events.clone())
            .unwrap_or_default();
        window.set(model::initial_window(&events, super::now_ms()));
    };
    rsx! {
        div { class: "space-y-2 border-b border-line px-3 py-2",
            // Playback: play, speed, the cursor's time; then the detections.
            div { class: "flex flex-wrap items-center gap-2",
                button {
                    class: "{BTN_SECONDARY_SM} h-8 w-9",
                    r#type: "button",
                    "aria-label": if playing() { t!("cw-pause") } else { t!("cw-play") },
                    onclick: move |_| playing.set(!playing()),
                    if playing() {
                        "❚❚"
                    } else {
                        "▶"
                    }
                }
                select {
                    class: "{field} w-24",
                    "aria-label": t!("cw-speed"),
                    onchange: move |e| speed.set(e.value().parse().unwrap_or(3)),
                    for (v, l) in [(1, "×1"), (3, "×3"), (8, "×8")] {
                        option {
                            key: "{v}",
                            value: "{v}",
                            selected: speed() == v,
                            "{l}"
                        }
                    }
                    option { value: "0", selected: speed() == 0, {t!("cw-speed-fit")} }
                }
                span { class: "font-mono text-base font-medium text-ink",
                    {model::clock(cursor, local())}
                }
                span { class: "font-mono text-sm text-slate", {model::day(cursor, local())} }
                button {
                    class: "h-6 rounded border border-line px-1.5 text-xs text-slate hover:border-signal hover:text-signal",
                    r#type: "button",
                    title: t!("cw-zone-toggle"),
                    onclick: move |_| local.set(!local()),
                    "{zone}"
                }
                div { class: "ml-auto flex items-center gap-1",
                    DetectionsButton {}
                    button {
                        class: if show_signals() { "h-8 rounded-md border border-signal bg-signal-soft px-2.5 text-sm text-signal" } else { "h-8 rounded-md border border-line-strong bg-surface px-2.5 text-sm text-ink hover:bg-gray-50" },
                        r#type: "button",
                        "aria-pressed": show_signals(),
                        title: t!("cw-signals-help"),
                        "data-testid": "case-signals-toggle",
                        onclick: move |_| show_signals.set(!show_signals()),
                        {t!("cw-signals")}
                    }
                    if ws.can_write {
                        button {
                            class: "{BTN_SECONDARY_SM} h-8",
                            r#type: "button",
                            disabled: too_wide,
                            title: if too_wide { t!("cw-detect-too-wide") } else { t!("cw-detect-help") },
                            "data-testid": "case-detect",
                            onclick: move |_| ws.detect(),
                            {t!("cw-detect")}
                        }
                    }
                    button {
                        class: if compact() { "h-8 rounded-md border border-signal bg-signal-soft px-2.5 text-sm text-signal" } else { "h-8 rounded-md border border-line-strong bg-surface px-2.5 text-sm text-ink hover:bg-gray-50" },
                        r#type: "button",
                        "aria-pressed": compact(),
                        title: t!("cw-compact-help"),
                        "data-testid": "case-compact",
                        onclick: move |_| compact.set(!compact()),
                        {t!("cw-compact")}
                    }
                    button {
                        class: "{BTN_SECONDARY_SM} h-8 w-9",
                        r#type: "button",
                        "aria-expanded": "true",
                        "aria-label": t!("cw-dock-fold"),
                        title: t!("cw-dock-fold"),
                        "data-testid": "case-dock-toggle",
                        onclick: move |_| {
                            let mut d = ws.dock_open;
                            d.set(false);
                        },
                        icons::ChevronDown { class: "h-4 w-4" }
                    }
                }
            }
            // Window: its bounds; then zoom, fit, shortcuts.
            div { class: "flex flex-wrap items-center gap-2",
                span { class: "text-xs text-slate", {t!("cw-window")} }
                label { class: "sr-only", r#for: "cw-from", {t!("cw-window-from")} }
                input {
                    id: "cw-from",
                    class: "{field} font-mono",
                    r#type: "datetime-local",
                    "data-testid": "case-window-from",
                    value: model::input_value(win.0, local()),
                    onchange: set_from,
                }
                span { class: "text-slate", "→" }
                label { class: "sr-only", r#for: "cw-to", {t!("cw-window-to")} }
                input {
                    id: "cw-to",
                    class: "{field} font-mono",
                    r#type: "datetime-local",
                    "data-testid": "case-window-to",
                    value: model::input_value(win.1, local()),
                    onchange: set_to,
                }
                div { class: "ml-auto flex items-center gap-1",
                    button {
                        class: "{BTN_SECONDARY_SM} h-8 w-8",
                        r#type: "button",
                        "aria-label": t!("cw-zoom-out"),
                        onclick: move |_| window.set(model::zoom(window(), cursor, 1.5)),
                        "−"
                    }
                    button {
                        class: "{BTN_SECONDARY_SM} h-8 w-8",
                        r#type: "button",
                        "aria-label": t!("cw-zoom-in"),
                        onclick: move |_| window.set(model::zoom(window(), cursor, 0.66)),
                        "+"
                    }
                    button {
                        class: "{BTN_SECONDARY_SM} h-8",
                        r#type: "button",
                        onclick: fit_window,
                        {t!("cw-fit")}
                    }
                }
            }
        }
    }
}

/// Pending detections: a click shows them in the inspector, at the cursor.
#[component]
fn DetectionsButton() -> Element {
    let ws = use_context::<Ws>();
    let n = (ws.proposals)()
        .iter()
        .filter(|p| p.status == "proposed")
        .count();
    rsx! {
        button {
            class: if n > 0 { "h-8 rounded-md border border-signal bg-signal-soft px-2.5 text-sm text-signal" } else { "h-8 rounded-md border border-line-strong bg-surface px-2.5 text-sm text-slate hover:bg-gray-50" },
            r#type: "button",
            title: t!("cw-detections-open"),
            "data-testid": "case-detections-open",
            onclick: move |_| {
                let (mut at, mut sel) = (ws.at_cursor, ws.sel);
                sel.set(super::Sel::None);
                at.set(true);
            },
            {t!("cw-detections-n", count : n)}
        }
    }
}

/// The folded timeline: play, the cursor's time, a thin strip with the
/// events and the cursor (a click moves it), the detections, unfold.
#[component]
fn MiniBar() -> Element {
    let ws = use_context::<Ws>();
    let mut playing = ws.playing;
    let mut local = ws.local_time;
    let win = (ws.window)();
    let cursor = (ws.cursor)();
    let scale = model::Scale::linear(win);
    let marks: Vec<f64> = (ws.data)()
        .map(|w| {
            w.events
                .iter()
                .filter_map(|e| model::span(&e.event).map(|s| s.0))
                .filter(|t| *t >= win.0 && *t <= win.1)
                .map(|t| scale.pct(t))
                .collect()
        })
        .unwrap_or_default();
    let zone = if local() {
        t!("cw-local-time").to_string()
    } else {
        "UTC".to_string()
    };
    let seek = move |e: PointerEvent| {
        if let Some((l, _, w, _)) = crate::util::element_rect(MINI) {
            let p = (e.client_coordinates().x - l) / w.max(1.0) * 100.0;
            let mut cur = ws.cursor;
            cur.set(model::Scale::linear(*ws.window.peek()).time_at(p));
        }
    };
    rsx! {
        section {
            class: "flex h-12 shrink-0 items-center gap-2 border-t border-line-strong bg-surface px-3",
            "aria-label": t!("cw-timeline"),
            "data-testid": "case-minibar",
            button {
                class: "{BTN_SECONDARY_SM} h-8 w-9",
                r#type: "button",
                "aria-label": if playing() { t!("cw-pause") } else { t!("cw-play") },
                onclick: move |_| playing.set(!playing()),
                if playing() {
                    "❚❚"
                } else {
                    "▶"
                }
            }
            span { class: "font-mono text-base font-medium text-ink", {model::clock(cursor, local())} }
            span { class: "hidden font-mono text-sm text-slate sm:inline",
                {model::day(cursor, local())}
            }
            button {
                class: "h-6 rounded border border-line px-1.5 text-xs text-slate hover:border-signal hover:text-signal",
                r#type: "button",
                title: t!("cw-zone-toggle"),
                onclick: move |_| local.set(!local()),
                "{zone}"
            }
            div {
                id: MINI,
                class: "relative mx-2 h-8 min-w-24 flex-1 cursor-pointer",
                onpointerdown: seek,
                div { class: "absolute inset-x-0 top-1/2 h-0.5 -translate-y-1/2 bg-line-strong" }
                div {
                    class: "absolute left-0 top-1/2 h-0.5 -translate-y-1/2 bg-signal",
                    style: "width: {scale.pct(cursor).clamp(0.0, 100.0)}%",
                }
                for (i, m) in marks.iter().enumerate() {
                    span {
                        key: "{i}",
                        class: "absolute top-1/2 h-2.5 w-2.5 -translate-x-1/2 -translate-y-1/2 rotate-45 border-2 border-ink bg-surface",
                        style: "left: {m}%",
                    }
                }
                span {
                    class: "absolute top-0 h-8 w-0.5 bg-signal",
                    style: "left: {scale.pct(cursor).clamp(0.0, 100.0)}%",
                }
            }
            DetectionsButton {}
            button {
                class: "{BTN_SECONDARY_SM} h-8 w-9",
                r#type: "button",
                "aria-expanded": "false",
                "aria-label": t!("cw-dock-unfold"),
                title: t!("cw-dock-unfold"),
                "data-testid": "case-dock-toggle",
                onclick: move |_| {
                    let mut d = ws.dock_open;
                    d.set(true);
                },
                icons::ChevronUp { class: "h-4 w-4" }
            }
        }
    }
}
