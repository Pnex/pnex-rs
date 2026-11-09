//! Page `/market-decisions` (markets-research.md D897): the decisions posted
//! by the runners of external models, and how well each model did once the
//! horizon passed. Read only: a decision is advice, nothing is ordered here.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::markets::decisions::{Decision, ModelScores, ReliabilityBin, OPTIONS};

use crate::api;
use crate::components::crud::filters::{FilterBar, SearchInput};
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ui::{BTN_SECONDARY_SM, FIELD_LABEL, INPUT};

/// Fill of an option: buy and sell keep the colours of the rest of the app.
pub(crate) fn option_tone(option: &str) -> &'static str {
    match option {
        "buy" => "bg-ok",
        "sell" => "bg-alarm",
        _ => "bg-slate",
    }
}

fn option_word(option: &str) -> String {
    match option {
        "buy" => t!("decisions-option-buy").to_string(),
        "sell" => t!("decisions-option-sell").to_string(),
        _ => t!("decisions-option-neutral").to_string(),
    }
}

fn pct(v: f64) -> String {
    format!("{:.0} %", v * 100.0)
}

/// The three probabilities as one bar, each segment as wide as its share.
#[component]
fn Distribution(probabilities: Vec<f64>) -> Element {
    let text = OPTIONS
        .iter()
        .zip(&probabilities)
        .map(|(o, p)| format!("{} {}", option_word(o), pct(*p)))
        .collect::<Vec<_>>()
        .join(", ");
    rsx! {
        div {
            class: "flex h-2 w-32 overflow-hidden rounded-full bg-line",
            role: "img",
            "aria-label": "{text}",
            title: "{text}",
            for (o, p) in OPTIONS.iter().zip(probabilities.clone()) {
                span {
                    key: "{o}",
                    class: "h-full {option_tone(o)}",
                    style: "width: {p * 100.0:.1}%",
                }
            }
        }
    }
}

/// Predicted against realized per tenth: on the diagonal, the model says
/// what happens.
#[component]
fn Reliability(bins: Vec<ReliabilityBin>) -> Element {
    const S: f64 = 64.0;
    let label = t!("decisions-reliability-aria");
    rsx! {
        svg {
            class: "h-16 w-16 shrink-0",
            view_box: "0 0 {S} {S}",
            role: "img",
            "aria-label": "{label}",
            rect {
                x: "0.5",
                y: "0.5",
                width: "{S - 1.0}",
                height: "{S - 1.0}",
                fill: "none",
                class: "stroke-line",
            }
            line {
                x1: "0",
                y1: "{S}",
                x2: "{S}",
                y2: "0",
                class: "stroke-line",
                stroke_dasharray: "2 2",
            }
            for b in bins {
                circle {
                    key: "{b.from}",
                    cx: "{b.mean_probability * S}",
                    cy: "{(1.0 - b.observed) * S}",
                    r: "{(2.0 + (b.count as f64).log10()).min(5.0)}",
                    class: "fill-signal",
                }
            }
        }
    }
}

#[component]
pub fn MarketDecisions() -> Element {
    let mut reload = use_signal(|| 0u32);
    let page = use_signal(|| 0i64);
    let model = use_signal(String::new);
    let instrument = use_signal(String::new);
    let list = use_resource(move || {
        let offset = page() * PAGE_SIZE;
        async move {
            let _ = reload();
            api::markets::decisions(&model.peek(), &instrument.peek(), PAGE_SIZE, offset).await
        }
    });
    let scores = use_resource(move || async move {
        let _ = reload();
        api::markets::decision_scores(&model.peek(), &instrument.peek()).await
    });
    let (state, count, rows) = match &*list.value().read() {
        None => (None, 0, Vec::new()),
        Some(Ok(p)) => (Some(Ok(())), p.count, p.results.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), 0, Vec::new()),
    };
    let measured: Vec<ModelScores> = match &*scores.value().read() {
        Some(Ok(p)) => p.results.clone(),
        _ => Vec::new(),
    };
    let columns = vec![
        Column::new(t!("decisions-col-as-of").to_string(), |d: &Decision| {
            let when = d.as_of.get(..16).unwrap_or(&d.as_of).replace('T', " ");
            rsx! {
                span { class: "whitespace-nowrap tabular-nums", "{when}" }
            }
        }),
        Column::new(t!("decisions-col-model").to_string(), |d: &Decision| {
            rsx! {
                span { class: "block font-medium text-ink break-words", "{d.model}" }
                span { class: "block text-xs text-slate break-words", "{d.model_version}" }
            }
        }),
        Column::new(
            t!("decisions-col-instrument").to_string(),
            |d: &Decision| {
                rsx! {
                    span { class: "block break-words", "{d.instrument}" }
                    span { class: "block text-xs text-slate break-words", "{d.source}, {d.interval}" }
                }
            },
        ),
        Column::new(t!("decisions-col-best").to_string(), |d: &Decision| {
            let p = OPTIONS
                .iter()
                .position(|o| *o == d.best)
                .and_then(|i| d.probabilities.get(i).copied())
                .unwrap_or_default();
            rsx! {
                span { class: "inline-flex items-center gap-2 whitespace-nowrap",
                    span {
                        class: "h-2 w-2 shrink-0 rounded-full {option_tone(&d.best)}",
                        aria_hidden: "true",
                    }
                    "{option_word(&d.best)} {pct(p)}"
                }
            }
        }),
        Column::new(
            t!("decisions-col-distribution").to_string(),
            |d: &Decision| {
                rsx! {
                    Distribution { probabilities: d.probabilities.clone() }
                }
            },
        )
        .secondary(),
        Column::new(
            t!("decisions-col-confidence").to_string(),
            |d: &Decision| {
                rsx! {
                    span { class: "tabular-nums", "{pct(d.confidence)}" }
                }
            },
        )
        .secondary(),
    ];
    let refresh = move |_| {
        let (mut p, mut r) = (page, reload);
        p.set(0);
        r.with_mut(|r| *r += 1);
    };
    rsx! {
        ListLayout {
            title: t!("decisions-title").to_string(),
            subtitle: Some(t!("decisions-subtitle").to_string()),
            can_write: false,
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            filters: rsx! {
                FilterBar {
                    SearchInput {
                        placeholder: t!("decisions-filter-model").to_string(),
                        value: model,
                        on_submit: refresh,
                    }
                    SearchInput {
                        placeholder: t!("decisions-filter-instrument").to_string(),
                        value: instrument,
                        on_submit: refresh,
                    }
                }
            },
            div { class: "space-y-6",
                p {
                    class: "rounded-md border border-line bg-surface px-3 py-2 text-sm text-slate",
                    "data-testid": "decisions-advice",
                    {t!("decisions-advice")}
                }
                AnalyseAsOf {}
                if !measured.is_empty() {
                    section { class: "space-y-2", "data-testid": "decisions-scores",
                        h2 { class: "text-sm font-semibold text-ink", {t!("decisions-scores-title")} }
                        ul { class: "divide-y divide-line rounded-lg border border-line bg-surface",
                            for (i, m) in measured.into_iter().enumerate() {
                                ScoreRow { key: "{i}", m }
                            }
                        }
                    }
                }
                ListStates {
                    state,
                    is_empty: count == 0,
                    empty_message: t!("decisions-empty").to_string(),
                    div { class: "space-y-4",
                        DataTable {
                            columns,
                            rows,
                            row_key: RowKey::new(|d: &Decision| d.id.clone()),
                        }
                        ListPager { count, page }
                    }
                }
            }
        }
    }
}

/// "Analysis at a date" (D902): opens the assistant frozen at an instant,
/// so every answer is what was known then, never later.
#[component]
fn AnalyseAsOf() -> Element {
    let mut at = use_signal(String::new);
    // `datetime-local` gives `2026-01-06T10:15`, read as UTC.
    let instant = chrono::NaiveDateTime::parse_from_str(&at(), "%Y-%m-%dT%H:%M")
        .ok()
        .map(|t| t.and_utc())
        .filter(|t| *t <= chrono::Utc::now());
    rsx! {
        section {
            class: "flex flex-wrap items-end gap-2 rounded-lg border border-line bg-surface px-3 py-2",
            "data-testid": "decisions-analyse-as-of",
            div { class: "min-w-0",
                label { class: FIELD_LABEL, r#for: "decisions-as-of", {t!("decisions-as-of-label")} }
                input {
                    id: "decisions-as-of",
                    class: INPUT,
                    r#type: "datetime-local",
                    value: "{at}",
                    oninput: move |e| at.set(e.value()),
                }
            }
            button {
                class: BTN_SECONDARY_SM,
                r#type: "button",
                disabled: instant.is_none(),
                onclick: move |_| {
                    if let Some(t) = instant {
                        crate::components::assistant::open_frozen(t.to_rfc3339());
                    }
                },
                {t!("decisions-as-of-ask")}
            }
            p { class: "basis-full text-xs text-slate", {t!("decisions-as-of-help")} }
        }
    }
}

/// One model on one instrument: its curve, then its numbers.
#[component]
fn ScoreRow(m: ModelScores) -> Element {
    let s = &m.scores;
    let opt = |v: Option<f64>, f: fn(f64) -> String| v.map(f).unwrap_or_else(|| "–".into());
    let hit = opt(s.hit_rate, pct);
    let brier = opt(s.brier, |b| format!("{b:.3}"));
    let above = opt(s.above_threshold_hit_rate, pct);
    let threshold = pct(s.threshold);
    rsx! {
        li { class: "flex flex-wrap items-center gap-4 px-3 py-3",
            Reliability { bins: s.reliability.clone() }
            div { class: "min-w-40 flex-1",
                p { class: "font-medium text-ink break-words", "{m.model} {m.model_version}" }
                p { class: "text-xs text-slate break-words",
                    "{m.instrument}, {m.source}, {m.interval}"
                }
            }
            dl { class: "grid grid-cols-2 gap-x-6 gap-y-1 text-sm sm:grid-cols-4",
                dt { class: "text-slate", {t!("decisions-measured")} }
                dd { class: "tabular-nums text-ink", "{s.measured} / {s.decisions}" }
                dt { class: "text-slate", {t!("decisions-hit-rate")} }
                dd { class: "tabular-nums text-ink", "{hit}" }
                dt { class: "text-slate", {t!("decisions-brier")} }
                dd { class: "tabular-nums text-ink", "{brier}" }
                dt { class: "text-slate", {t!("decisions-above", threshold : threshold.clone())} }
                dd { class: "tabular-nums text-ink", "{s.above_threshold} ({above})" }
            }
        }
    }
}
