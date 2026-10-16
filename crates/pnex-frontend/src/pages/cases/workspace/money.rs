//! Money view of a case (cases-money.md K57): a Sankey diagram of the
//! payments, from payers on the left to payees on the right. What an
//! intermediary received and did not pass on shows as "untraced"; a payment
//! that goes back up the chain is a dashed loop above, never dropped. A
//! click on a band lists its payments, on a node selects the object.

use std::collections::HashMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::CaseWorkspace;
use pnex_core::ontology::money::{self, NodeKind, Sankey, Transfer};

use super::{graph::family, model, Sel, Ws};

const TOP: f64 = 72.0;
/// Room on the left for the first column's labels (right-aligned).
const LEFT: f64 = 210.0;
const NODE_W: f64 = 12.0;
const COL_W: f64 = 240.0;
const PAD: f64 = 18.0;
/// Height given to each node of the fullest column.
const ROW_H: f64 = 46.0;
const MIN_H: f64 = 320.0;
/// An untraced stub sits this far right of its intermediary, not a column away.
const STUB: f64 = 110.0;
/// Room above the diagram for the loops of cycles.
const LOOP_ROOM: f64 = 40.0;

fn french() -> bool {
    crate::i18n::current_tag().starts_with("fr")
}

/// Label of a node: the object's title, or what stands for a missing end.
fn label(titles: &HashMap<String, String>, id: &str, kind: NodeKind) -> String {
    match kind {
        NodeKind::UnknownPayer => t!("money-unknown-payer").to_string(),
        NodeKind::UnknownPayee => t!("money-unknown-payee").to_string(),
        NodeKind::Untraced => t!("money-untraced").to_string(),
        NodeKind::Object => titles.get(id).cloned().unwrap_or_else(|| id.to_string()),
    }
}

/// Band of a forward link: a thick stroke along an S-curve.
fn band(x0: f64, y0: f64, x1: f64, y1: f64) -> String {
    let m = (x0 + x1) / 2.0;
    format!("M {x0} {y0} C {m} {y0}, {m} {y1}, {x1} {y1}")
}

/// Loop of a link back up the chain: out to the right, over the top, in.
fn back_loop(x0: f64, y0: f64, x1: f64, y1: f64, lift: f64) -> String {
    let top = TOP - LOOP_ROOM + lift;
    format!(
        "M {x0} {y0} C {} {y0}, {} {top}, {x0} {top} L {x1} {top} C {} {top}, {} {y1}, {x1} {y1}",
        x0 + 40.0,
        x0 + 40.0,
        x1 - 40.0,
        x1 - 40.0
    )
}

#[component]
pub(super) fn Money(workspace: CaseWorkspace) -> Element {
    let ws = use_context::<Ws>();
    let mut currency = use_signal(|| None::<String>);
    let mut until_cursor = use_signal(|| false);
    let mut band_sel = use_signal(|| None::<usize>);
    let mut hovered = use_signal(|| None::<usize>);
    let fr = french();

    let all = money::transfers(&workspace);
    if all.is_empty() {
        return rsx! {
            div { class: "absolute inset-0 flex items-center justify-center p-6",
                div { class: "max-w-md space-y-2 text-center text-sm text-slate",
                    p { class: "font-semibold text-ink", {t!("money-empty-title")} }
                    p { {t!("money-empty-help")} }
                }
            }
        };
    }
    let currencies = money::currencies(&all);
    let cur = currency()
        .filter(|c| currencies.contains(c))
        .unwrap_or_else(|| currencies[0].clone());
    let cursor = (ws.cursor)();
    let starts: HashMap<&str, i64> = workspace
        .events
        .iter()
        .filter_map(|e| model::span(&e.event).map(|s| (e.event.id.as_str(), s.0)))
        .collect();
    let shown: Vec<Transfer> = all
        .iter()
        .filter(|t| t.currency == cur)
        .filter(|t| {
            !until_cursor()
                || starts
                    .get(t.event_id.as_str())
                    .is_some_and(|s| *s <= cursor)
        })
        .cloned()
        .collect();
    let later = all.iter().filter(|t| t.currency == cur).count() - shown.len();

    // Height from the fullest column, then the real layout.
    let probe = money::layout(&shown, 1.0, 0.0);
    let fullest = (0..probe.cols)
        .map(|c| probe.nodes.iter().filter(|n| n.col == c).count())
        .max()
        .unwrap_or(1);
    let height = (fullest as f64 * ROW_H).max(MIN_H);
    let s: Sankey = money::layout(&shown, height, PAD);

    let titles: HashMap<String, String> = workspace
        .events
        .iter()
        .flat_map(|e| e.attachments.iter())
        .map(|a| (a.object.id.clone(), a.object.title.clone()))
        .collect();
    let unsourced: std::collections::HashSet<String> = shown
        .iter()
        .filter(|t| !t.sourced)
        .map(|t| t.event_id.clone())
        .collect();
    let events: HashMap<&str, &pnex_core::ontology::api::ObjectView> = workspace
        .events
        .iter()
        .map(|e| (e.event.id.as_str(), &e.event))
        .collect();
    // Nodes take their family colour, as in the links graph: one object,
    // one colour everywhere.
    let kinds: HashMap<String, String> = workspace
        .events
        .iter()
        .flat_map(|e| e.attachments.iter())
        .map(|a| (a.object.id.clone(), a.object.type_key.clone()))
        .collect();
    let colour: Vec<&'static str> = s
        .nodes
        .iter()
        .map(|n| match kinds.get(&n.id).map(|k| family(k)) {
            Some("person") => "var(--color-signal)",
            Some("organization") => "var(--color-ok)",
            Some("place" | "identifier") => "var(--color-warn)",
            _ => "var(--color-idle)",
        })
        .collect();
    let traced: f64 = s
        .links
        .iter()
        .filter(|l| s.nodes[l.to].kind != NodeKind::Untraced && s.nodes[l.from].col == 0)
        .map(|l| l.value)
        .sum();
    let untraced: f64 = s
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Untraced)
        .map(|n| n.value_in)
        .sum();
    let loops = s.links.iter().filter(|l| l.back).count();
    let x = |col: usize| LEFT + col as f64 * COL_W;
    // Left edge of a node: its column, or a short stub for what is untraced.
    let nx = |n: &money::SankeyNode| {
        if n.kind == NodeKind::Untraced {
            x(n.col.saturating_sub(1)) + NODE_W + STUB
        } else {
            x(n.col)
        }
    };
    let width = s
        .nodes
        .iter()
        .map(|n| {
            nx(n)
                + NODE_W
                + if n.kind == NodeKind::Untraced {
                    130.0
                } else {
                    220.0
                }
        })
        .fold(0.0, f64::max);
    let total_h = TOP + height + 24.0;
    let selected_obj = match (ws.sel)() {
        Sel::Object(id) => Some(id),
        _ => None,
    };
    let cur_label = cur.clone();
    let (c1, c2) = (cur.clone(), cur);
    let amount = move |v: f64| format!("{} {c1}", money::compact_amount(v, fr));
    let amount_full = move |v: f64| format!("{} {c2}", money::full_amount(v, fr));

    rsx! {
        div {
            class: "absolute inset-0 overflow-auto px-4 pb-4 pt-16",
            "data-testid": "case-money",
            // Summary and controls.
            div { class: "mb-3 flex flex-wrap items-center gap-x-6 gap-y-2 text-sm",
                span { class: "text-slate",
                    {t!("money-traced")}
                    " "
                    strong {
                        class: "font-mono text-ink",
                        "data-testid": "money-total",
                        {amount_full(traced)}
                    }
                }
                if untraced > 0.0 {
                    span { class: "text-slate",
                        {t!("money-untraced")}
                        " "
                        strong {
                            class: "font-mono text-ink",
                            "data-testid": "money-untraced",
                            {amount_full(untraced)}
                        }
                    }
                }
                if loops > 0 {
                    span { class: "text-ink", "data-testid": "money-loops",
                        {t!("money-loops", count : loops)}
                    }
                }
                if currencies.len() > 1 {
                    select {
                        class: "{crate::components::ui::INPUT} w-28",
                        "aria-label": t!("onto-money-currency"),
                        onchange: move |e| currency.set(Some(e.value())),
                        for c in currencies.iter() {
                            option {
                                key: "{c}",
                                value: "{c}",
                                selected: *c == cur_label,
                                "{c}"
                            }
                        }
                    }
                }
                label { class: "flex items-center gap-1 text-slate",
                    input {
                        r#type: "checkbox",
                        checked: until_cursor(),
                        onchange: move |e| until_cursor.set(e.checked()),
                    }
                    {t!("money-until-cursor")}
                }
                if until_cursor() && later > 0 {
                    span { class: "text-xs text-slate", {t!("money-later", count : later)} }
                }
            }
            Legend {}
            if s.nodes.is_empty() {
                p { class: "py-12 text-center text-sm text-slate", {t!("money-none-yet")} }
            } else {
                svg {
                    width: "{width}",
                    height: "{total_h}",
                    view_box: "0 0 {width} {total_h}",
                    role: "img",
                    "aria-label": t!("money-view"),
                    defs {
                        // The timeline's hatching for what is not known (signal gaps).
                        pattern {
                            id: "money-hatch",
                            width: "6",
                            height: "6",
                            pattern_units: "userSpaceOnUse",
                            pattern_transform: "rotate(45)",
                            line {
                                x1: "0",
                                y1: "0",
                                x2: "0",
                                y2: "6",
                                stroke: "var(--color-slate)",
                                stroke_width: "2",
                            }
                        }
                    }
                    // Bands first, nodes over them.
                    for (j, l) in s.links.iter().enumerate() {
                        {
                            let (a, b) = (&s.nodes[l.from], &s.nodes[l.to]);
                            let (x0, x1) = (nx(a) + NODE_W, nx(b));
                            let (y0, y1) = (TOP + l.y0 + l.w / 2.0, TOP + l.y1 + l.w / 2.0);
                            let d = if l.back {
                                back_loop(x0, y0, x(b.col), y1, -(j as f64 % 4.0) * 6.0)
                            } else {
                                band(x0, y0, x1, y1)
                            };
                            let lit = band_sel() == Some(j) || hovered() == Some(j);
                            let untraced_band = b.kind == NodeKind::Untraced;
                            let bare = l.unsourced && !untraced_band;
                            let opacity = match (lit, untraced_band, l.estimated || l.back || bare) {
                                (true, _, _) => "0.8",
                                (false, true, _) => "0.55",
                                (false, false, true) => "0.22",
                                _ => "0.45",
                            };
                            let d2 = d.clone();
                            let tip = format!(
                                "{} → {} : {}",
                                label(&titles, &a.id, a.kind),
                                label(&titles, &b.id, b.kind),
                                amount_full(l.value),
                            );
                            rsx! {
                                path {
                                    key: "l{j}",
                                    d,
                                    fill: "none",
                                    stroke: if untraced_band { "url(#money-hatch)" } else if l.back { "var(--color-warn)" } else { "var(--color-signal)" },
                                    stroke_width: "{l.w.max(1.5)}",
                                    stroke_opacity: opacity,
                                    class: "cursor-pointer",
                                    "data-testid": "money-band",
                                    onmouseenter: move |_| hovered.set(Some(j)),
                                    onmouseleave: move |_| hovered.set(None),
                                    onclick: move |_| band_sel.set(if band_sel() == Some(j) { None } else { Some(j) }),
                                    title { "{tip}" }
                                }
                                // No piece behind one of its payments (K52): pale, with a dotted thread.
                                if bare {
                                    path {
                                        d: d2,
                                        fill: "none",
                                        stroke: if l.back { "var(--color-warn)" } else { "var(--color-signal)" },
                                        stroke_width: "1.5",
                                        stroke_dasharray: "4 4",
                                        pointer_events: "none",
                                    }
                                }
                            }
                        }
                    }
                    for (i, n) in s.nodes.iter().enumerate() {
                        {
                            let name = label(&titles, &n.id, n.kind);
                            let value = n.value_in.max(n.value_out);
                            let last = n.col + 1 == s.cols || n.kind == NodeKind::Untraced;
                            // Labels sit before their node, except at the end of a chain.
                            let (tx, anchor) = if last {
                                (nx(n) + NODE_W + 8.0, "start")
                            } else {
                                (nx(n) - 8.0, "end")
                            };
                            let is_obj = n.kind == NodeKind::Object;
                            let sel = is_obj && selected_obj.as_deref() == Some(n.id.as_str());
                            let id = n.id.clone();
                            let ty = TOP + n.y + n.h / 2.0;
                            rsx! {
                                g {
                                    key: "n{i}",
                                    class: if is_obj { "cursor-pointer" } else { "" },
                                    onclick: move |_| {
                                        if is_obj {
                                            let mut s = ws.sel;
                                            s.set(Sel::Object(id.clone()));
                                        }
                                    },
                                    rect {
                                        x: "{nx(n)}",
                                        y: "{TOP + n.y}",
                                        width: "{NODE_W}",
                                        height: "{n.h.max(2.0)}",
                                        rx: "2",
                                        fill: if n.kind == NodeKind::Untraced { "url(#money-hatch)" } else if is_obj { colour[i] } else { "var(--color-idle)" },
                                        stroke: if sel { "var(--color-signal)" } else { "none" },
                                        stroke_width: "3",
                                    }
                                    text {
                                        x: "{tx}",
                                        y: "{ty - 3.0}",
                                        text_anchor: anchor,
                                        class: if is_obj { "fill-ink text-sm font-semibold" } else { "fill-slate text-sm italic" },
                                        style: "paint-order: stroke; stroke: var(--color-ground); stroke-width: 4px; stroke-linejoin: round",
                                        {model::short(&name, 30)}
                                    }
                                    text {
                                        x: "{tx}",
                                        y: "{ty + 13.0}",
                                        text_anchor: anchor,
                                        class: "fill-slate text-xs font-mono",
                                        style: "paint-order: stroke; stroke: var(--color-ground); stroke-width: 4px; stroke-linejoin: round",
                                        {amount(value)}
                                    }
                                }
                            }
                        }
                    }
                }
                // Payments behind the selected band (details on demand).
                if let Some(l) = band_sel().and_then(|j| s.links.get(j)) {
                    div {
                        // Sticky: stays in sight under the diagram while it scrolls.
                        class: "sticky bottom-0 left-0 mt-3 max-w-xl rounded-lg border border-line bg-surface p-3 shadow-sm",
                        "data-testid": "money-band-detail",
                        p { class: "mb-2 text-sm font-semibold text-ink",
                            {label(&titles, &s.nodes[l.from].id, s.nodes[l.from].kind)}
                            " → "
                            {label(&titles, &s.nodes[l.to].id, s.nodes[l.to].kind)}
                            " · "
                            span { class: "font-mono", {amount_full(l.value)} }
                        }
                        if l.events.is_empty() {
                            p { class: "text-sm text-slate", {t!("money-untraced-help")} }
                        }
                        ul { class: "divide-y divide-line",
                            for eid in l.events.iter() {
                                {
                                    let ev = events.get(eid.as_str());
                                    let title = ev.map(|e| e.title.clone()).unwrap_or_default();
                                    let value = ev
                                        .and_then(|e| e.properties.get(pnex_core::ontology::EVENT_AMOUNT))
                                        .map(crate::components::ontology::money_label)
                                        .unwrap_or_default();
                                    let when = ev
                                        .and_then(|e| model::span(e))
                                        .map(|s| model::day(s.0, (ws.local_time)()))
                                        .unwrap_or_default();
                                    let id = eid.clone();
                                    let bare = unsourced.contains(eid);
                                    rsx! {
                                        li { key: "{eid}",
                                            button {
                                                r#type: "button",
                                                class: "flex w-full items-baseline gap-3 py-1.5 text-left text-sm hover:text-signal",
                                                onclick: move |_| ws.select_event(id.clone()),
                                                span { class: "font-mono text-xs text-slate", "{when}" }
                                                span { class: "flex-1 truncate text-ink", "{title}" }
                                                if bare {
                                                    span { class: "rounded border border-line-strong px-1 text-xs text-slate",
                                                        {t!("money-unsourced")}
                                                    }
                                                }
                                                span { class: "font-mono text-xs text-ink", "{value}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Swatches rather than a sentence: what each kind of band means.
#[component]
fn Legend() -> Element {
    let swatch = |stroke: &'static str, dash: &'static str, opacity: &'static str| {
        rsx! {
            svg { width: "28", height: "10", "aria-hidden": "true",
                line {
                    x1: "0",
                    y1: "5",
                    x2: "28",
                    y2: "5",
                    stroke,
                    stroke_width: "8",
                    stroke_opacity: opacity,
                    stroke_dasharray: dash,
                }
            }
        }
    };
    rsx! {
        ul { class: "mb-2 flex flex-wrap items-center gap-x-5 gap-y-1 text-xs text-slate",
            li { class: "flex items-center gap-1.5",
                {swatch("var(--color-signal)", "none", "0.45")}
                {t!("money-legend-sourced")}
            }
            li { class: "flex items-center gap-1.5",
                svg { width: "28", height: "10", "aria-hidden": "true",
                    line {
                        x1: "0",
                        y1: "5",
                        x2: "28",
                        y2: "5",
                        stroke: "var(--color-signal)",
                        stroke_width: "8",
                        stroke_opacity: "0.22",
                    }
                    line {
                        x1: "0",
                        y1: "5",
                        x2: "28",
                        y2: "5",
                        stroke: "var(--color-signal)",
                        stroke_width: "1.5",
                        stroke_dasharray: "4 4",
                    }
                }
                {t!("money-legend-unsourced")}
            }
            li { class: "flex items-center gap-1.5",
                svg { width: "28", height: "10", "aria-hidden": "true",
                    defs {
                        pattern {
                            id: "money-hatch-legend",
                            width: "6",
                            height: "6",
                            pattern_units: "userSpaceOnUse",
                            pattern_transform: "rotate(45)",
                            line {
                                x1: "0",
                                y1: "0",
                                x2: "0",
                                y2: "6",
                                stroke: "var(--color-slate)",
                                stroke_width: "2",
                            }
                        }
                    }
                    rect {
                        width: "28",
                        height: "10",
                        fill: "url(#money-hatch-legend)",
                        fill_opacity: "0.55",
                    }
                }
                {t!("money-untraced")}
            }
            li { class: "flex items-center gap-1.5",
                {swatch("var(--color-warn)", "none", "0.5")}
                {t!("money-legend-back")}
            }
            li { {t!("money-legend-width")} }
        }
    }
}
