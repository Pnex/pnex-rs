//! Story view of a case (xkcd "Movie Narrative Charts", storylines): each
//! actor that comes back in two events or more is a thread running through
//! time, left to right; the threads of an event gather at its column. A
//! click on a thread selects the actor, on a column title the event: the
//! inspector opens as in the other views.

use std::collections::HashMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{CaseWorkspace, ObjectRef};

use super::{model, timeline::actor_color, Sel, Ws};

const LEFT: f64 = 250.0;
const TOP: f64 = 110.0;
const COL_W: f64 = 64.0;
const GAP: f64 = 46.0;
const TIGHT: f64 = 16.0;
const BOTTOM: f64 = 170.0;
/// Threads shown before "show all".
const TOP_ACTORS: usize = 12;
/// Context objects, not actors: they never get a thread.
const NOT_ACTORS: [&str; 8] = [
    "place",
    "piece",
    "claim",
    "hypothesis",
    "identifier",
    "map_pin",
    "doc",
    "demand",
];

/// Actors of the case by number of events, most present first.
fn actors(w: &CaseWorkspace) -> Vec<(ObjectRef, usize)> {
    let mut count: HashMap<String, (ObjectRef, usize)> = HashMap::new();
    for e in &w.events {
        for a in &e.attachments {
            if NOT_ACTORS.contains(&a.object.type_key.as_str()) {
                continue;
            }
            count
                .entry(a.object.id.clone())
                .or_insert_with(|| (a.object.clone(), 0))
                .1 += 1;
        }
    }
    let mut out: Vec<_> = count.into_values().filter(|(_, n)| *n >= 2).collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.title.cmp(&b.0.title)));
    out
}

/// `DD/MM` of a `YYYY-MM-DD` day.
fn day_month(day: &str) -> String {
    format!(
        "{}/{}",
        day.get(8..10).unwrap_or(""),
        day.get(5..7).unwrap_or("")
    )
}

/// SVG path of a thread through its column positions (smooth steps).
fn thread_path(ys: &[f64]) -> String {
    let x = |c: usize| LEFT + c as f64 * COL_W;
    let mut d = format!("M {} {} L {} {}", x(0) - 40.0, ys[0], x(0), ys[0]);
    for c in 1..ys.len() {
        let (x0, x1) = (x(c - 1), x(c));
        d.push_str(&format!(
            " C {} {}, {} {}, {} {}",
            x0 + COL_W / 2.0,
            ys[c - 1],
            x1 - COL_W / 2.0,
            ys[c],
            x1,
            ys[c]
        ));
    }
    d
}

#[component]
pub(super) fn Story(workspace: CaseWorkspace) -> Element {
    let ws = use_context::<Ws>();
    let mut show_all = use_signal(|| false);
    let mut hovered = use_signal(|| None::<usize>);
    let local = (ws.local_time)();
    let all = actors(&workspace);
    if all.is_empty() {
        return rsx! {
            div { class: "absolute inset-0 flex items-center justify-center p-6",
                p { class: "max-w-md text-center text-sm text-slate", {t!("cw-story-empty")} }
            }
        };
    }
    let picked: Vec<(ObjectRef, usize)> = if show_all() {
        all.clone()
    } else {
        all.iter().take(TOP_ACTORS).cloned().collect()
    };
    // Threads that meet often run side by side (fewer long dives).
    let first: HashMap<&str, usize> = picked
        .iter()
        .enumerate()
        .map(|(i, (o, _))| (o.id.as_str(), i))
        .collect();
    let first_meets: Vec<Vec<usize>> = workspace
        .events
        .iter()
        .map(|e| {
            e.attachments
                .iter()
                .filter_map(|a| first.get(a.object.id.as_str()).copied())
                .collect()
        })
        .collect();
    let present: Vec<usize> = picked.iter().map(|(_, n)| *n).collect();
    let shown: Vec<(ObjectRef, usize)> = model::story_order(&present, &first_meets)
        .into_iter()
        .map(|i| picked[i].clone())
        .collect();
    let index: HashMap<&str, usize> = shown
        .iter()
        .enumerate()
        .map(|(i, (o, _))| (o.id.as_str(), i))
        .collect();
    // Columns: the events that involve a shown actor, oldest first.
    let columns: Vec<(&pnex_core::ontology::api::CaseEvent, Vec<usize>)> = workspace
        .events
        .iter()
        .filter_map(|e| {
            let on: Vec<usize> = e
                .attachments
                .iter()
                .filter_map(|a| index.get(a.object.id.as_str()).copied())
                .collect();
            (!on.is_empty()).then_some((e, on))
        })
        .collect();
    let meets: Vec<Vec<usize>> = columns.iter().map(|(_, on)| on.clone()).collect();
    let n = shown.len();
    let home: Vec<f64> = (0..n).map(|i| i as f64 * GAP).collect();
    let mut cols = vec![home.clone()];
    cols.extend(model::storyline(n, &meets, GAP, TIGHT));
    cols.push(home);
    let width = LEFT + (cols.len() - 1) as f64 * COL_W + 40.0;
    let height = TOP + (n.max(1) - 1) as f64 * GAP + BOTTOM;
    let base = TOP + (n.max(1) - 1) as f64 * GAP;
    let selected = match (ws.sel)() {
        Sel::Object(id) => index.get(id.as_str()).copied(),
        _ => None,
    };
    let focus = hovered().or(selected);
    let x = |c: usize| LEFT + c as f64 * COL_W;
    rsx! {
        div {
            class: "absolute inset-0 overflow-auto px-4 pb-4 pt-16",
            "data-testid": "case-story",
            if all.len() > TOP_ACTORS {
                div { class: "mb-2 flex justify-end",
                    button {
                        r#type: "button",
                        class: crate::components::ui::BTN_SECONDARY_SM,
                        onclick: move |_| show_all.toggle(),
                        if show_all() {
                            {t!("cw-story-top", count : TOP_ACTORS)}
                        } else {
                            {t!("cw-story-all", count : all.len())}
                        }
                    }
                }
            }
            svg {
                width: "{width}",
                height: "{height}",
                view_box: "0 0 {width} {height}",
                role: "img",
                "aria-label": t!("cw-story"),
                // Event columns: date on top, title below, a click opens it.
                for (c, (e, on)) in columns.iter().enumerate() {
                    g { key: "c{e.event.id}",
                        line {
                            x1: "{x(c + 1)}",
                            y1: "{TOP - 34.0}",
                            x2: "{x(c + 1)}",
                            y2: "{base + 20.0}",
                            class: "stroke-line",
                            stroke_dasharray: "2 4",
                        }
                        text {
                            x: "{x(c + 1)}",
                            y: "{TOP - 42.0}",
                            text_anchor: "middle",
                            class: "fill-slate text-xs font-mono",
                            {
                                model::span(&e.event)
                                    .map(|s| day_month(&model::day(s.0, local)))
                                    .unwrap_or_default()
                            }
                        }
                        if on.len() > 1 {
                            rect {
                                x: "{x(c + 1) - 9.0}",
                                y: "{TOP + on.iter().map(|i| cols[c + 1][*i]).fold(f64::MAX, f64::min) - 10.0}",
                                width: "18",
                                height: "{on.iter().map(|i| cols[c + 1][*i]).fold(f64::MIN, f64::max) - on.iter().map(|i| cols[c + 1][*i]).fold(f64::MAX, f64::min) + 20.0}",
                                rx: "9",
                                class: "fill-ink opacity-[0.07]",
                            }
                        }
                        text {
                            transform: "translate({x(c + 1) + 3.0} {base + 34.0}) rotate(38)",
                            class: "fill-ink text-xs cursor-pointer hover:fill-signal",
                            onclick: {
                                let id = e.event.id.clone();
                                move |_| ws.select_event(id.clone())
                            },
                            {model::short(&e.event.title, 34)}
                        }
                    }
                }
                // Threads, one per actor.
                for (i, (o, _)) in shown.iter().enumerate() {
                    g {
                        key: "t{o.id}",
                        class: "cursor-pointer",
                        opacity: if focus.is_some_and(|f| f != i) { "0.2" } else { "1" },
                        onmouseenter: move |_| hovered.set(Some(i)),
                        onmouseleave: move |_| hovered.set(None),
                        onclick: {
                            let id = o.id.clone();
                            move |_| {
                                let mut sel = ws.sel;
                                sel.set(Sel::Object(id.clone()));
                            }
                        },
                        path {
                            d: thread_path(&cols.iter().map(|c| TOP + c[i]).collect::<Vec<_>>()),
                            fill: "none",
                            stroke: actor_color(i),
                            stroke_width: if selected == Some(i) { "7" } else { "5" },
                            stroke_linecap: "round",
                        }
                        // A wide invisible stroke: easy to hit with a finger.
                        path {
                            d: thread_path(&cols.iter().map(|c| TOP + c[i]).collect::<Vec<_>>()),
                            fill: "none",
                            stroke: "transparent",
                            stroke_width: "18",
                        }
                        text {
                            x: "{x(0) - 46.0}",
                            y: "{TOP + cols[0][i] + 4.0}",
                            text_anchor: "end",
                            class: "fill-ink text-sm font-semibold",
                            {model::short(&o.title, 26)}
                        }
                        for (c, _) in columns.iter().enumerate().filter(|(_, (_, on))| on.contains(&i)) {
                            circle {
                                key: "{c}",
                                cx: "{x(c + 1)}",
                                cy: "{TOP + cols[c + 1][i]}",
                                r: "5",
                                class: "fill-surface",
                                stroke: actor_color(i),
                                stroke_width: "3",
                            }
                        }
                    }
                }
            }
        }
    }
}
