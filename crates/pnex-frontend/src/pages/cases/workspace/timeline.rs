//! The timeline, always on top (cases-workspace.md K23), drawn by the
//! shared `components::timeline`: the ruler with the density of what
//! happens, the events (click = instant, drag = period, drag an event =
//! move it), the raw signals, one lane per tracked member (covered spans,
//! signal gaps), and the cursor across every lane. Long empty stretches
//! fold into breaks (compact axis); labels past a few rows gather into
//! "+N" chips that zoom onto what they hold.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{CaseTrack, SignalItem};
use pnex_core::ontology::EVENT_OCCURRED;

use super::{model, Sel, Target, Ws};
use crate::components::icons;
use crate::components::timeline::{Body, Hover, Item, ItemKind, Lane, TimeBase};
use crate::components::transport::{speed_options, Clock, PlayButton, ScrubBar, SpeedPicker};
use crate::components::ui::BTN_SECONDARY_SM;

/// Element id of the lanes column: pointer x → instant goes through its rect.
const CANVAS: &str = "cw-frise";
/// Label rows shown before the rest gathers into "+N" chips.
const MAX_EVENT_ROWS: usize = 12;
const MAX_SIGNAL_ROWS: usize = 4;
const EVENTS: &str = "events";
const SIGNALS: &str = "signals";
/// Categorical colours of the tracked members (distinct from state colours).
pub(super) const ACTOR_COLORS: [&str; 8] = [
    "#5b4fc4", "#a3478f", "#7a6a12", "#9a4d1c", "#3d6b2f", "#6b4f8a", "#8a3b3b", "#2f5f7a",
];

/// Colour of a tracked member, by its order in the tracks.
pub(super) fn actor_color(i: usize) -> &'static str {
    ACTOR_COLORS[i % ACTOR_COLORS.len()]
}

#[component]
pub(super) fn Timeline() -> Element {
    let ws = use_context::<Ws>();
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
    let sel = (ws.sel)();
    let target = (ws.target)();
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
    let pending: Vec<_> = (ws.proposals)()
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

    // Events on the window (outside, they would spill over).
    let events: Vec<Item> = w
        .events
        .iter()
        .filter(|e| {
            follow
                .as_ref()
                .is_none_or(|f| e.attachments.iter().any(|a| &a.object.id == f))
        })
        .filter_map(|e| {
            let (from, to) = model::span(&e.event)?;
            let id = e.event.id.clone();
            (to >= win.0 && from <= win.1).then(|| Item {
                selected: sel == Sel::Event(id.clone()),
                targeted: target == Some(Target::Event(id.clone())),
                id,
                from,
                to,
                label: e.event.title.clone(),
                badge: Some(e.attachments.len().to_string()),
                kind: if e.event.properties.get("origin").and_then(|v| v.as_str())
                    == Some("detection")
                {
                    ItemKind::Detected
                } else {
                    ItemKind::Mark
                },
                dim: from > cursor,
                movable: ws.can_write,
                ..Default::default()
            })
        })
        .collect();
    // Signal line: the (grouped) signals, then the pending detections.
    let mut marks: Vec<Item> = grouped
        .iter()
        .enumerate()
        .filter_map(|(i, (s, n))| {
            let at = model::ms(&s.at)?;
            Some(Item {
                id: format!("s{i}"),
                from: at,
                to: at,
                label: if *n > 1 {
                    model::group_title(s, *n)
                } else {
                    model::signal_title(s)
                },
                tip: Some(format!("{} · {}", s.kind, model::stamp(at, local))),
                kind: ItemKind::Signal,
                dim: at > cursor,
                ..Default::default()
            })
        })
        .collect();
    marks.extend(pending.iter().filter_map(|p| {
        let at = model::ms(&p.t_start)?;
        Some(Item {
            id: format!("p{}", p.id),
            from: at,
            to: at,
            label: super::proposal_title(p),
            tip: Some(format!(
                "{} · {}",
                t!("cw-detection-hint", detector : p.detector.clone()),
                model::stamp(at, local)
            )),
            kind: ItemKind::Proposal,
            ..Default::default()
        })
    }));
    let instants: Vec<(String, i64)> = marks.iter().map(|m| (m.id.clone(), m.from)).collect();
    let n_events = events.len();
    let n_marks = marks.len();

    let mut lanes = vec![
        Lane {
            count: Some(n_events),
            editable: ws.can_write,
            testid: Some("case-events-lane"),
            item_testid: "case-event",
            ..Lane::new(
                EVENTS,
                t!("cw-events"),
                Body::Items {
                    items: events,
                    max_rows: MAX_EVENT_ROWS,
                    stack: true,
                    empty: Some(t!("cw-events-empty").to_string()),
                },
            )
        },
        Lane {
            count: Some(n_marks),
            tinted: true,
            testid: Some("case-signals-lane"),
            ..Lane::new(
                SIGNALS,
                if show_signals {
                    t!("cw-signals")
                } else {
                    t!("cw-detections")
                },
                Body::Items {
                    items: marks,
                    max_rows: MAX_SIGNAL_ROWS,
                    stack: true,
                    empty: None,
                },
            )
        },
    ];
    let followed: Vec<(usize, &CaseTrack)> = tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| follow.as_ref().is_none_or(|f| &t.object.id == f))
        .collect();
    if followed.is_empty() {
        let note = if super::Ws::too_wide(win) {
            t!("cw-tracks-too-wide")
        } else if tracks_available {
            t!("cw-no-actor-help")
        } else {
            t!("cw-tracks-unavailable")
        };
        lanes.push(Lane {
            tinted: true,
            ..Lane::new("", t!("cw-no-actor"), Body::Note(note.to_string()))
        });
    }
    for (i, track) in followed {
        let id = track.object.id.clone();
        lanes.push(Lane {
            color: Some(actor_color(i).to_string()),
            active: follow.as_ref() == Some(&id),
            clickable: true,
            title_tip: Some(t!("cw-follow").to_string()),
            ..Lane::new(
                id.clone(),
                track.object.title.clone(),
                Body::Spans {
                    spans: model::coverage(track),
                    gaps: true,
                    marks: w
                        .events
                        .iter()
                        .filter(|e| e.attachments.iter().any(|a| a.object.id == id))
                        .filter_map(|e| model::span(&e.event).map(|s| s.0))
                        .collect(),
                },
            )
        });
    }

    let on_item = move |(lane, id): (String, String)| {
        if lane == EVENTS {
            ws.select_event(id);
        } else if let Some((_, at)) = instants.iter().find(|(i, _)| *i == id) {
            let (mut cur, mut sel) = (ws.cursor, ws.sel);
            cur.set(*at);
            sel.set(Sel::None);
        }
    };
    let on_create = move |(lane, a, b): (String, i64, i64)| {
        if lane != EVENTS || !ws.can_write {
            return;
        }
        let title = if b > a {
            t!("cw-new-period")
        } else {
            t!("cw-new-event")
        };
        ws.create_event(title.to_string(), a, b, Vec::new());
    };
    let on_move = move |(id, shift): (String, i64)| {
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
        let Some((from, to)) = model::span(&ev.event) else {
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
    };
    let on_hover = move |h: Hover| {
        let next = match (h.lane.as_str(), h.item) {
            (EVENTS, Some(id)) => Target::Event(id),
            (EVENTS, None) => Target::Lane {
                at: h.at,
                actor: None,
            },
            (SIGNALS | "", _) => return,
            (actor, _) => Target::Lane {
                at: h.at,
                actor: Some(actor.to_string()),
            },
        };
        let mut t = ws.target;
        t.set(Some(next));
    };

    rsx! {
        section {
            class: "shrink-0 border-t border-line-strong bg-surface",
            "aria-label": t!("cw-timeline"),
            Toolbar {}
            crate::components::timeline::Timeline {
                canvas: CANVAS,
                scale,
                base: TimeBase::Wall { local },
                cursor,
                lanes,
                header: if local { t!("cw-local-time").to_string() } else { "UTC".to_string() },
                dropping: (ws.drag)().is_some(),
                on_seek: move |t: i64| {
                    let mut cur = ws.cursor;
                    cur.set(t);
                },
                on_item,
                on_title: move |id: String| {
                    let mut f = ws.follow;
                    let next = if f().as_ref() == Some(&id) { None } else { Some(id) };
                    f.set(next);
                },
                on_create,
                on_move,
                on_hover,
                on_frame: move |f: (i64, i64)| {
                    let mut window = ws.window;
                    window.set(f);
                },
            }
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
    let mut speeds = speed_options(&[1, 3, 8]);
    speeds.push((0, t!("cw-speed-fit").to_string()));
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
                PlayButton {
                    playing: playing(),
                    on_toggle: move |_| playing.set(!playing()),
                }
                SpeedPicker {
                    value: speed() as u32,
                    options: speeds,
                    on_change: move |v: u32| speed.set(i64::from(v)),
                }
                Clock {
                    time: model::clock(cursor, local()),
                    sub: Some(model::day(cursor, local())),
                }
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
    let on_seek = move |p: f64| {
        let mut cur = ws.cursor;
        cur.set(model::Scale::linear(*ws.window.peek()).time_at(p));
    };
    rsx! {
        section {
            class: "flex h-12 shrink-0 items-center gap-2 border-t border-line-strong bg-surface px-3",
            "aria-label": t!("cw-timeline"),
            "data-testid": "case-minibar",
            PlayButton {
                playing: playing(),
                on_toggle: move |_| playing.set(!playing()),
            }
            Clock {
                time: model::clock(cursor, local()),
                sub: Some(model::day(cursor, local())),
            }
            button {
                class: "h-6 rounded border border-line px-1.5 text-xs text-slate hover:border-signal hover:text-signal",
                r#type: "button",
                title: t!("cw-zone-toggle"),
                onclick: move |_| local.set(!local()),
                "{zone}"
            }
            div { class: "mx-2 flex min-w-24 flex-1",
                ScrubBar { cursor: Some(scale.pct(cursor)), marks, on_seek }
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
