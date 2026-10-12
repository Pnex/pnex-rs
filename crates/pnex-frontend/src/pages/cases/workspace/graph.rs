//! The case graph (cases-workspace.md K31): events (diamonds) and the
//! objects they involve (circles), an edge per attachment with its role,
//! the ontology relations among members dashed (faded before they start,
//! hidden once closed at the cursor). A deterministic force layout (`pnex_core::ontology::layout`,
//! seeded by the case id) places the nodes once; a newcomer never moves the
//! others, and a node dragged by hand stays where it is dropped — saved
//! for the whole team ("Re-layout" clears it). What lies
//! after the cursor fades (only classes change per frame). Double-click an
//! object to expand it: its neighbours counted by link and type, then the
//! ones picked, until "Undo expansion" or "Keep" (they join the undated).

use std::collections::{HashMap, HashSet};

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{
    CaseAttachInput, CaseLayout, CaseWorkspace, ExpandFacet, ExpandInput, ExpandResult, LinkView,
    ObjectRef,
};
use pnex_core::ontology::layout::{force_layout, LayoutNode};

use super::{model, Sel, Ws};
use crate::api;
use crate::components::ontology::add_to_case::role_label;
use crate::components::ui::{BTN_GHOST, BTN_PRIMARY_SM, BTN_SECONDARY_SM};
use crate::state::toasts;

/// Nodes drawn at most; the rest is counted.
const CAP: usize = 300;
/// Under this many nodes every label shows; above, on hover or selection.
const LABELS_UNDER: usize = 40;
const SVG_ID: &str = "cw-graph";

#[derive(Debug, Clone, PartialEq)]
pub(super) struct GNode {
    pub id: String,
    pub title: String,
    pub event_from: Option<i64>,
    /// Added by an expansion, not (yet) in the case.
    pub extra: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum EdgeKind {
    /// An attachment: the role, the event's start.
    Role { role: String, at: Option<i64> },
    /// An ontology relation and its validity.
    Relation {
        link_type: String,
        from: Option<i64>,
        to: Option<i64>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct GEdge {
    pub a: usize,
    pub b: usize,
    pub kind: EdgeKind,
}

/// Nodes and edges of the graph, and how many nodes the cap left out.
pub(super) fn build(
    w: &CaseWorkspace,
    follow: Option<&str>,
    relations: &[LinkView],
    extra: &[ExpandResult],
) -> (Vec<GNode>, Vec<GEdge>, usize) {
    let mut nodes: Vec<GNode> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut hidden = 0;
    let mut add = |nodes: &mut Vec<GNode>, o: &ObjectRef, at: Option<i64>, extra: bool| {
        if let Some(i) = index.get(&o.id) {
            return Some(*i);
        }
        if nodes.len() == CAP {
            hidden += 1;
            return None;
        }
        index.insert(o.id.clone(), nodes.len());
        nodes.push(GNode {
            id: o.id.clone(),
            title: o.title.clone(),
            event_from: at,
            extra,
        });
        Some(nodes.len() - 1)
    };
    let mut edges = Vec::new();
    let events = w
        .events
        .iter()
        .filter(|e| follow.is_none_or(|f| e.attachments.iter().any(|a| a.object.id == f)));
    for e in events {
        let at = model::span(&e.event).map(|s| s.0);
        let ev = ObjectRef {
            id: e.event.id.clone(),
            type_key: e.event.type_key.clone(),
            title: e.event.title.clone(),
            native_id: e.event.native_id.clone(),
        };
        let Some(ei) = add(&mut nodes, &ev, at, false) else {
            continue;
        };
        for a in &e.attachments {
            if let Some(oi) = add(&mut nodes, &a.object, None, false) {
                edges.push(GEdge {
                    a: ei,
                    b: oi,
                    kind: EdgeKind::Role {
                        role: a.role.clone(),
                        at,
                    },
                });
            }
        }
    }
    if follow.is_none() {
        for u in &w.undated {
            add(&mut nodes, u, None, false);
        }
    }
    for x in extra {
        for n in &x.nodes {
            add(&mut nodes, n, None, true);
        }
    }
    let ms = |s: &str| model::ms(s);
    for l in relations
        .iter()
        .chain(extra.iter().flat_map(|x| x.links.iter()))
    {
        let (Some(a), Some(b)) = (index.get(&l.source.id), index.get(&l.target.id)) else {
            continue;
        };
        edges.push(GEdge {
            a: *a,
            b: *b,
            kind: EdgeKind::Relation {
                link_type: l.link_type.clone(),
                from: ms(&l.valid_from),
                to: l.valid_to.as_deref().and_then(ms),
            },
        });
    }
    (nodes, edges, hidden)
}

/// How a relation draws at the cursor: valid then; not yet (faded: it is
/// often recorded after the facts); closed before (hidden).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Shown {
    Live,
    Ahead,
    Gone,
}

pub(super) fn relation_at(from: Option<i64>, to: Option<i64>, cursor: i64) -> Shown {
    if to.is_some_and(|t| t <= cursor) {
        Shown::Gone
    } else if from.is_some_and(|f| f > cursor) {
        Shown::Ahead
    } else {
        Shown::Live
    }
}

/// Faded nodes: an event after the cursor; an object whose attachments are
/// all to events after the cursor. Undated pieces never fade.
pub(super) fn faded(nodes: &[GNode], edges: &[GEdge], cursor: i64) -> Vec<bool> {
    let mut out: Vec<bool> = nodes
        .iter()
        .map(|n| n.event_from.is_some_and(|t| t > cursor))
        .collect();
    let mut past: HashSet<usize> = HashSet::new();
    let mut attached: HashSet<usize> = HashSet::new();
    for e in edges {
        if let EdgeKind::Role { at, .. } = e.kind {
            attached.insert(e.b);
            if at.is_none_or(|t| t <= cursor) {
                past.insert(e.b);
            }
        }
    }
    for i in attached.difference(&past) {
        out[*i] = true;
    }
    out
}

/// Layout coordinates onto the panel's pixels: one scale for both axes
/// (never enlarging more than 1.5 ×), centred; nodes and labels keep their
/// size on screen whatever the layout's extent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Fit {
    k: f64,
    ox: f64,
    oy: f64,
    x0: f64,
    y0: f64,
    pub w: f64,
    pub h: f64,
}

impl Fit {
    pub fn new(points: &[(f64, f64)], w: f64, h: f64) -> Self {
        // Room for the labels and, top left, the "+" and the tools.
        let pad = 72.0;
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (x, y) in points {
            x0 = x0.min(*x);
            y0 = y0.min(*y);
            x1 = x1.max(*x);
            y1 = y1.max(*y);
        }
        if points.is_empty() {
            (x0, y0, x1, y1) = (0.0, 0.0, 1.0, 1.0);
        }
        let (rx, ry) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
        let k = ((w - 2.0 * pad) / rx)
            .min((h - 2.0 * pad) / ry)
            .clamp(1e-3, 1.5);
        Self {
            k,
            ox: (w - rx * k) / 2.0,
            oy: (h - ry * k) / 2.0,
            x0,
            y0,
            w,
            h,
        }
    }

    /// The same fit seen through a zoom (around the panel's centre) and a
    /// pan, both in screen pixels.
    pub fn view(mut self, zoom: f64, pan: (f64, f64)) -> Self {
        let (cx, cy) = (self.w / 2.0, self.h / 2.0);
        self.ox = cx + (self.ox - cx) * zoom + pan.0;
        self.oy = cy + (self.oy - cy) * zoom + pan.1;
        self.k *= zoom;
        self
    }

    pub fn px(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (
            self.ox + (x - self.x0) * self.k,
            self.oy + (y - self.y0) * self.k,
        )
    }

    pub fn layout_of(&self, (px, py): (f64, f64)) -> (f64, f64) {
        (
            self.x0 + (px - self.ox) / self.k,
            self.y0 + (py - self.oy) / self.k,
        )
    }
}

fn short(s: &str) -> String {
    let mut out: String = s.chars().take(22).collect();
    if s.chars().count() > 22 {
        out.push('…');
    }
    out
}

/// Neighbours offered by an expansion: the node, the facets, the ticked ones.
#[derive(Clone, PartialEq)]
struct Offer {
    node: String,
    title: String,
    facets: Vec<ExpandFacet>,
    ticked: HashSet<usize>,
}

#[component]
pub(super) fn Graph(workspace: CaseWorkspace) -> Element {
    let ws = use_context::<Ws>();
    let mut positions = use_signal(HashMap::<String, (f64, f64)>::new);
    let mut extra = use_signal(Vec::<ExpandResult>::new);
    let mut relayout = use_signal(|| 0u32);
    let mut drag = use_signal(|| None::<(String, bool)>);
    // View: zoom around the pointer (wheel), pan by dragging the background.
    let mut zoom = use_signal(|| 1.0f64);
    let mut pan = use_signal(|| (0.0f64, 0.0f64));
    let mut panning = use_signal(|| None::<((f64, f64), (f64, f64))>);
    // The fit stays put from a drag on (no rescale under the hand, a dropped
    // node stays where it was let go) until nodes come or go, or re-layout.
    let mut held = use_signal(|| None::<Fit>);
    // The panel's size, followed (the timeline folds and unfolds under it):
    // the drawing refits instead of stretching.
    let mut size = use_signal(|| (0.0f64, 0.0f64));
    use_effect(move || {
        spawn(async move {
            let mut ev = document::eval(
                r#"
                const el = document.getElementById('cw-graph-box');
                if (el) {
                    new ResizeObserver((es) => {
                        const r = es[0].contentRect;
                        dioxus.send([r.width, r.height]);
                    }).observe(el);
                }
                await new Promise(() => {});
                "#,
            );
            while let Ok(wh) = ev.recv::<(f64, f64)>().await {
                held.set(None);
                size.set(wh);
            }
        });
    });
    let mut hover = use_signal(|| None::<String>);
    let mut offer = use_signal(|| None::<Offer>);
    let relations = use_resource(move || {
        let _ = (ws.data)();
        let id = ws.case_id.peek().clone();
        async move { api::cases::relations(&id).await.unwrap_or_default() }
    });
    // The arrangement the team saved, applied once before the first layout.
    let saved = use_resource(move || {
        let id = ws.case_id.peek().clone();
        async move { api::cases::layout(&id).await.unwrap_or_default() }
    });
    let mut seeded = use_signal(|| false);
    // Place the newcomers; whatever has a position keeps it.
    use_effect(move || {
        let _ = relayout();
        let Some(w) = (ws.data)() else { return };
        let Some(saved) = saved.read().clone() else {
            return;
        };
        let rel = relations.read().clone().unwrap_or_default();
        let (nodes, edges, _) = build(&w, (ws.follow)().as_deref(), &rel, &extra());
        let mut prev = positions.peek().clone();
        if !*seeded.peek() {
            prev.extend(saved.positions);
            seeded.set(true);
        }
        let input: Vec<LayoutNode> = nodes
            .iter()
            .map(|n| LayoutNode {
                key: n.id.clone(),
                fixed: prev.get(&n.id).copied(),
            })
            .collect();
        let pairs: Vec<(usize, usize)> = edges.iter().map(|e| (e.a, e.b)).collect();
        // The frame takes the panel's shape: a wide panel, a wide drawing.
        let aspect = crate::util::element_rect(SVG_ID).map_or(1.6, |r| r.2 / r.3.max(1.0));
        let placed = force_layout(&input, &pairs, &ws.case_id.peek(), aspect);
        // A newcomer may need room: only then does the drawing re-fit.
        if nodes.iter().any(|n| !prev.contains_key(&n.id)) {
            held.set(None);
        }
        let mut next = prev;
        for (n, p) in nodes.iter().zip(placed) {
            next.insert(n.id.clone(), p);
        }
        positions.set(next);
    });

    let follow = (ws.follow)();
    let rel = relations.read().clone().unwrap_or_default();
    let (nodes, edges, hidden) = build(&workspace, follow.as_deref(), &rel, &extra());
    let cursor = (ws.cursor)();
    let fade = faded(&nodes, &edges, cursor);
    let pos = positions();
    let raw = |i: usize| pos.get(&nodes[i].id).copied().unwrap_or((500.0, 300.0));
    let points: Vec<(f64, f64)> = (0..nodes.len()).map(raw).collect();
    let (pw, ph) = match size() {
        (w, h) if w > 0.0 && h > 0.0 => (w, h),
        _ => crate::util::element_rect(SVG_ID).map_or((600.0, 288.0), |r| (r.2, r.3)),
    };
    let base = held().unwrap_or_else(|| Fit::new(&points, pw.max(200.0), ph.max(160.0)));
    let fit = base.view(zoom(), pan());
    let at = |i: usize| fit.px(points[i]);
    let selected = match (ws.sel)() {
        Sel::Event(id) | Sel::Object(id) => Some(id),
        Sel::None => None,
    };
    let focus = hover().or(selected.clone());
    let focus_idx = focus
        .as_ref()
        .and_then(|f| nodes.iter().position(|n| &n.id == f));
    let near: HashSet<usize> = edges
        .iter()
        .filter(|e| Some(e.a) == focus_idx || Some(e.b) == focus_idx)
        .flat_map(|e| [e.a, e.b])
        .collect();
    let show_all_labels = nodes.len() <= LABELS_UNDER;
    let empty = nodes.is_empty();
    let is_event: HashSet<String> = workspace
        .events
        .iter()
        .map(|e| e.event.id.clone())
        .collect();

    let on_move = move |e: PointerEvent| {
        let c = e.client_coordinates();
        if let Some((start, from)) = panning() {
            pan.set((from.0 + c.x - start.0, from.1 + c.y - start.1));
            return;
        }
        let Some((id, _)) = drag() else { return };
        if let Some((l, t, _, _)) = crate::util::element_rect(SVG_ID) {
            let p = fit.layout_of((c.x - l, c.y - t));
            positions.with_mut(|m| {
                m.insert(id.clone(), p);
            });
            drag.set(Some((id, true)));
        }
    };
    let select = move |id: String, event: bool| {
        if event {
            ws.select_event(id);
        } else {
            let mut sel = ws.sel;
            sel.set(Sel::Object(id));
        }
    };
    // Saves the arrangement (none = back to the automatic layout).
    let save = move |p: HashMap<String, (f64, f64)>| {
        let id = ws.case_id.peek().clone();
        let input = CaseLayout {
            positions: p.into_iter().collect(),
        };
        spawn(async move {
            if let Err(e) = api::cases::save_layout(&id, &input).await {
                toasts::error(e);
            }
        });
    };
    // The background: a press pans the view, the wheel zooms around the
    // pointer (the point under it stays put).
    let on_press = move |e: PointerEvent| {
        let c = e.client_coordinates();
        panning.set(Some(((c.x, c.y), pan())));
    };
    let on_wheel = move |e: WheelEvent| {
        e.prevent_default();
        let Some((l, t, w, h)) = crate::util::element_rect(SVG_ID) else {
            return;
        };
        let c = e.client_coordinates();
        let dy = e.delta().strip_units().y;
        let (z, p) = (zoom(), pan());
        let next = (z * if dy < 0.0 { 1.15 } else { 1.0 / 1.15 }).clamp(0.3, 6.0);
        let (sx, sy) = (c.x - l - w / 2.0, c.y - t - h / 2.0);
        zoom.set(next);
        pan.set((sx - (sx - p.0) * next / z, sy - (sy - p.1) * next / z));
    };
    let on_up = {
        let is_event = is_event.clone();
        move |e: PointerEvent| {
            panning.set(None);
            if let Some((id, moved)) = drag() {
                if !moved {
                    select(id.clone(), is_event.contains(&id));
                    return drag.set(None);
                }
                // The release point itself: the last move may not have landed.
                let c = e.client_coordinates();
                if let Some((l, t, _, _)) = crate::util::element_rect(SVG_ID) {
                    let p = fit.layout_of((c.x - l, c.y - t));
                    positions.with_mut(|m| {
                        m.insert(id.clone(), p);
                    });
                }
                if ws.can_write {
                    // Kept for everyone who opens the case.
                    save(positions.peek().clone());
                }
            }
            drag.set(None);
        }
    };
    // Leaving the panel mid-drag drops the node there (a drop near the edge
    // used to be lost).
    let on_leave = move |_| {
        panning.set(None);
        if drag().is_some_and(|(_, moved)| moved) && ws.can_write {
            save(positions.peek().clone());
        }
        drag.set(None);
    };
    let shown: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
    let open_offer = {
        let shown = shown.clone();
        move |(node, title): (String, String)| {
            let id = ws.case_id.peek().clone();
            let input = ExpandInput {
                object_id: node.clone(),
                exclude: shown.clone(),
                pick: None,
            };
            spawn(async move {
                match api::cases::expand(&id, &input).await {
                    Ok(r) if r.facets.is_empty() => toasts::info(t!("cw-expand-none").to_string()),
                    Ok(r) => offer.set(Some(Offer {
                        node,
                        title,
                        facets: r.facets,
                        ticked: HashSet::new(),
                    })),
                    Err(e) => toasts::error(e),
                }
            });
        }
    };
    let add_picked = move |_| {
        let Some(o) = offer() else { return };
        let input = ExpandInput {
            object_id: o.node.clone(),
            exclude: shown.clone(),
            pick: Some(
                o.ticked
                    .iter()
                    .filter_map(|i| o.facets.get(*i).cloned())
                    .collect(),
            ),
        };
        let id = ws.case_id.peek().clone();
        offer.set(None);
        spawn(async move {
            match api::cases::expand(&id, &input).await {
                Ok(r) => extra.with_mut(|x| x.push(r)),
                Err(e) => toasts::error(e),
            }
        });
    };
    // Keep: the added neighbours join the case as undated pieces.
    let keep = move |_| {
        let added: Vec<String> = extra()
            .iter()
            .flat_map(|x| x.nodes.iter().map(|n| n.id.clone()))
            .collect();
        let id = ws.case_id.peek().clone();
        spawn(async move {
            for object_id in added {
                let input = CaseAttachInput {
                    object_id,
                    ..Default::default()
                };
                if let Err(e) = api::cases::attach(&id, &input).await {
                    toasts::error(e);
                    break;
                }
            }
            extra.set(Vec::new());
            ws.reload();
        });
    };

    rsx! {
        section {
            id: "cw-graph-box",
            class: "absolute inset-0",
            "aria-label": t!("cw-links"),
            // Floating tools, top right (the "+" sits top left).
            div {
                class: "absolute right-3 top-3 z-20 flex flex-wrap items-center gap-1 rounded-lg border border-line bg-surface px-2 py-1 shadow-sm",
                title: t!("cw-graph-help"),
                if hidden > 0 {
                    span { class: "text-xs text-slate", {t!("cw-graph-hidden", count : hidden)} }
                }
                if follow.is_some() {
                    button {
                        class: BTN_GHOST,
                        r#type: "button",
                        onclick: move |_| {
                            let mut f = ws.follow;
                            f.set(None);
                        },
                        {t!("cw-show-all")}
                    }
                }
                div { class: "flex items-center gap-1",
                    if !extra().is_empty() {
                        button {
                            class: BTN_GHOST,
                            r#type: "button",
                            "data-testid": "case-expand-undo",
                            onclick: move |_| {
                                extra
                                    .with_mut(|x| {
                                        x.pop();
                                    })
                            },
                            {t!("cw-expand-undo")}
                        }
                        if ws.can_write {
                            button {
                                class: BTN_SECONDARY_SM,
                                r#type: "button",
                                "data-testid": "case-expand-keep",
                                onclick: keep,
                                {t!("cw-expand-keep")}
                            }
                        }
                    }
                    button {
                        class: BTN_GHOST,
                        r#type: "button",
                        "aria-label": t!("cw-zoom-out"),
                        onclick: move |_| zoom.set((zoom() / 1.25).max(0.3)),
                        "−"
                    }
                    button {
                        class: BTN_GHOST,
                        r#type: "button",
                        "aria-label": t!("cw-zoom-in"),
                        onclick: move |_| zoom.set((zoom() * 1.25).min(6.0)),
                        "+"
                    }
                    button {
                        class: BTN_GHOST,
                        r#type: "button",
                        title: t!("cw-graph-fit-help"),
                        onclick: move |_| {
                            zoom.set(1.0);
                            pan.set((0.0, 0.0));
                        },
                        {t!("cw-fit")}
                    }
                    button {
                        class: BTN_GHOST,
                        r#type: "button",
                        title: t!("cw-graph-relayout-help"),
                        onclick: move |_| {
                            positions.set(HashMap::new());
                            relayout += 1;
                            if ws.can_write {
                                save(HashMap::new());
                            }
                        },
                        {t!("cw-graph-relayout")}
                    }
                }
            }
            if empty {
                p { class: "absolute inset-0 grid place-items-center p-4 text-center text-sm text-slate",
                    {t!("cw-links-empty")}
                }
            } else {
                svg {
                    id: SVG_ID,
                    class: "absolute inset-0 block h-full w-full touch-none select-none",
                    view_box: "0 0 {fit.w} {fit.h}",
                    preserve_aspect_ratio: "none",
                    role: "img",
                    "aria-label": t!("cw-links"),
                    "data-testid": "case-graph",
                    onpointerdown: on_press,
                    onwheel: on_wheel,
                    onpointermove: on_move,
                    onpointerup: on_up,
                    onpointerleave: on_leave,
                    for (k, e) in edges.iter().enumerate() {
                        {
                            let ((x1, y1), (x2, y2)) = (at(e.a), at(e.b));
                            let (dashed, shown, label) = match &e.kind {
                                EdgeKind::Role { role, .. } => (false, Shown::Live, role_label(role)),
                                EdgeKind::Relation { link_type, from, to } => {
                                    (true, relation_at(*from, *to, cursor), link_type.replace('_', " "))
                                }
                            };
                            let live = shown != Shown::Gone;
                            let dim = fade[e.a] || fade[e.b] || shown == Shown::Ahead;
                            let lit = Some(e.a) == focus_idx || Some(e.b) == focus_idx;
                            rsx! {
                                g { key: "e{k}", class: if !live { "hidden" } else if dim { "opacity-25" },
                                    line {
                                        x1: "{x1}",
                                        y1: "{y1}",
                                        x2: "{x2}",
                                        y2: "{y2}",
                                        stroke: if lit { "var(--color-signal, #1f6f8b)" } else { "var(--color-line-strong, #c3cbd3)" },
                                        stroke_width: if lit { "2.5" } else { "1.5" },
                                        stroke_dasharray: if dashed { "6 5" } else { "none" },
                                    }
                                    if lit {
                                        text {
                                            x: "{(x1 + x2) / 2.0}",
                                            y: "{(y1 + y2) / 2.0 - 4.0}",
                                            text_anchor: "middle",
                                            font_size: "12",
                                            fill: "var(--color-slate, #4c5966)",
                                            "{label}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                    for (i, n) in nodes.iter().enumerate() {
                        {
                            let (x, y) = at(i);
                            let event = n.event_from.is_some() || is_event.contains(&n.id);
                            let on = selected.as_ref() == Some(&n.id);
                            let label = show_all_labels || on || Some(i) == focus_idx || near.contains(&i);
                            let id = n.id.clone();
                            let (hid, hid2) = (n.id.clone(), n.id.clone());
                            let dbl = (n.id.clone(), n.title.clone());
                            let open_offer = open_offer.clone();
                            rsx! {
                                g {
                                    key: "{n.id}",
                                    class: if fade[i] { "cursor-pointer opacity-30" } else { "cursor-pointer" },
                                    "data-testid": "case-graph-node",
                                    onpointerdown: move |e: PointerEvent| {
                                        e.stop_propagation();
                                        drag.set(Some((id.clone(), false)));
                                        held.set(Some(base));
                                    },
                                    onpointerenter: move |_| hover.set(Some(hid.clone())),
                                    onpointerleave: move |_| {
                                        if hover() == Some(hid2.clone()) {
                                            hover.set(None);
                                        }
                                    },
                                    ondoubleclick: move |_| {
                                        if !event {
                                            open_offer(dbl.clone());
                                        }
                                    },
                                    if event {
                                        rect {
                                            x: "{x - 8.0}",
                                            y: "{y - 8.0}",
                                            width: "16",
                                            height: "16",
                                            transform: "rotate(45 {x} {y})",
                                            fill: if on { "var(--color-signal, #1f6f8b)" } else { "var(--color-surface, #fff)" },
                                            stroke: "var(--color-ink, #1b2733)",
                                            stroke_width: "2",
                                        }
                                    } else {
                                        circle {
                                            cx: "{x}",
                                            cy: "{y}",
                                            r: "9",
                                            fill: if on { "var(--color-signal, #1f6f8b)" } else { "var(--color-signal-soft, #e3f0f4)" },
                                            stroke: "var(--color-signal, #1f6f8b)",
                                            stroke_width: "2",
                                            stroke_dasharray: if n.extra { "3 3" } else { "none" },
                                        }
                                    }
                                    if label {
                                        text {
                                            x: "{x}",
                                            y: "{y + 24.0}",
                                            text_anchor: "middle",
                                            font_size: "13",
                                            fill: "var(--color-ink, #1b2733)",
                                            "{short(&n.title)}"
                                        }
                                    }
                                    title { "{n.title}" }
                                }
                            }
                        }
                    }
                }
            }
            if let Some(o) = offer() {
                div {
                    class: "absolute inset-x-3 top-12 z-10 space-y-2 rounded-md border border-line-strong bg-surface p-3 shadow-lg",
                    role: "dialog",
                    "aria-label": t!("cw-expand-title", name : o.title.clone()),
                    "data-testid": "case-expand",
                    p { class: "text-sm font-medium text-ink",
                        {t!("cw-expand-title", name : o.title.clone())}
                    }
                    for (i, f) in o.facets.iter().enumerate() {
                        label {
                            key: "{i}",
                            class: "flex items-center gap-2 text-sm text-ink",
                            input {
                                r#type: "checkbox",
                                checked: o.ticked.contains(&i),
                                onchange: move |_| {
                                    offer
                                        .with_mut(|x| {
                                            if let Some(x) = x {
                                                if !x.ticked.remove(&i) {
                                                    x.ticked.insert(i);
                                                }
                                            }
                                        })
                                },
                            }
                            "{f.link_type.replace('_', \" \")} · {f.type_key} ({f.count})"
                        }
                    }
                    div { class: "flex justify-end gap-2",
                        button {
                            class: BTN_GHOST,
                            r#type: "button",
                            onclick: move |_| offer.set(None),
                            {t!("common-cancel")}
                        }
                        button {
                            class: BTN_PRIMARY_SM,
                            r#type: "button",
                            disabled: o.ticked.is_empty(),
                            "data-testid": "case-expand-add",
                            onclick: add_picked,
                            {t!("cw-expand-add")}
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::ontology::api::{CaseAttachment, CaseEvent, ObjectView};
    use serde_json::json;

    fn obj(id: &str) -> ObjectRef {
        ObjectRef {
            id: id.into(),
            type_key: "person".into(),
            title: id.into(),
            native_id: String::new(),
        }
    }

    fn event(id: &str, at: &str, attached: &[&str]) -> CaseEvent {
        let view: ObjectView = serde_json::from_value(json!({
            "id": id, "type_key": "event", "title": id, "native_id": "", "system": false,
            "properties": {"occurred": {"from": at, "to": at, "precision": "instant"}},
            "version": 1, "valid_from": "", "created_at": "", "updated_at": "",
        }))
        .unwrap();
        CaseEvent {
            event: view,
            attachments: attached
                .iter()
                .map(|o| CaseAttachment {
                    link_id: 1,
                    object: obj(o),
                    role: "actor".into(),
                    detail: None,
                })
                .collect(),
        }
    }

    fn workspace(events: Vec<CaseEvent>, undated: &[&str]) -> CaseWorkspace {
        let case: ObjectView = serde_json::from_value(json!({
            "id": "c", "type_key": "case", "title": "c", "native_id": "", "system": false,
            "properties": {}, "version": 1, "valid_from": "", "created_at": "", "updated_at": "",
        }))
        .unwrap();
        CaseWorkspace {
            case,
            events,
            undated: undated.iter().map(|u| obj(u)).collect(),
            zones: Vec::new(),
            places: Vec::new(),
        }
    }

    #[test]
    fn bipartite_nodes_shared_objects_and_fading_by_the_cursor() {
        let w = workspace(
            vec![
                event("e1", "2025-01-01T10:00:00Z", &["ann", "bob"]),
                event("e2", "2025-01-01T12:00:00Z", &["bob", "carl"]),
            ],
            &["memo"],
        );
        let rel = LinkView {
            id: 9,
            link_type: "knows".into(),
            source: obj("ann"),
            target: obj("carl"),
            attributes: Default::default(),
            valid_from: "2025-01-01T11:00:00Z".into(),
            valid_to: None,
            source_ref: None,
        };
        let (nodes, edges, hidden) = build(&w, None, &[rel], &[]);
        let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["e1", "ann", "bob", "e2", "carl", "memo"]);
        assert_eq!((edges.len(), hidden), (5, 0));
        // At 11:00: e2 is ahead, so is Carl (only on e2); Bob is on e1 too.
        let t = model::ms("2025-01-01T11:00:00Z").unwrap();
        let f = faded(&nodes, &edges, t);
        assert_eq!(f, [false, false, false, true, true, false]);
        // The relation is live from 11:00, ahead before, gone once closed.
        assert_eq!(relation_at(Some(t), None, t), Shown::Live);
        assert_eq!(relation_at(Some(t), None, t - 1), Shown::Ahead);
        assert_eq!(relation_at(None, Some(t), t), Shown::Gone);
        // Following Ann keeps e1 and what it involves only.
        let (n2, _, _) = build(&w, Some("ann"), &[], &[]);
        assert_eq!(n2.len(), 3);
    }

    #[test]
    fn the_fit_maps_both_ways_and_keeps_the_aspect() {
        let f = Fit::new(&[(0.0, 0.0), (1000.0, 100.0)], 600.0, 300.0);
        let (a, b) = (f.px((0.0, 0.0)), f.px((1000.0, 100.0)));
        assert!(a.0 >= 39.9 && b.0 <= 560.1, "{a:?} {b:?}");
        // Same scale on both axes.
        assert!(((b.0 - a.0) / 1000.0 - (b.1 - a.1) / 100.0).abs() < 1e-9);
        let back = f.layout_of(f.px((250.0, 50.0)));
        assert!((back.0 - 250.0).abs() < 1e-9 && (back.1 - 50.0).abs() < 1e-9);
        // A tiny graph is not blown up past 1.5 ×.
        let g = Fit::new(&[(0.0, 0.0), (10.0, 10.0)], 600.0, 300.0);
        assert!((g.px((10.0, 0.0)).0 - g.px((0.0, 0.0)).0 - 15.0).abs() < 1e-9);
    }

    #[test]
    fn the_cap_counts_what_it_leaves_out() {
        let names: Vec<String> = (0..CAP + 5).map(|i| format!("o{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let w = workspace(Vec::new(), &refs);
        let (nodes, _, hidden) = build(&w, None, &[], &[]);
        assert_eq!((nodes.len(), hidden), (CAP, 5));
    }
}
