//! The price chart of an issuer or an instrument (markets-research.md D813):
//! raw closes as one line, abnormal sessions as triangles on it, filings as
//! ticks on the time axis. Focusing a marker names it under the chart.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::markets::chart::{draw, ChartEventKind, H, PLOT, W};

use crate::api;
use crate::app::Route;
use crate::components::ui::{TAB, TAB_ACTIVE};

const RANGES: [(u32, &str); 3] = [
    (365, "onto-chart-range-1y"),
    (1825, "onto-chart-range-5y"),
    (3650, "onto-chart-range-10y"),
];

#[component]
pub fn ChartPanel(id: String) -> Element {
    let mut days = use_signal(|| 365u32);
    let mut picked = use_signal(|| None::<usize>);
    let mut model = use_signal(String::new);
    let chart = use_resource(move || {
        let id = id.clone();
        let days = days();
        async move { api::markets::issuer_chart(&id, days).await }
    });
    let read = chart.read();
    let c = match &*read {
        Some(Ok(c)) => {
            let mut c = c.clone();
            // D897: one model's decisions at a time when asked.
            let pick = model();
            c.events
                .retain(|e| pick.is_empty() || e.model.as_ref().is_none_or(|m| *m == pick));
            c
        }
        Some(Err(e)) => {
            return rsx! {
                p { class: "text-sm text-alarm", {crate::api::error_i18n::localize(e)} }
            };
        }
        None => return rsx! {},
    };
    let drawing = draw(&c);
    let ticker = c.ticker.clone().unwrap_or_default();
    let (left, top, right, bottom) = PLOT;
    let label = t!("onto-chart-aria", ticker: ticker.clone());
    let picked_event = picked().and_then(|i| c.events.get(i).cloned());
    let models: std::collections::BTreeSet<String> = match &*read {
        Some(Ok(all)) => all.events.iter().filter_map(|e| e.model.clone()).collect(),
        _ => Default::default(),
    };
    rsx! {
        div {
            class: "bg-surface rounded-lg border border-line p-4 space-y-3",
            "data-testid": "onto-chart",
            div { class: "flex flex-wrap items-baseline justify-between gap-2",
                h3 { class: "text-sm font-semibold text-ink",
                    if ticker.is_empty() {
                        {t!("onto-chart-title")}
                    } else {
                        {t!("onto-chart-title-ticker", ticker : ticker.clone())}
                    }
                }
                if !models.is_empty() {
                    select {
                        class: "rounded-md border border-line-strong bg-surface px-2 py-1 text-sm text-ink",
                        "aria-label": t!("onto-chart-filter-model"),
                        "data-testid": "onto-chart-model",
                        onchange: move |e| {
                            picked.set(None);
                            model.set(e.value());
                        },
                        option { value: "", selected: model().is_empty(),
                            {t!("onto-chart-all-models")}
                        }
                        for m in models {
                            option {
                                key: "{m}",
                                value: "{m}",
                                selected: model() == m,
                                "{m}"
                            }
                        }
                    }
                }
                div { class: "flex gap-1", role: "group",
                    for (n, key) in RANGES {
                        button {
                            key: "{n}",
                            r#type: "button",
                            class: if days() == n { TAB_ACTIVE } else { TAB },
                            aria_pressed: days() == n,
                            "data-testid": "onto-chart-range-{n}",
                            onclick: move |_| {
                                picked.set(None);
                                days.set(n);
                            },
                            {t!(key)}
                        }
                    }
                }
            }
            if let Some(d) = drawing {
                svg {
                    class: "w-full h-auto",
                    view_box: "0 0 {W} {H}",
                    role: "img",
                    "aria-label": "{label}",
                    line {
                        x1: "{left}",
                        y1: "{bottom}",
                        x2: "{right}",
                        y2: "{bottom}",
                        class: "stroke-line",
                        stroke_width: "1",
                    }
                    text {
                        x: "{left - 6.0}",
                        y: "{top + 8.0}",
                        text_anchor: "end",
                        class: "fill-slate",
                        font_size: "10",
                        "{d.max:.2}"
                    }
                    // A flat series has one value: shown once.
                    if d.min < d.max {
                        text {
                            x: "{left - 6.0}",
                            y: "{bottom}",
                            text_anchor: "end",
                            class: "fill-slate",
                            font_size: "10",
                            "{d.min:.2}"
                        }
                    }
                    text {
                        x: "{left}",
                        y: "{H - 8.0}",
                        class: "fill-slate",
                        font_size: "10",
                        "{d.first}"
                    }
                    text {
                        x: "{right}",
                        y: "{H - 8.0}",
                        text_anchor: "end",
                        class: "fill-slate",
                        font_size: "10",
                        "{d.last}"
                    }
                    polyline {
                        points: "{d.line}",
                        fill: "none",
                        class: "stroke-ink",
                        stroke_width: "1.5",
                        stroke_linejoin: "round",
                    }
                    for (x, i) in d.marks.clone() {
                        Marker {
                            key: "{i}",
                            x,
                            index: i,
                            kind: c.events[i].kind,
                            up: c.events[i].detail == "up",
                            option: c.events[i].detail.clone(),
                            probability: c.events[i].probability.unwrap_or(1.0),
                            title: c.events[i].title.clone(),
                            on: picked() == Some(i),
                            onpick: move |i| picked.set(Some(i)),
                        }
                    }
                }
                div {
                    class: "min-h-6 text-sm break-words",
                    "data-testid": "onto-chart-picked",
                    if let Some(e) = picked_event.clone().filter(|e| e.kind == ChartEventKind::Decision) {
                        span { class: "text-slate mr-2", "{e.date}" }
                        Link {
                            class: "text-signal hover:underline",
                            to: Route::MarketDecisions {},
                            "{e.title}: {e.detail} {e.probability.unwrap_or_default() * 100.0:.0} %"
                        }
                    } else if let Some(e) = picked_event {
                        span { class: "text-slate mr-2", "{e.date}" }
                        Link {
                            class: "text-signal hover:underline",
                            to: Route::OntologyObject {
                                id: e.id.clone(),
                            },
                            "{e.title}"
                        }
                    } else {
                        span { class: "text-slate", {t!("onto-chart-hint")} }
                    }
                }
            } else {
                p {
                    class: "text-sm text-slate",
                    "data-testid": "onto-chart-empty",
                    {t!("onto-chart-empty")}
                }
            }
        }
    }
}

/// One marker: a triangle on the top band for a session, a tick on the
/// axis for a filing. Focusable, so the keyboard reaches each one.
#[component]
fn Marker(
    x: f64,
    index: usize,
    kind: ChartEventKind,
    up: bool,
    option: String,
    probability: f64,
    title: String,
    on: bool,
    onpick: EventHandler<usize>,
) -> Element {
    let (_, top, _, bottom) = PLOT;
    let tone = if on {
        "fill-signal stroke-signal"
    } else {
        "fill-warn stroke-warn"
    };
    let shape = match kind {
        ChartEventKind::Move if up => {
            format!(
                "{:.1},{:.1} {:.1},{:.1} {:.1},{:.1}",
                x - 5.0,
                top + 10.0,
                x + 5.0,
                top + 10.0,
                x,
                top
            )
        }
        ChartEventKind::Move => {
            format!(
                "{:.1},{:.1} {:.1},{:.1} {:.1},{:.1}",
                x - 5.0,
                top,
                x + 5.0,
                top,
                x,
                top + 10.0
            )
        }
        ChartEventKind::Filing | ChartEventKind::Decision => String::new(),
    };
    // Buy and sell keep the colours of the decisions page.
    let dot = match option.as_str() {
        "buy" => "fill-ok",
        "sell" => "fill-alarm",
        _ => "fill-slate",
    };
    rsx! {
        g {
            tabindex: "0",
            class: "cursor-pointer outline-none",
            "data-testid": "onto-chart-mark",
            onmouseenter: move |_| onpick.call(index),
            onfocus: move |_| onpick.call(index),
            onclick: move |_| onpick.call(index),
            title { "{title}" }
            if kind == ChartEventKind::Filing {
                line {
                    x1: "{x}",
                    y1: "{bottom - 8.0}",
                    x2: "{x}",
                    y2: "{bottom}",
                    class: if on { "stroke-signal" } else { "stroke-slate" },
                    stroke_width: if on { "2.5" } else { "1.5" },
                }
                // A wider transparent band: a 1.5 px tick is too thin to hover.
                rect {
                    x: "{x - 4.0}",
                    y: "{bottom - 12.0}",
                    width: "8",
                    height: "12",
                    fill: "transparent",
                }
            } else if kind == ChartEventKind::Decision {
                // A faint dot is an unsure decision.
                circle {
                    cx: "{x}",
                    cy: "{top + 22.0}",
                    r: if on { "5" } else { "3.5" },
                    class: if on { "fill-signal" } else { dot },
                    fill_opacity: "{0.25 + 0.75 * probability.clamp(0.0, 1.0):.2}",
                }
            } else {
                polygon { points: "{shape}", class: "{tone}" }
            }
        }
    }
}
