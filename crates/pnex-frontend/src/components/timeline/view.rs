//! The timeline view: labels column, ruler, lanes, cursor. Gestures: press
//! the ruler (or a span lane) to scrub, drag on an editable lane to draw an
//! instant or a period, drag a movable mark to move it, click a mark to
//! select it, "+N" frames what did not fit.

use dioxus::prelude::*;

use super::axis::{self, Scale, TimeBase};

const RULER_H: f64 = 36.0;
const ROW_H: f64 = 26.0;
const SPAN_LANE_H: f64 = 30.0;
/// Widest label, in px: longer ones are cut with an ellipsis, so the
/// stacking estimate holds (labels never overlap).
const LABEL_MAX_PX: f64 = 224.0;
/// Items within this many percent of each other share one "+N" chip.
const CHIP_NEAR: f64 = 3.0;
/// Under this share of the width a drawn span is a click: an instant.
const CLICK_PCT: f64 = 0.6;

/// How an item is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ItemKind {
    /// Diamond (instant) or bar (period) with a framed label.
    #[default]
    Mark,
    /// A mark with a dotted diamond: proposed by a detector.
    Detected,
    /// Dashed chip: a raw signal.
    Signal,
    /// Accent chip: a pending proposal.
    Proposal,
}

/// One instant or period of a lane.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Item {
    pub id: String,
    pub from: i64,
    /// `to > from` is a period.
    pub to: i64,
    /// Empty: the mark alone (frames of a video).
    pub label: String,
    /// Small mono text after the label (a count).
    pub badge: Option<String>,
    pub tip: Option<String>,
    pub kind: ItemKind,
    /// Fill of the diamond when not selected (CSS colour).
    pub fill: Option<String>,
    pub selected: bool,
    /// Under an external drag (drop target).
    pub targeted: bool,
    /// Drawn faded (after the cursor).
    pub dim: bool,
    /// Can be dragged along the axis (`on_move`).
    pub movable: bool,
}

/// What a lane holds.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// Marks and chips, stacked in rows so labels never overlap (or on one
    /// row when `stack` is off), past `max_rows` gathered into "+N".
    Items {
        items: Vec<Item>,
        max_rows: usize,
        stack: bool,
        empty: Option<String>,
    },
    /// Covered spans in the lane's colour, the gaps between them hatched,
    /// thin marks at `marks`.
    Spans {
        spans: Vec<(i64, i64)>,
        gaps: bool,
        marks: Vec<i64>,
    },
    /// A line of text in place of data.
    Note(String),
}

/// A row of the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Lane {
    pub id: String,
    pub title: String,
    pub count: Option<usize>,
    /// Dot before the title, colour of the spans.
    pub color: Option<String>,
    /// Title highlighted (followed, filtered on).
    pub active: bool,
    /// Grey background (secondary data).
    pub tinted: bool,
    /// A drag on the lane draws an item (`on_create`).
    pub editable: bool,
    /// The title is a button (`on_title`).
    pub clickable: bool,
    pub title_tip: Option<String>,
    pub testid: Option<&'static str>,
    /// `data-testid` of its marks.
    pub item_testid: &'static str,
    pub body: Body,
}

impl Lane {
    pub fn new(id: impl Into<String>, title: impl Into<String>, body: Body) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            count: None,
            color: None,
            active: false,
            tinted: false,
            editable: false,
            clickable: false,
            title_tip: None,
            testid: None,
            item_testid: "timeline-mark",
            body,
        }
    }
}

/// Where an external drag hovers: a lane at an instant, or one of its items.
#[derive(Debug, Clone, PartialEq)]
pub struct Hover {
    pub lane: String,
    pub item: Option<String>,
    pub at: i64,
}

#[derive(Clone, PartialEq)]
enum Gesture {
    Scrub,
    New { lane: String, t0: i64, t1: i64 },
    Move { id: String, x0: f64, shift: i64 },
}

/// Layout of an items lane: row of each item, "+N" groups, height.
struct Stack {
    rows: Vec<usize>,
    chips: Vec<(f64, Vec<usize>)>,
    h: f64,
}

/// Approximate width of a label, in percent of a timeline `width_px` wide.
fn label_pct(title: &str, width_px: f64) -> f64 {
    ((title.chars().count() as f64 * 7.0).min(LABEL_MAX_PX) + 30.0) / width_px * 100.0
}

/// Extent of an item for the stacking (start, width), in percent: past
/// 80 % the label sits left of the mark (`axis::anchor`), so does its room.
fn extent(x: f64, bar: f64, label: f64) -> (f64, f64) {
    if x > 80.0 {
        (x - label, label + bar)
    } else {
        (x, bar + label)
    }
}

fn stack(scale: &Scale, items: &[Item], max_rows: usize, stacked: bool, width_px: f64) -> Stack {
    if !stacked {
        return Stack {
            rows: vec![0; items.len()],
            chips: Vec::new(),
            h: ROW_H + 10.0,
        };
    }
    let starts: Vec<(f64, f64)> = items
        .iter()
        .map(|it| {
            let x = scale.pct(it.from);
            let bar = if matches!(it.kind, ItemKind::Mark | ItemKind::Detected) {
                (scale.pct(it.to) - x).max(0.0)
            } else {
                0.0
            };
            extent(x, bar, label_pct(&it.label, width_px))
        })
        .collect();
    let (rows, n) = axis::rows(&starts);
    let xs: Vec<f64> = items.iter().map(|it| scale.pct(it.from)).collect();
    let chips = axis::overflow(&rows, &xs, max_rows, CHIP_NEAR);
    let shown = n.min(max_rows) + usize::from(!chips.is_empty());
    Stack {
        rows,
        chips,
        h: shown as f64 * ROW_H + 10.0,
    }
}

fn pointer_time(canvas: &str, scale: &Scale, x: f64) -> Option<i64> {
    let (l, _, w, _) = crate::util::element_rect(canvas)?;
    Some(scale.time_at((x - l) / w.max(1.0) * 100.0))
}

#[component]
pub fn Timeline(
    /// Element id of the lanes column (pointer x → instant).
    canvas: String,
    scale: Scale,
    base: TimeBase,
    cursor: i64,
    lanes: Vec<Lane>,
    /// Text above the labels column (time zone, unit).
    header: String,
    /// An external drag is in progress: lanes report `on_hover`.
    #[props(default)]
    dropping: bool,
    /// Density of what happens under the ticks (off when items are evenly
    /// spaced, like the frames of a video).
    #[props(default = true)]
    density: bool,
    on_seek: EventHandler<i64>,
    #[props(default)] on_item: EventHandler<(String, String)>,
    #[props(default)] on_title: EventHandler<String>,
    /// `(lane, from, to)`; `to == from` for a click.
    #[props(default)]
    on_create: EventHandler<(String, i64, i64)>,
    /// `(item, shift ms)`.
    #[props(default)]
    on_move: EventHandler<(String, i64)>,
    #[props(default)] on_hover: EventHandler<Hover>,
    /// A "+N" chip asks to frame `(from, to)`.
    #[props(default)]
    on_frame: EventHandler<(i64, i64)>,
) -> Element {
    let mut gesture = use_signal(|| None::<Gesture>);
    let width_px = crate::util::element_rect(&canvas)
        .map_or(1000.0, |r| r.2)
        .max(200.0);
    let pct = |t: i64| scale.pct(t);

    let stacks: Vec<Option<Stack>> = lanes
        .iter()
        .map(|l| match &l.body {
            Body::Items {
                items,
                max_rows,
                stack: s,
                ..
            } => Some(stack(&scale, items, *max_rows, *s, width_px)),
            _ => None,
        })
        .collect();
    let heights: Vec<f64> = stacks
        .iter()
        .map(|s| s.as_ref().map_or(SPAN_LANE_H, |s| s.h))
        .collect();
    let total_h = RULER_H + heights.iter().sum::<f64>();

    // Ruler ticks (per unfolded stretch); one too close to the previous
    // one, or to a break, would overlap it.
    let breaks: Vec<axis::Stretch> = scale
        .stretches
        .iter()
        .filter(|s| s.folded)
        .copied()
        .collect();
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
    // Density of what happens, under the ticks.
    let mut bins = [0u32; 60];
    for l in &lanes {
        if let Body::Items { items, .. } = &l.body {
            for it in items {
                let p = pct(it.from);
                if (0.0..100.0).contains(&p) {
                    bins[(p / 100.0 * 60.0) as usize] += 1;
                }
            }
        }
    }
    let max_bin = bins.iter().copied().max().unwrap_or(0).max(1) as f64;
    let chip_pad = match base {
        TimeBase::Wall { .. } => 60_000,
        _ => ((scale.window.1 - scale.window.0) / 100).max(1),
    };

    let (c1, s1) = (canvas.clone(), scale.clone());
    let on_down = move |e: PointerEvent| {
        if e.trigger_button() != Some(dioxus::html::input_data::MouseButton::Primary) {
            return;
        }
        if let Some(t) = pointer_time(&c1, &s1, e.client_coordinates().x) {
            on_seek.call(t);
            gesture.set(Some(Gesture::Scrub));
        }
    };
    let (c2, s2) = (canvas.clone(), scale.clone());
    let on_move_ptr = move |e: PointerEvent| {
        let x = e.client_coordinates().x;
        let Some(t) = pointer_time(&c2, &s2, x) else {
            return;
        };
        match gesture() {
            Some(Gesture::Scrub) => on_seek.call(t),
            Some(Gesture::New { lane, t0, .. }) => {
                gesture.set(Some(Gesture::New { lane, t0, t1: t }))
            }
            Some(Gesture::Move { id, x0, .. }) => {
                let shift = t - pointer_time(&c2, &s2, x0).unwrap_or(t);
                gesture.set(Some(Gesture::Move { id, x0, shift }));
            }
            None => {}
        }
    };
    // Lane of each item: a click on a mark reports both.
    let owners: Vec<(String, String)> = lanes
        .iter()
        .flat_map(|l| match &l.body {
            Body::Items { items, .. } => items
                .iter()
                .map(|it| (it.id.clone(), l.id.clone()))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect();
    let s3 = scale.clone();
    let on_up = move |_| {
        let g = gesture();
        gesture.set(None);
        match g {
            Some(Gesture::New { lane, t0, t1 }) => {
                let (a, b) = (t0.min(t1), t0.max(t1));
                let b = if s3.pct(b) - s3.pct(a) > CLICK_PCT {
                    b
                } else {
                    a
                };
                on_create.call((lane, a, b));
            }
            Some(Gesture::Move { id, shift, .. }) => {
                let at = s3.window.0;
                if (s3.pct(at + shift) - s3.pct(at)).abs() <= 0.3 {
                    let lane = owners
                        .iter()
                        .find(|(i, _)| *i == id)
                        .map(|(_, l)| l.clone())
                        .unwrap_or_default();
                    on_item.call((lane, id));
                } else {
                    on_move.call((id, shift));
                }
            }
            _ => {}
        }
    };
    let moved = move |id: &str| match &*gesture.read() {
        Some(Gesture::Move { id: m, shift, .. }) if m == id => *shift,
        _ => 0,
    };

    rsx! {
        div { class: "grid max-h-[42vh] grid-cols-[9rem_minmax(0,1fr)] overflow-y-auto sm:grid-cols-[11rem_minmax(0,1fr)]",
            // Labels column.
            div { class: "border-r border-line text-xs",
                div {
                    class: "flex items-center px-2 border-b border-line bg-gray-50 text-slate",
                    style: "height: {RULER_H}px",
                    "{header}"
                }
                for (lane, h) in lanes.iter().zip(heights.iter().copied()) {
                    LaneTitle {
                        key: "{lane.id}",
                        lane: lane.clone(),
                        height: h,
                        on_click: move |id: String| on_title.call(id),
                    }
                }
            }
            // Lanes column: one relative block, the cursor spans it.
            div {
                id: "{canvas}",
                class: "relative select-none touch-none",
                onpointermove: on_move_ptr,
                onpointerup: on_up,
                onpointerleave: move |_| gesture.set(None),
                // Ruler.
                div {
                    class: "relative border-b border-line bg-gray-50 cursor-ew-resize",
                    style: "height: {RULER_H}px",
                    "data-testid": "timeline-ruler",
                    onpointerdown: on_down,
                    for (t, step) in ticks.iter().copied() {
                        div {
                            key: "{t}",
                            class: "absolute top-0 h-6 border-l border-line-strong pl-1 pt-0.5 font-mono text-xs text-slate whitespace-nowrap",
                            style: "left: {pct(t)}%",
                            {base.tick_label(t, step)}
                        }
                    }
                    for b in breaks.iter().copied() {
                        BreakMark { key: "b{b.t0}", stretch: b, base }
                    }
                    svg {
                        class: "absolute inset-x-0 bottom-0 h-2.5 w-full",
                        view_box: "0 0 60 10",
                        preserve_aspect_ratio: "none",
                        "aria-hidden": "true",
                        for (i, c) in bins.iter().copied().enumerate() {
                            if density && c > 0 {
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
                for (li, lane) in lanes.iter().enumerate() {
                    match &lane.body {
                        Body::Items { items, max_rows, empty, .. } => {
                            let st = stacks[li].as_ref().expect("items lane has a stack");
                            let lane_id = lane.id.clone();
                            let (cv, sc) = (canvas.clone(), scale.clone());
                            let (cv2, sc2) = (canvas.clone(), scale.clone());
                            let hover_lane = lane.id.clone();
                            let editable = lane.editable;
                            rsx! {
                                div {
                                    key: "{lane.id}",
                                    class: "relative border-b border-line",
                                    class: if lane.tinted { "bg-gray-50" },
                                    class: if editable { "cursor-crosshair" },
                                    "data-testid": lane.testid,
                                    style: "height: {st.h}px",
                                    onpointerdown: move |e: PointerEvent| {
                                        if !editable {
                                            return;
                                        }
                                        if let Some(t) = pointer_time(&cv, &sc, e.client_coordinates().x) {
                                            gesture
                                                .set(
                                                    Some(Gesture::New {
                                                        lane: lane_id.clone(),
                                                        t0: t,
                                                        t1: t,
                                                    }),
                                                );
                                        }
                                    },
                                    onpointermove: move |e: PointerEvent| {
                                        if dropping {
                                            if let Some(at) = pointer_time(&cv2, &sc2, e.client_coordinates().x) {
                                                on_hover
                                                    .call(Hover {
                                                        lane: hover_lane.clone(),
                                                        item: None,
                                                        at,
                                                    });
                                            }
                                        }
                                    },
                                    if items.is_empty() {
                                        if let Some(text) = empty {
                                            p { class: "absolute left-3 top-2 text-xs text-slate pointer-events-none",
                                                "{text}"
                                            }
                                        }
                                    }
                                    for (x, group) in st.chips.iter().cloned() {
                                        MoreChip {
                                            key: "m{x}",
                                            left: x,
                                            top: 5.0 + *max_rows as f64 * ROW_H,
                                            count: group.len(),
                                            frame: axis::frame(group.iter().map(|i| (items[*i].from, items[*i].to)), chip_pad),
                                            on_frame,
                                        }
                                    }
                                    if let Some(Gesture::New { lane: gl, t0, t1 }) = gesture() {
                                        if gl == lane.id {
                                            div {
                                                class: "absolute top-1.5 h-3 rounded-sm border border-dashed border-signal bg-signal-soft pointer-events-none",
                                                style: "left: {pct(t0.min(t1))}%; width: {pct(t0.max(t1)) - pct(t0.min(t1))}%",
                                            }
                                        }
                                    }
                                    for (i, it) in items.iter().enumerate().filter(|(i, _)| st.rows[*i] < *max_rows) {
                                        ItemView {
                                            key: "{it.id}",
                                            item: it.clone(),
                                            lane: lane.id.clone(),
                                            testid: lane.item_testid,
                                            left: pct(it.from + moved(&it.id)),
                                            right: pct(it.to + moved(&it.id)),
                                            row: st.rows[i],
                                            dropping,
                                            base,
                                            on_grab: move |(id, x): (String, f64)| {
                                                gesture
                                                    .set(
                                                        Some(Gesture::Move {
                                                            id,
                                                            x0: x,
                                                            shift: 0,
                                                        }),
                                                    );
                                            },
                                            on_item,
                                            on_hover,
                                        }
                                    }
                                }
                            }
                        }
                        Body::Spans { spans, gaps, marks } => rsx! {
                            SpanLane {
                                key: "{lane.id}",
                                canvas: canvas.clone(),
                                scale: scale.clone(),
                                lane: lane.id.clone(),
                                color: lane.color.clone().unwrap_or_else(|| "#4c5966".into()),
                                spans: spans.clone(),
                                gaps: *gaps,
                                marks: marks.clone(),
                                dropping,
                                on_seek,
                                on_hover,
                            }
                        },
                        Body::Note(text) => rsx! {
                            div {
                                key: "{lane.id}",
                                class: "flex items-center overflow-hidden whitespace-nowrap border-b border-line px-3 text-xs text-slate",
                                style: "height: {SPAN_LANE_H}px",
                                "{text}"
                            }
                        },
                    }
                }
                // Breaks across the lanes, under the items.
                for b in breaks.iter().copied() {
                    div {
                        key: "x{b.t0}",
                        class: "pointer-events-none absolute",
                        style: "left: {b.p0}%; width: {b.p1 - b.p0}%; top: {RULER_H}px; height: {total_h - RULER_H}px; background: repeating-linear-gradient(90deg, transparent 0 3px, color-mix(in srgb, var(--color-line-strong, #c3cbd3) 45%, transparent) 3px 4px)",
                    }
                }
                // Cursor, with its handle in the ruler (a press there scrubs).
                div {
                    class: "pointer-events-none absolute top-0 w-0.5 bg-signal",
                    style: "left: {pct(cursor)}%; height: {total_h}px",
                    "data-testid": "timeline-cursor",
                    span {
                        class: "absolute left-1/2 h-4 w-4 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-surface bg-signal shadow pointer-coarse:h-6 pointer-coarse:w-6",
                        style: "top: {RULER_H / 2.0}px",
                    }
                }
            }
        }
    }
}

/// Title of a lane in the labels column (a button when `clickable`).
#[component]
fn LaneTitle(lane: Lane, height: f64, on_click: EventHandler<String>) -> Element {
    let class = if lane.active {
        "flex w-full items-center gap-2 px-2 border-b border-line bg-signal-soft text-left"
    } else if lane.clickable {
        "flex w-full items-center gap-2 px-2 border-b border-line text-left hover:bg-gray-50"
    } else {
        "flex w-full items-center gap-2 px-2 border-b border-line text-left"
    };
    let id = lane.id.clone();
    rsx! {
        button {
            r#type: "button",
            class: "{class}",
            class: if lane.tinted { "text-slate" } else { "text-ink" },
            disabled: !lane.clickable,
            style: "height: {height}px",
            title: lane.title_tip.clone(),
            onclick: move |_| on_click.call(id.clone()),
            if let Some(c) = &lane.color {
                span {
                    class: "h-2.5 w-2.5 shrink-0 rounded-full",
                    style: "background: {c}",
                }
            }
            span { class: "truncate font-medium", "{lane.title}" }
            if let Some(n) = lane.count {
                span { class: "ml-auto font-mono text-slate", "{n}" }
            }
        }
    }
}

/// One item: a mark (diamond or bar, framed label) or a chip.
#[component]
fn ItemView(
    item: Item,
    lane: String,
    testid: &'static str,
    left: f64,
    right: f64,
    row: usize,
    dropping: bool,
    base: TimeBase,
    on_grab: EventHandler<(String, f64)>,
    on_item: EventHandler<(String, String)>,
    on_hover: EventHandler<Hover>,
) -> Element {
    let top = 5.0 + row as f64 * ROW_H;
    let tip = item
        .tip
        .clone()
        .unwrap_or_else(|| match item.label.is_empty() {
            true => base.stamp(item.from),
            false => format!("{} · {}", item.label, base.stamp(item.from)),
        });
    let (id, lane_id) = (item.id.clone(), lane.clone());
    if matches!(item.kind, ItemKind::Signal | ItemKind::Proposal) {
        let chip = if item.kind == ItemKind::Signal {
            "absolute flex max-w-48 -translate-x-1 items-center gap-1 rounded border border-dashed border-line-strong bg-surface px-1.5 text-xs leading-5 text-slate hover:border-signal"
        } else {
            "absolute flex max-w-48 -translate-x-1 items-center gap-1 rounded border border-signal bg-signal-soft px-1.5 text-xs leading-5 text-ink"
        };
        return rsx! {
            button {
                r#type: "button",
                class: "{chip}",
                class: if item.dim { "opacity-40" },
                style: "left: {left}%; top: {top}px; {axis::anchor(left)}",
                title: "{tip}",
                "data-testid": if item.kind == ItemKind::Signal { "timeline-signal" } else { "timeline-proposal" },
                onpointerdown: move |e: PointerEvent| e.stop_propagation(),
                onclick: move |_| on_item.call((lane_id.clone(), id.clone())),
                span { class: "truncate", "{item.label}" }
            }
        };
    }
    let period = item.to > item.from;
    let at = if period { right } else { left };
    let fill = if item.selected {
        "background: var(--color-signal, #0f766e)".to_string()
    } else {
        item.fill
            .as_ref()
            .map(|f| format!("background: {f}"))
            .unwrap_or_default()
    };
    let (grab_id, hover_id, key_id) = (item.id.clone(), item.id.clone(), item.id.clone());
    let (hover_lane, key_lane) = (lane.clone(), lane.clone());
    rsx! {
        if period {
            div {
                class: "pointer-events-none absolute h-3 rounded-sm border",
                class: if item.selected { "border-signal bg-signal-soft" } else { "border-ink bg-gray-100" },
                style: "left: {left}%; width: max(6px, {right - left}%); top: {top + 4.0}px",
            }
        }
        div {
            class: "absolute flex h-5 items-center gap-1.5 whitespace-nowrap text-xs",
            class: if item.movable { "cursor-grab" } else { "cursor-pointer" },
            class: if item.dim { "opacity-40" },
            style: "left: {at}%; top: {top}px; {axis::anchor(at)}",
            "data-testid": testid,
            tabindex: 0,
            role: "button",
            "aria-label": "{tip}",
            "aria-pressed": item.selected,
            title: "{tip}",
            onpointerdown: move |e: PointerEvent| {
                e.stop_propagation();
                on_grab.call((grab_id.clone(), e.client_coordinates().x));
            },
            onpointermove: move |e: PointerEvent| {
                if dropping {
                    e.stop_propagation();
                    on_hover
                        .call(Hover {
                            lane: hover_lane.clone(),
                            item: Some(hover_id.clone()),
                            at: 0,
                        });
                }
            },
            onkeydown: move |e: KeyboardEvent| {
                if e.key() == Key::Enter {
                    on_item.call((key_lane.clone(), key_id.clone()));
                }
            },
            if !period {
                span {
                    class: "h-3 w-3 -translate-x-1.5 rotate-45 rounded-sm border-2 bg-surface transition-transform duration-150 ease-out hover:scale-125",
                    class: if item.selected { "border-signal" } else { "border-ink" },
                    class: if item.kind == ItemKind::Detected { "border-dotted" },
                    style: "{fill}",
                }
            }
            if !item.label.is_empty() {
                span {
                    class: "relative rounded-md border bg-surface px-1.5 leading-5",
                    class: if item.selected || item.targeted { "border-signal text-signal" } else { "border-line-strong text-ink" },
                    class: if item.targeted { "ring-2 ring-signal" },
                    class: if period { "ml-1.5" },
                    span {
                        class: "inline-block truncate align-bottom",
                        style: "max-width: {LABEL_MAX_PX}px",
                        "{item.label}"
                    }
                    if let Some(b) = &item.badge {
                        " "
                        span { class: "font-mono text-slate", "{b}" }
                    }
                }
            }
        }
    }
}

/// Lane of covered spans: in its colour, gaps hatched, thin marks; a press
/// scrubs.
#[component]
fn SpanLane(
    canvas: String,
    scale: Scale,
    lane: String,
    color: String,
    spans: Vec<(i64, i64)>,
    gaps: bool,
    marks: Vec<i64>,
    dropping: bool,
    on_seek: EventHandler<i64>,
    on_hover: EventHandler<Hover>,
) -> Element {
    let holes: Vec<(i64, i64)> = if gaps {
        spans.windows(2).map(|p| (p[0].1, p[1].0)).collect()
    } else {
        Vec::new()
    };
    let (cv, sc) = (canvas.clone(), scale.clone());
    rsx! {
        div {
            class: "relative border-b border-line",
            style: "height: {SPAN_LANE_H}px",
            onpointermove: move |e: PointerEvent| {
                if dropping {
                    if let Some(at) = pointer_time(&cv, &sc, e.client_coordinates().x) {
                        on_hover
                            .call(Hover {
                                lane: lane.clone(),
                                item: None,
                                at,
                            });
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
            for (i, (a, b)) in holes.iter().copied().enumerate() {
                div {
                    key: "g{i}",
                    class: "absolute top-1.5 h-4 rounded",
                    style: "left: {scale.pct(a)}%; width: {scale.pct(b) - scale.pct(a)}%; background: repeating-linear-gradient(135deg, transparent 0 4px, var(--color-line-strong, #c3cbd3) 4px 5px)",
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

/// A folded stretch on the ruler: its rough length, its bounds on hover.
#[component]
fn BreakMark(stretch: axis::Stretch, base: TimeBase) -> Element {
    let tip = format!("{} → {}", base.stamp(stretch.t0), base.stamp(stretch.t1));
    rsx! {
        div {
            class: "absolute inset-y-0 flex items-center justify-center overflow-hidden border-x border-dashed border-line-strong bg-gray-100 text-xs text-slate",
            style: "left: {stretch.p0}%; width: {stretch.p1 - stretch.p0}%",
            title: "{tip}",
            "data-testid": "timeline-break",
            span { class: "whitespace-nowrap px-0.5", {base.span_text(stretch.t1 - stretch.t0)} }
        }
    }
}

/// Items that did not fit: a click frames them.
#[component]
fn MoreChip(
    left: f64,
    top: f64,
    count: usize,
    frame: Option<(i64, i64)>,
    on_frame: EventHandler<(i64, i64)>,
) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: "absolute -translate-x-1 rounded-full border border-line-strong bg-surface px-2 text-xs leading-5 text-ink hover:border-signal",
            style: "left: {left}%; top: {top}px; {axis::anchor(left)}",
            title: dioxus_i18n::t!("timeline-more-help"),
            "data-testid": "timeline-more",
            onpointerdown: move |e: PointerEvent| e.stop_propagation(),
            onclick: move |_| {
                if let Some(f) = frame {
                    on_frame.call(f);
                }
            },
            "+{count}"
        }
    }
}
